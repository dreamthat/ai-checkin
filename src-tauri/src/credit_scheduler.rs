// Qoder/ZCode/灵犀 信用平台定时调度器(方案见 .trae/documents/merge-creditdaddy-qoder-zcode.md)。
// 单一循环覆盖三平台:Qoder checkin_all + ZCode claim_all 按 2h±10min 抖动轮执行
// (抖动纯函数在 credit-core schedule.rs);灵犀按每日定点时间点(Asia/Shanghai)执行,
// 在分段 sleep 的 15s 粒度里检查到点即跑——三平台同环,不另开循环。
// 每账号完成发系统通知(成功/失败摘要,灵犀按其通知开关)。
// generation 计数控制任务生命周期(同 trae_scheduler,无需 abort);tokio 分段 sleep 15s
// 及时响应 stop/restart;下次执行时刻记入 Mutex<Option<DateTime>> 供 get_next_run_time 查询。
// 注意:与 workbuddy 签到周期、TRAE 每日定点循环语义均不同,独立成环互不影响。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Datelike, TimeZone, Utc};
use tauri::{AppHandle, Manager};
use tauri_plugin_notification::NotificationExt;

use credit_core::schedule::{next_round_delay_default, parse_hhmm_list, shanghai_today, shanghai_tz};

use crate::lingxi_commands::LingxiState;
use crate::qoder_commands::QoderState;
use crate::zcode_commands::ZcodeState;

pub struct CreditSchedulerState {
    pub generation: Arc<AtomicU64>,
    /// 下次执行时刻(睡眠前记录、执行时清空;qoder/zcode_get_next_run_time 查询用)
    pub next_run_at: Mutex<Option<DateTime<Utc>>>,
    /// 灵犀今日已执行的时间点标记("YYYY-MM-DD|HH:MM"),防同一天重复执行
    pub lingxi_fired: Mutex<Vec<String>>,
}

impl Default for CreditSchedulerState {
    fn default() -> Self {
        CreditSchedulerState {
            generation: Arc::new(AtomicU64::new(0)),
            next_run_at: Mutex::new(None),
            lingxi_fired: Mutex::new(Vec::new()),
        }
    }
}

/// 启动定时任务(三平台共用一个循环)。每次调用增 generation,旧任务自行退出;
/// 所有平台都无调度需求时不启动(增 generation 已等效停止旧任务)。
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
            // 分段 sleep,每 15s 检查 generation,及时响应 stop/restart;
            // 顺带做灵犀每日定点检查(到点即执行一轮,三平台同环不另开循环)
            let mut remaining = dur;
            while remaining > Duration::ZERO {
                if !is_current(&app_clone, my_gen) {
                    break;
                }
                check_lingxi(&app_clone).await;
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

/// 任一平台有调度需求?(Qoder/ZCode 设置任一 auto_claim_enabled,或灵犀配置了有效时间点)
pub fn any_auto_enabled(app: &AppHandle) -> bool {
    let (qoder_on, zcode_on) = auto_flags(app);
    qoder_on || zcode_on || lingxi_enabled(app)
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

// ===== 灵犀每日定点(同环检查,见 start_scheduler 分段 sleep) =====

/// 灵犀是否参与调度:配置了有效签到时间点即参与(每日定点模型,无独立开关)。
fn lingxi_enabled(app: &AppHandle) -> bool {
    let state = app.state::<LingxiState>();
    let data = state.core.data.lock().unwrap();
    let settings = data.get_settings();
    !parse_hhmm_list(settings.checkin_times.iter().map(String::as_str)).is_empty()
}

/// 灵犀到点检查:存在"已到点且今日未执行过"的时间点 → 执行一轮。
/// fired 标记记 "YYYY-MM-DD|HH:MM",换日自动失效;启动补跑(错过的时间点立即执行)
/// 依赖 core 的 per-account lastSuccessDate 幂等,重复执行无害。
async fn check_lingxi(app: &AppHandle) {
    let times = {
        let state = app.state::<LingxiState>();
        let data = state.core.data.lock().unwrap();
        let settings = data.get_settings();
        settings.checkin_times
    };
    let now = Utc::now();
    let today = shanghai_today(now);
    let tz = shanghai_tz();
    let today_cn = now.with_timezone(&tz).date_naive();
    let mut due: Vec<String> = Vec::new();
    for (h, m) in parse_hhmm_list(times.iter().map(String::as_str)) {
        let at = tz.with_ymd_and_hms(today_cn.year(), today_cn.month(), today_cn.day(), h, m, 0);
        if let Some(at) = at.single() {
            if at.with_timezone(&Utc) <= now {
                due.push(format!("{today}|{h:02}:{m:02}"));
            }
        }
    }
    if due.is_empty() {
        return;
    }
    {
        let mut fired = app
            .state::<CreditSchedulerState>()
            .inner()
            .lingxi_fired
            .lock()
            .unwrap();
        fired.retain(|k| k.starts_with(&format!("{today}|")));
        let new_keys: Vec<String> = due.iter().filter(|k| !fired.contains(*k)).cloned().collect();
        if new_keys.is_empty() {
            return;
        }
        fired.extend(new_keys);
    }
    run_lingxi_round(app).await;
}

/// 一轮灵犀签到:全部启用账号(当日已成功在 core 跳过),按设置发系统通知
/// (unknown 不通知,下个时间点重试;skipped 无需打扰)。
async fn run_lingxi_round(app: &AppHandle) {
    let (names, notify_on_success, notify_on_failed) = {
        let state = app.state::<LingxiState>();
        let data = state.core.data.lock().unwrap();
        let names: Vec<(String, String)> =
            data.get_accounts().iter().map(|a| (a.id.clone(), a.name.clone())).collect();
        let s = data.get_settings();
        (names, s.notify_on_success, s.notify_on_failed)
    };
    let today = shanghai_today(Utc::now());
    let results = {
        let state = app.state::<LingxiState>();
        credit_core::lingxi::checkin::checkin_all(&state.core, &state.client, &today).await
    };
    for (id, o) in &results {
        use credit_core::lingxi::Outcome;
        let title = match o.outcome {
            Outcome::Success | Outcome::Already => notify_on_success.then_some("灵犀签到成功"),
            Outcome::Failed => notify_on_failed.then_some("灵犀签到失败"),
            Outcome::Unknown | Outcome::Skipped => None,
        };
        if let Some(title) = title {
            notify_account(app, title, &format!("{}：{}", name_of(&names, id), o.message));
        }
    }
}
