// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
mod commands;
mod companion;
mod credit_scheduler;
#[cfg(target_os = "macos")]
mod instance_lock;
mod lingxi_commands;
#[cfg(desktop)]
mod tray;
mod qoder_commands;
mod trae_commands;
mod trae_scheduler;
mod update_service;
mod zcode_commands;

use std::time::Duration;
use tauri::{Emitter, Manager};
use wb_switch_core::modules;

const SCREENSHOT_DEMO_ENV: &str = "WB_SWITCH_SCREENSHOT_DEMO";

pub(crate) fn is_screenshot_demo() -> bool {
    std::env::var(SCREENSHOT_DEMO_ENV).as_deref() == Ok("1")
}

/// 轮换推迟提示：桌面端先向前端推 `rotate-deferred`（应用内提示，窗口开着就能看到），
/// 再尽力投递系统通知（应用在托盘/后台时可见）。
///
/// 应用内提示不依赖系统通知权限：插件在开发态会把通知登记到「终端」名下，且投递失败
/// 无法观测（`show()` 恒返回 Ok），所以两者都发、以前者为准。
/// 其它形态由 core 的日志与 `notify` 返回字段承载，宿主不投递。
pub(crate) fn deliver_rotate_notify(app: &tauri::AppHandle, result: &serde_json::Value) {
    #[cfg(desktop)]
    {
        if let Some(notify) = result.get("notify") {
            let _ = app.emit("rotate-deferred", notify.clone());
            tray::notify_rotate_deferred(app, notify);
        }
    }
    #[cfg(not(desktop))]
    {
        let _ = (app, result);
    }
}

/// 后台循环：自动签到启动即核验，之后按 core 计算的下一轮延迟睡眠（未设置
/// 签到时间段时固定 30 分钟）；自动轮换每 30 秒检查；每天一次保活；
/// 限额 hook 信号每秒轮询一次（入账即通知前端）；限额 hook 启动时后台默认接入。
fn spawn_background_loops(app: tauri::AppHandle) {
    let rotate_app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(error) = modules::config::compact_checkin_logs() {
            eprintln!("[签到] 历史日志整理失败: {error}");
        }
        let _ =
            modules::checkin::run_checkin_cycle(modules::checkin::CheckinCycleMode::StartupVerify)
                .await;
        loop {
            tokio::time::sleep(modules::checkin::next_cycle_delay()).await;
            let _ = modules::checkin::run_checkin_cycle(
                modules::checkin::CheckinCycleMode::PeriodicRecovery,
            )
            .await;
        }
    });

    // 派猫猫旅行：启动即派发，之后周期性补派（并重试 no-buddy / 瞬时错误）。
    // 档位过滤在 core（`travel_capable_accounts`）：不支持成长中心的档位不会发请求。
    tauri::async_runtime::spawn(async move {
        let _ = modules::travel::run_travel_cycle().await;
        loop {
            tokio::time::sleep(modules::travel::TRAVEL_RETRY_INTERVAL).await;
            let _ = modules::travel::run_travel_cycle().await;
        }
    });

    // 旅行领取：启动立刻查一轮（避免重启后空等 15 分钟漏领），之后按周期检查。
    tauri::async_runtime::spawn(async move {
        let _ = modules::travel::run_travel_claim_cycle().await;
        loop {
            tokio::time::sleep(modules::travel::TRAVEL_CLAIM_INTERVAL).await;
            let _ = modules::travel::run_travel_claim_cycle().await;
        }
    });

    tauri::async_runtime::spawn(async move {
        let mut last_keepalive_day = String::new();
        let mut last_rotate_at: i64 = 0;
        loop {
            // 自动轮换（CodeBuddy CLI）：按配置间隔执行
            let rotate_cfg = modules::config::load_auto_rotate_config();
            if rotate_cfg.get("enabled").and_then(|v| v.as_bool()) == Some(true) {
                let interval_minutes = rotate_cfg
                    .get("check_interval_minutes")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(5)
                    .max(1);
                let now = modules::config::now_ms();
                if now - last_rotate_at >= interval_minutes * 60_000 {
                    last_rotate_at = now;
                    let result = modules::rotate::run_rotate_cycle().await;
                    deliver_rotate_notify(&rotate_app, &result);
                }
            }
            let today = modules::checkin::date_str(None);
            if today != last_keepalive_day {
                last_keepalive_day = today;
                let _ = modules::refresh::run_keepalive_cycle().await;
            }
            tokio::time::sleep(Duration::from_secs(30)).await;
        }
    });

    // 统一更新服务：首次 15 秒后检查一次，之后每 30 分钟（未带 force，走 core 的
    // 6 小时缓存）。检查由 Rust 常驻，替代前端 30 分钟轮询：轻量模式 / 主窗口关闭时
    // 同样在跑，托盘菜单随时反映最新阶段。
    update_service::spawn_periodic_check(app.clone());

    // TRAE 定时签到：独立的每日定点循环（Asia/Shanghai，读 TraeState.settings）。
    // 与上方 workbuddy 签到周期（StartupVerify / PeriodicRecovery）语义不同，独立成环互不影响。
    // 未开启 auto_checkin 时内部直接返回，设置变更经 trae_save_settings 重启调度。
    trae_scheduler::start_scheduler(app.clone());

    // Qoder/ZCode/灵犀 信用平台：单循环覆盖三平台（Qoder/ZCode 2h±10min 抖动轮 +
    // 灵犀每日定点时间点）。与 workbuddy / TRAE 循环语义均不同，独立成环互不影响；
    // 全部平台都无调度需求时内部直接返回，设置变更经 qoder/zcode/lingxi_save_settings 重启调度。
    credit_scheduler::start_scheduler(app.clone());

    // 限额 hook 信号：轮询 `~/.wb-switch/hook-events.jsonl`（CLI / WorkBuddy 的 429 当轮
    // 由客户端 hook 追加），入账后通知前端立即拉取。轻量模式下窗口销毁但进程仍在，
    // 状态由后端持有（见 `rate_limit_events.rs`）。
    modules::rate_limit_events::spawn_watcher(move || {
        let _ = app.emit("rate-limits-updated", serde_json::json!({}));
    });

    // 默认接入：后台线程自动安装 hook（幂等、非阻塞、失败静默）。
    // 前置条件（开关开启 / 用户没卸载过 / 存在客户端 / 未装全）由 core 判定；
    // 装上了就作废扫描缓存——扫描范围从全量收窄到「未注册的来源」。
    std::thread::spawn(|| {
        if modules::rate_limit_hook::auto_install_on_startup() {
            modules::limits::invalidate_scan_cache();
        }
    });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let mut builder = tauri::Builder::default();

    // 单实例互斥必须最先注册：`Builder::build()` 按注册顺序 initialize_plugins，
    // 插件 setup 命中已有实例会直接 `std::process::exit(0)`，因此第二个进程在
    // 建主窗口 / 建托盘图标 / 起后台循环之前就已退出，不会产生账号侧副作用。
    #[cfg(desktop)]
    {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            tray::on_second_instance(app, args);
        }));
    }

    builder = builder
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init());

    #[cfg(desktop)]
    if !is_screenshot_demo() {
        builder = builder.plugin(agent_studio_desktop::init(companion::config()));
    }

    #[cfg(desktop)]
    {
        builder = builder.plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec![tray::SILENT_STARTUP_ARG]),
        ));
        builder = builder.on_window_event(tray::on_window_event);
    }

    let app = builder
        .setup(|app| {
            #[cfg(desktop)]
            {
                // 插件已在 initialize_plugins 阶段决定 notify-or-exit；此处只兜底
                // 插件漏掉的 macOS 竞态。必须在 tray::setup 之前：拿不到锁的第二
                // 实例不能先建出托盘图标。不得放到 run() 开头，否则会抢在插件
                // notify 之前拦下正常第二实例，丢掉「再点开 → 既有窗口弹出」。
                #[cfg(target_os = "macos")]
                instance_lock::acquire_or_exit(app.handle());
                tray::setup(app)?;
                // 主窗口由配置创建为不可见；在事件循环呈现前决定本次启动是否静默。
                // 仅系统自启（精确 `--hidden` 参数）进入静默托盘，普通启动立即显示主窗口。
                tray::setup_startup_visibility(
                    app.handle(),
                    tray::is_silent_startup(std::env::args()),
                );
            }
            // TRAE 子系统：数据根 ~/.wb-switch/trae/、状态、HTTP 客户端与调度器状态。
            // 与 workbuddy 主功能共用进程但状态完全独立（不同产品平台，零功能重叠）。
            let trae_dir = wb_switch_core::modules::config::trae_dir();
            let _ = std::fs::create_dir_all(&trae_dir);
            app.manage(trae_core::store::TraeState::new(trae_dir.clone()));
            app.manage(reqwest::Client::new());
            app.manage(trae_scheduler::TraeSchedulerState::default());
            // 旧 TraeMate 数据静默迁移（只拷贝不删除、幂等；失败仅日志，
            // 可由 trae_migrate_legacy_data 手动重跑并查看报告）。
            let config_dir = app.path().app_config_dir().ok();
            let report = trae_core::migrate::migrate_from_legacy(&trae_dir, config_dir.as_deref());
            if report.detected {
                println!(
                    "[trae] 旧版数据迁移完成: 账号 {} 条, 日志 {} 条, 文件 {} 个, 跳过 {} 项{}",
                    report.accounts_imported,
                    report.logs_imported,
                    report.files.len(),
                    report.skipped.len(),
                    if report.legacy_process_running {
                        "; 警告: 检测到旧 TraeMate 仍在运行,请退出并卸载旧版,避免双开竞争签到"
                    } else {
                        ""
                    }
                );
            }
            // Qoder/ZCode 信用平台子系统：数据根 ~/.wb-switch/{qoder,zcode}/（core 内自建目录），
            // 各持 core 存储状态 + 独立 HTTP 客户端；调度器状态含 generation 与下次执行时刻。
            // 与 workbuddy / TRAE 状态完全独立（不同产品平台，零功能重叠）。
            app.manage(qoder_commands::QoderState {
                core: credit_core::store::open_qoder_state(),
                client: reqwest::Client::new(),
            });
            app.manage(zcode_commands::ZcodeState {
                core: credit_core::store::open_zcode_state(),
                client: reqwest::Client::new(),
            });
            app.manage(lingxi_commands::LingxiState {
                core: credit_core::store::open_lingxi_state(),
                client: reqwest::Client::new(),
            });
            app.manage(credit_scheduler::CreditSchedulerState::default());
            // README 截图模式只渲染前端虚构数据，禁止读取账号后执行签到、轮换或保活。
            if !is_screenshot_demo() {
                spawn_background_loops(app.handle().clone());
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_status,
            commands::get_accounts,
            commands::get_codebuddy_cli_status,
            commands::install_codebuddy_cli_helper,
            commands::switch_codebuddy_cli_account,
            commands::get_codebuddy_cn_ide_status,
            commands::switch_codebuddy_cn_ide_account,
            commands::detect_codebuddy_cn_ide_account,
            commands::list_codebuddy_ide_sessions,
            commands::codebuddy_ide_session_links_preview,
            commands::get_vscode_ext_status,
            commands::switch_vscode_ext_account,
            commands::detect_vscode_ext_account,
            commands::list_vscode_sessions,
            commands::vscode_session_links_preview,
            commands::get_codebuddy_ide_status,
            commands::switch_codebuddy_ide_account,
            commands::list_codebuddy_intl_ide_sessions,
            commands::codebuddy_intl_ide_session_links_preview,
            commands::detect_codebuddy_ide_account,
            commands::get_jetbrains_status,
            commands::switch_jetbrains_account,
            commands::detect_jetbrains_account,
            commands::delete_account,
            commands::oauth_start,
            commands::oauth_status,
            commands::import_local,
            commands::export_accounts,
            commands::export_accounts_to_path,
            commands::preview_import_accounts,
            commands::import_accounts,
            commands::switch_account,
            commands::list_sessions,
            commands::copy_sessions,
            commands::session_links_preview,
            commands::open_permission_settings,
            commands::check_auth_permission,
            commands::reveal_app_in_finder,
            commands::get_checkin_status,
            commands::get_credit_expiry,
            commands::get_credit_statistics,
            commands::get_token_statistics,
            commands::get_rate_limits,
            commands::get_rate_limit_hook_status,
            commands::install_rate_limit_hook,
            commands::uninstall_rate_limit_hook,
            commands::get_rate_limit_config,
            commands::save_rate_limit_config,
            commands::checkin,
            commands::checkin_all,
            commands::get_auto_checkin_config,
            commands::save_auto_checkin_config,
            commands::get_checkin_logs,
            commands::get_travel_status,
            commands::get_auto_travel_config,
            commands::save_auto_travel_config,
            commands::refresh_account_token,
            commands::get_auto_rotate_config,
            commands::save_auto_rotate_config,
            commands::rotate_status,
            commands::run_rotate,
            commands::get_rotate_logs,
            commands::get_github_config,
            commands::save_github_config,
            commands::check_update,
            commands::update_state,
            commands::update_download,
            commands::update_restart,
            commands::relaunch_app,
            commands::get_launch_at_login_enabled,
            commands::set_launch_at_login_enabled,
            commands::record_notification,
            commands::list_notifications,
            commands::clear_notifications,
            commands::log_error,
            commands::get_error_log_path,
            commands::reveal_error_log,
            // TRAE 子系统（签到/多开/积分,见 trae_commands.rs;与主功能独立）
            trae_commands::trae_get_accounts,
            trae_commands::trae_import_desktop_account,
            trae_commands::trae_update_account,
            trae_commands::trae_delete_account,
            trae_commands::trae_checkin_account,
            trae_commands::trae_checkin_all,
            trae_commands::trae_get_account_points,
            trae_commands::trae_get_logs,
            trae_commands::trae_clear_logs,
            trae_commands::trae_get_settings,
            trae_commands::trae_save_settings,
            trae_commands::trae_start_scheduler,
            trae_commands::trae_stop_scheduler,
            trae_commands::trae_get_next_run_time,
            trae_commands::trae_launch_account_multi,
            trae_commands::trae_get_trae_exe_path,
            trae_commands::trae_set_trae_exe_path,
            trae_commands::trae_scan_trae_exe_path,
            trae_commands::trae_get_account_instance_state,
            trae_commands::trae_focus_account_instance,
            trae_commands::trae_open_new_login_instance,
            trae_commands::trae_scan_instance_dirs,
            trae_commands::trae_import_account_from_dir,
            trae_commands::trae_refresh_account_credential,
            trae_commands::trae_account_add_jwt,
            trae_commands::trae_jwt_parse_preview,
            trae_commands::trae_refresh_jwt_account,
            trae_commands::trae_cooldown_clear,
            trae_commands::trae_cooldown_clear_all,
            trae_commands::trae_device_reset,
            trae_commands::trae_fetch_remaining_credits,
            trae_commands::trae_refresh_all_remaining_credits,
            trae_commands::trae_credits_daily_list,
            trae_commands::trae_open_url,
            trae_commands::trae_migrate_legacy_data,
            // Qoder/ZCode 信用平台（签到/领取，见 qoder_commands.rs / zcode_commands.rs / credit_scheduler.rs）
            qoder_commands::qoder_get_accounts,
            qoder_commands::qoder_import_local,
            qoder_commands::qoder_add_account,
            qoder_commands::qoder_update_account,
            qoder_commands::qoder_delete_account,
            qoder_commands::qoder_checkin_account,
            qoder_commands::qoder_checkin_all,
            qoder_commands::qoder_get_account_quota,
            qoder_commands::qoder_get_logs,
            qoder_commands::qoder_clear_logs,
            qoder_commands::qoder_get_settings,
            qoder_commands::qoder_save_settings,
            qoder_commands::qoder_get_next_run_time,
            zcode_commands::zcode_get_accounts,
            zcode_commands::zcode_import_local,
            zcode_commands::zcode_add_account,
            zcode_commands::zcode_update_account,
            zcode_commands::zcode_delete_account,
            zcode_commands::zcode_claim_account,
            zcode_commands::zcode_claim_all,
            zcode_commands::zcode_get_account_quota,
            zcode_commands::zcode_get_logs,
            zcode_commands::zcode_clear_logs,
            zcode_commands::zcode_get_settings,
            zcode_commands::zcode_save_settings,
            zcode_commands::zcode_get_next_run_time,
            // 灵犀多用户签到（POST checkinUrl + Cookie,见 lingxi_commands.rs / credit_scheduler.rs）
            lingxi_commands::lingxi_get_accounts,
            lingxi_commands::lingxi_add_account,
            lingxi_commands::lingxi_update_account,
            lingxi_commands::lingxi_delete_account,
            lingxi_commands::lingxi_import_local,
            lingxi_commands::lingxi_checkin_account,
            lingxi_commands::lingxi_checkin_all,
            lingxi_commands::lingxi_get_logs,
            lingxi_commands::lingxi_clear_logs,
            lingxi_commands::lingxi_get_settings,
            lingxi_commands::lingxi_save_settings,
            lingxi_commands::lingxi_get_next_run_time,
            companion::get_companion_enabled,
            companion::set_companion_enabled,
            companion::open_companion_settings,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|_app_handle, event| {
        #[cfg(desktop)]
        {
            // 点击 Dock / Finder 再次激活已运行实例：窗口已隐藏到托盘时显示主窗口。
            // `Reopen` 在主线程派发，可直接调用窗口路径。
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen {
                has_visible_windows: false,
                ..
            } = &event
            {
                tray::show_main_window_on_reopen(_app_handle);
            }
            tray::on_run_event(event);
        }
    });
}
