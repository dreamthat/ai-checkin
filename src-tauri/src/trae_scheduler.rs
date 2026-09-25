// TRAE 定时签到调度器(自 trae-mate scheduler.rs 平移,含 AppHandle 的宿主部分)。
// generation 计数控制任务生命周期(无需 abort);tokio 分段 sleep 15s 及时响应 stop。
// 时区 Asia/Shanghai(纯函数在 trae-core schedule.rs)。
// 注意:与 workbuddy 的签到周期(checkin cycle,周期恢复语义)完全独立,勿合并为一个循环。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use tauri::{AppHandle, Manager};
use tauri_plugin_notification::NotificationExt;

use trae_core::checkin::perform_all_checkin;
use trae_core::schedule::next_run_instant;
use trae_core::store::TraeState;

use crate::trae_commands::AppNotifier;

pub struct TraeSchedulerState {
    pub generation: Arc<AtomicU64>,
}

impl Default for TraeSchedulerState {
    fn default() -> Self {
        TraeSchedulerState {
            generation: Arc::new(AtomicU64::new(0)),
        }
    }
}

/// 启动定时任务(若已开启自动签到)。每次调用增 generation,旧任务自行退出。
pub fn start_scheduler(app: AppHandle) {
    let gen = app.state::<TraeSchedulerState>().inner().generation.clone();
    let my_gen = gen.fetch_add(1, Ordering::SeqCst) + 1;

    let settings = {
        let state = app.state::<TraeState>();
        let data = state.data.lock().unwrap();
        data.get_settings()
    };
    if !settings.auto_checkin {
        return;
    }

    let app_clone = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            if !is_current(&app_clone, my_gen) {
                break;
            }
            let dur = {
                let settings = {
                    let state = app_clone.state::<TraeState>();
                    let data = state.data.lock().unwrap();
                    data.get_settings()
                };
                match trae_core::schedule::next_run_duration(&settings) {
                    Some(d) => d,
                    None => break,
                }
            };
            // 分段 sleep,每 15s 检查 generation,及时响应 stop/restart
            let mut remaining = dur;
            while remaining > std::time::Duration::ZERO {
                if !is_current(&app_clone, my_gen) {
                    break;
                }
                let step = remaining.min(std::time::Duration::from_secs(15));
                tokio::time::sleep(step).await;
                remaining = remaining.saturating_sub(step);
            }
            if !is_current(&app_clone, my_gen) {
                break;
            }
            run_auto_checkin(app_clone.clone()).await;
        }
    });
}

/// 停止定时任务(增 generation 使当前任务退出)
pub fn stop_scheduler(app: &AppHandle) {
    app.state::<TraeSchedulerState>()
        .inner()
        .generation
        .fetch_add(1, Ordering::SeqCst);
}

/// 下次执行时间(ISO8601),未启用返回 None
pub fn get_next_run_time(app: &AppHandle) -> Option<String> {
    let settings = {
        let state = app.state::<TraeState>();
        let data = state.data.lock().unwrap();
        data.get_settings()
    };
    if !settings.auto_checkin {
        return None;
    }
    next_run_instant(&settings).map(|dt| dt.to_rfc3339())
}

fn is_current(app: &AppHandle, gen: u64) -> bool {
    app.state::<TraeSchedulerState>()
        .inner()
        .generation
        .load(Ordering::SeqCst)
        == gen
}

async fn run_auto_checkin(app: AppHandle) {
    let notifier = AppNotifier(app.clone());
    let settings = {
        let state = app.state::<TraeState>();
        let data = state.data.lock().unwrap();
        data.get_settings()
    };

    // 自动签到账号间冷却间隔(分钟),最短 3 分钟
    let interval_secs = (settings.auto_checkin_interval_min.max(3) as u64) * 60;
    let client = app.state::<reqwest::Client>();
    let state = app.state::<TraeState>();
    let results = perform_all_checkin(
        Some(&notifier),
        client.inner(),
        state.inner(),
        interval_secs,
    )
    .await;
    let success = results.iter().filter(|(_, r)| r.success).count();
    let failed = results.len() - success;

    if settings.notify_on_success && success > 0 {
        let body = format!(
            "成功签到 {} 个账号{}",
            success,
            if failed > 0 {
                format!("，失败 {} 个", failed)
            } else {
                String::new()
            }
        );
        let _ = app
            .notification()
            .builder()
            .title("Trae 签到成功")
            .body(body)
            .show();
    }
    if settings.notify_on_failed && failed > 0 {
        let _ = app
            .notification()
            .builder()
            .title("Trae 签到失败")
            .body(format!("{} 个账号签到失败，请查看日志", failed))
            .show();
    }
}
