// Qoder/ZCode 信用平台定时调度器(方案见 .trae/documents/merge-creditdaddy-qoder-zcode.md)。
// 单一循环覆盖两平台:每轮 2h±10min 均匀抖动(抖动纯函数在 credit-core schedule.rs),
// Qoder checkin_all + ZCode claim_all 同轮执行,每账号完成发系统通知(成功/失败摘要)。
// generation 计数控制任务生命周期(同 trae_scheduler,无需 abort);tokio 分段 sleep 15s
// 及时响应 stop/restart;下次执行时刻记入 Mutex<Option<DateTime>> 供 get_next_run_time 查询。
// 注意:与 workbuddy 签到周期、TRAE 每日定点循环语义均不同,独立成环互不影响。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Utc};
use tauri::{AppHandle, Manager};
use tauri_plugin_notification::NotificationExt;

use credit_core::schedule::next_round_delay_default;

use crate::qoder_commands::QoderState;
use crate::zcode_commands::ZcodeState;

pub struct CreditSchedulerState {
    pub generation: Arc<AtomicU64>,
    /// 下次执行时刻(睡眠前记录、执行时清空;qoder/zcode_get_next_run_time 查询用)
    pub next_run_at: Mutex<Option<DateTime<Utc>>>,
}

impl Default for CreditSchedulerState {
    fn default() -> Self {
        CreditSchedulerState {
            generation: Arc::new(AtomicU64::new(0)),
            next_run_at: Mutex::new(None),
        }
    }
}

/// 启动定时任务(两平台共用一个循环)。每次调用增 generation,旧任务自行退出;
/// 两平台都未开启自动领取时不启动(增 generation 已等效停止旧任务)。
pub fn start_scheduler(app: AppHandle) {
    let gen = app.state::<CreditSchedulerState>().inner().generation.clone();
    let my_gen = gen.fetch_add(1, Ordering::SeqCst) + 1;

    if !any_auto_enabled(&app) {
        clear_next_run_at(&app);
        return;
    }
    // 立即记录下次执行时刻:save_settings 返回后前端即可查询,不等 spawn 首次落账
    set_next_run_at(&app, next_round_delay_default());

    let app_clone = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            if !is_current(&app_clone, my_gen) {
                break;
            }
            if !any_auto_enabled(&app_clone) {
                break;
            }
            let dur = next_round_delay_default();
            set_next_run_at(&app_clone, dur);
            // 分段 sleep,每 15s 检查 generation,及时响应 stop/restart
            let mut remaining = dur;
            while remaining > Duration::ZERO {
                if !is_current(&app_clone, my_gen) {
                    break;
                }
                let step = remaining.min(Duration::from_secs(15));
                tokio::time::sleep(step).await;
                remaining = remaining.saturating_sub(step);
            }
            clear_next_run_at(&app_clone);
            if !is_current(&app_clone, my_gen) {
                break;
            }
            run_round(&app_clone).await;
        }
    });
}

/// 停止定时任务(增 generation 使当前任务退出,并清空下次执行时刻)。
pub fn stop_scheduler(app: &AppHandle) {
    app.state::<CreditSchedulerState>()
        .inner()
        .generation
        .fetch_add(1, Ordering::SeqCst);
    clear_next_run_at(app);
}

/// 任一平台开启了自动领取?(两平台设置任一 auto_claim_enabled 即调度在跑)
pub fn any_auto_enabled(app: &AppHandle) -> bool {
    let (qoder_on, zcode_on) = auto_flags(app);
    qoder_on || zcode_on
}

/// 下次执行时间(ISO8601)。`platform_enabled` 为对应平台的自动领取开关:
/// 两平台共用一个循环、时刻一致;开关关闭时对前端返回 None。
pub fn get_next_run_time(app: &AppHandle, platform_enabled: bool) -> Option<String> {
    if !platform_enabled {
        return None;
    }
    app.state::<CreditSchedulerState>()
        .inner()
        .next_run_at
        .lock()
        .unwrap()
        .clone()
        .map(|dt| dt.to_rfc3339())
}

// ===== 内部 =====

fn is_current(app: &AppHandle, gen: u64) -> bool {
    app.state::<CreditSchedulerState>()
        .inner()
        .generation
        .load(Ordering::SeqCst)
        == gen
}

/// 两平台自动领取开关(QoderSettings/ZcodeSettings 的 auto_claim_enabled,逐平台读取不嵌套锁)。
fn auto_flags(app: &AppHandle) -> (bool, bool) {
    let qoder_on = {
        let state = app.state::<QoderState>();
        let data = state.core.data.lock().unwrap();
        data.get_settings().auto_claim_enabled
    };
    let zcode_on = {
        let state = app.state::<ZcodeState>();
        let data = state.core.data.lock().unwrap();
        data.get_settings().auto_claim_enabled
    };
    (qoder_on, zcode_on)
}

fn set_next_run_at(app: &AppHandle, dur: Duration) {
    let at = Utc::now() + chrono::Duration::milliseconds(dur.as_millis() as i64);
    *app.state::<CreditSchedulerState>().inner().next_run_at.lock().unwrap() = Some(at);
}

fn clear_next_run_at(app: &AppHandle) {
    *app.state::<CreditSchedulerState>().inner().next_run_at.lock().unwrap() = None;
}

/// 一轮:Qoder 领取 + ZCode 领取(按各平台开关门控;每账号完成发系统通知)。
async fn run_round(app: &AppHandle) {
    let (qoder_on, zcode_on) = auto_flags(app);
    if qoder_on {
        let names = account_names(app, true);
        let results = {
            let state = app.state::<QoderState>();
            credit_core::qoder::checkin::checkin_all(&state.core, &state.client).await
        };
        for (id, o) in &results {
            notify_account(
                app,
                if o.outcome.is_success() { "Qoder 签到成功" } else { "Qoder 签到失败" },
                &format!("{}：{}", name_of(&names, id), o.message),
            );
        }
    }
    if zcode_on {
        let names = account_names(app, false);
        let results = {
            let state = app.state::<ZcodeState>();
            credit_core::zcode::claim::claim_all(&state.core, &state.client).await
        };
        for (id, o) in &results {
            notify_account(
                app,
                if o.outcome.is_success() { "ZCode 领取成功" } else { "ZCode 领取失败" },
                &format!("{}：{}", name_of(&names, id), o.message),
            );
        }
    }
}

/// 轮次开始前的账号名快照(通知文案用;轮内删除账号不影响本轮提示)。
fn account_names(app: &AppHandle, qoder: bool) -> Vec<(String, String)> {
    if qoder {
        let state = app.state::<QoderState>();
        let data = state.core.data.lock().unwrap();
        data.get_accounts().iter().map(|a| (a.id.clone(), a.name.clone())).collect()
    } else {
        let state = app.state::<ZcodeState>();
        let data = state.core.data.lock().unwrap();
        data.get_accounts().iter().map(|a| (a.id.clone(), a.name.clone())).collect()
    }
}

fn name_of(names: &[(String, String)], id: &str) -> String {
    names
        .iter()
        .find(|(i, _)| i == id)
        .map(|(_, n)| n.clone())
        .unwrap_or_else(|| "账号".into())
}

/// 单账号完成的系统通知(成功/失败摘要)。
fn notify_account(app: &AppHandle, title: &str, body: &str) {
    let _ = app.notification().builder().title(title).body(body).show();
}
