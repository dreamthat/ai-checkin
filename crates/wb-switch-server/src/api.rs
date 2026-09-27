//! HTTP API 层：把 wb-switch-core 暴露为本地 REST 接口，供 webui（浏览器）调用。
//!
//! 路由设计对应 Python 版 server.py 与桌面端 commands.rs。仅绑定 127.0.0.1，
//! token 不出本机。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
#[cfg(target_os = "windows")]
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::{Query, RawQuery};
use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use rust_embed::RustEmbed;
use serde_json::{json, Value};

use chrono::{Datelike, TimeZone};

use credit_core::log::LogStore;
use credit_core::store::{LingxiState, QoderState, ZcodeState, open_lingxi_state, open_qoder_state, open_zcode_state};
use trae_core::store::TraeState;
use wb_switch_core::modules::{
    account, auth_file, checkin, codebuddy_cli, codebuddy_cn_ide, codebuddy_ide, config,
    credit_usage, credits, export_import, limits, notifications, oauth, process, rate_limit_events,
    rate_limit_hook, refresh, rotate, session, switch, token_stats, travel, update,
    variant::WbVariant, vscode_ext, vscode_session, vscode_session_sync,
};

/// WorkBuddy 运行状态缓存：Windows 上检测要跑 tasklist（慢），缓存几秒避免
/// 前端切 tab 频繁触发命令行导致卡顿/闪窗。按档位分别缓存。
#[cfg(target_os = "windows")]
static RUNNING_CACHE: Mutex<Option<(Instant, bool, WbVariant)>> = Mutex::new(None);

fn cached_workbuddy_running(variant: WbVariant) -> bool {
    #[cfg(target_os = "windows")]
    {
        let mut cache = RUNNING_CACHE.lock().unwrap();
        if let Some((t, v, cached_variant)) = cache.as_ref() {
            if *cached_variant == variant && t.elapsed() < Duration::from_secs(3) {
                return *v;
            }
        }
        let v = process::is_workbuddy_running(variant);
        *cache = Some((Instant::now(), v, variant));
        v
    }
    #[cfg(not(target_os = "windows"))]
    {
        process::is_workbuddy_running(variant)
    }
}

#[derive(RustEmbed)]
#[folder = "../../dist/"]
struct Assets;

/// 切换进度缓存：webui 通过 GET /api/switch/progress 轮询。
static SWITCH_PROGRESS: Mutex<Option<String>> = Mutex::new(None);
static SWITCH_RUNNING: Mutex<bool> = Mutex::new(false);

// ---------------------------------------------------------------------------
// TRAE 子系统(自 trae-mate 合并;数据根 ~/.wb-switch/trae/)
// ---------------------------------------------------------------------------

/// server 形态没有 Tauri 的 app_config_dir:TRAE exe 路径等宿主配置统一存放在
/// trae_dir() 下(桌面端存应用配置目录,两形态互不影响)。
static TRAE_STATE: OnceLock<TraeState> = OnceLock::new();
static TRAE_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
static TRAE_SCHED_GEN: AtomicU64 = AtomicU64::new(0);

pub fn trae_state() -> &'static TraeState {
    TRAE_STATE.get_or_init(|| TraeState::new(config::trae_dir()))
}

fn trae_client() -> &'static reqwest::Client {
    TRAE_CLIENT.get_or_init(reqwest::Client::new)
}

/// server 启动时初始化 TRAE 子系统:确保数据目录、静默迁移旧 TraeMate 数据、
/// 按设置启动定时签到循环(由 main.rs spawn_background_loops 调用)。
pub fn init_trae() {
    let dir = config::trae_dir();
    let _ = std::fs::create_dir_all(&dir);
    // server 无应用配置目录:exe 路径迁移目标传 None(仅迁移账号/日志数据)
    let report = trae_core::migrate::migrate_from_legacy(&dir, None);
    if report.detected {
        println!(
            "[TRAE] 已迁移旧 TraeMate 数据: 账号 {}, 日志 {}(跳过 {} 项: {:?})",
            report.accounts_imported,
            report.logs_imported,
            report.skipped.len(),
            report.skipped
        );
    }
    trae_start_scheduler();
}

/// TRAE 定时签到调度(server 形态):与桌面端 trae_scheduler 同语义——
/// generation 计数控制任务生命周期(旧任务自行退出),分段 sleep 15s 及时响应
/// stop/restart;无系统通知能力,结果降级为 stdout 日志。
fn trae_start_scheduler() -> bool {
    let gen = TRAE_SCHED_GEN.fetch_add(1, Ordering::SeqCst) + 1;
    let settings = trae_state().data.lock().unwrap().get_settings();
    if !settings.auto_checkin {
        return true;
    }
    let state = trae_state();
    let client = trae_client();
    tokio::spawn(async move {
        loop {
            if TRAE_SCHED_GEN.load(Ordering::SeqCst) != gen {
                break;
            }
            let dur = {
                let settings = state.data.lock().unwrap().get_settings();
                match trae_core::schedule::next_run_duration(&settings) {
                    Some(d) => d,
                    None => break,
                }
            };
            // 分段 sleep,每 15s 检查 generation,及时响应 stop/restart
            let mut remaining = dur;
            while remaining > std::time::Duration::ZERO {
                if TRAE_SCHED_GEN.load(Ordering::SeqCst) != gen {
                    break;
                }
                let step = remaining.min(std::time::Duration::from_secs(15));
                tokio::time::sleep(step).await;
                remaining = remaining.saturating_sub(step);
            }
            if TRAE_SCHED_GEN.load(Ordering::SeqCst) != gen {
                break;
            }
            trae_run_auto_checkin(state, client).await;
        }
    });
    true
}

/// TRAE 自动签到执行体(无通知者,结果打日志)。
async fn trae_run_auto_checkin(state: &TraeState, client: &reqwest::Client) {
    let settings = state.data.lock().unwrap().get_settings();
    // 自动签到账号间冷却间隔(分钟),最短 3 分钟(与桌面端一致)
    let interval_secs = (settings.auto_checkin_interval_min.max(3) as u64) * 60;
    let results =
        trae_core::checkin::perform_all_checkin(None, client, state, interval_secs).await;
    let success = results.iter().filter(|(_, r)| r.success).count();
    let failed = results.len() - success;
    println!("[TRAE] 自动签到完成: 成功 {success}, 失败 {failed}");
}

/// TRAE 命令结果序列化:AppResult<T> 成功回 JSON 值,失败回 {ok:false,error}
/// (与前端 api.ts 双通道错误处理约定一致)。
fn trae_json<T: serde::Serialize>(r: Result<T, trae_core::error::AppError>) -> Response {
    match r {
        Ok(v) => json_ok(serde_json::to_value(v).unwrap_or(json!(null))),
        Err(e) => json_err(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}

fn body_str(body: &Value, key: &str) -> Option<String> {
    body.get(key).and_then(Value::as_str).map(String::from)
}

pub fn router() -> Router {
    Router::new()
        .route("/api/status", get(api_status))
        .route("/api/accounts", get(api_accounts))
        .route("/api/codebuddy-cli/status", get(api_codebuddy_cli_status))
        .route(
            "/api/codebuddy-cli/install-helper",
            post(api_codebuddy_cli_install_helper),
        )
        .route("/api/codebuddy-cli/switch", post(api_codebuddy_cli_switch))
        .route(
            "/api/codebuddy-cn-ide/status",
            get(api_codebuddy_cn_ide_status),
        )
        .route(
            "/api/codebuddy-cn-ide/switch",
            post(api_codebuddy_cn_ide_switch),
        )
        .route(
            "/api/codebuddy-cn-ide/detect",
            post(api_codebuddy_cn_ide_detect),
        )
        .route("/api/codebuddy-ide/status", get(api_codebuddy_ide_status))
        .route("/api/codebuddy-ide/switch", post(api_codebuddy_ide_switch))
        .route("/api/codebuddy-ide/detect", post(api_codebuddy_ide_detect))
        .route("/api/vscode-ext/status", get(api_vscode_ext_status))
        .route("/api/vscode-ext/sessions", get(api_vscode_ext_sessions))
        .route("/api/vscode-ext/switch", post(api_vscode_ext_switch))
        .route("/api/vscode-ext/detect", post(api_vscode_ext_detect))
        .route(
            "/api/vscode-ext/session-links",
            post(api_vscode_ext_session_links_preview),
        )
        .route("/api/delete", post(api_delete))
        .route("/api/oauth/start", post(api_oauth_start))
        .route("/api/oauth/status", post(api_oauth_status))
        .route("/api/import-local", post(api_import_local))
        .route("/api/export-accounts", post(api_export_accounts))
        .route(
            "/api/export-accounts-to-path",
            post(api_export_accounts_to_path),
        )
        .route("/api/import/preview", post(api_preview_import))
        .route("/api/import", post(api_import))
        .route("/api/switch", post(api_switch))
        .route("/api/switch/progress", get(api_switch_progress))
        .route("/api/sessions", get(api_sessions))
        .route("/api/sessions/copy", post(api_copy_sessions))
        .route(
            "/api/session-links/preview",
            post(api_session_links_preview),
        )
        .route("/api/checkin/status", get(api_checkin_status))
        .route("/api/credits", post(api_credits))
        .route("/api/credits/stats", get(api_credit_statistics))
        .route("/api/token-stats", get(api_token_statistics))
        .route("/api/rate-limits", get(api_rate_limits))
        .route(
            "/api/rate-limits/hook-status",
            get(api_rate_limit_hook_status),
        )
        .route(
            "/api/rate-limits/install-hook",
            post(api_install_rate_limit_hook),
        )
        .route(
            "/api/rate-limits/uninstall-hook",
            post(api_uninstall_rate_limit_hook),
        )
        .route(
            "/api/rate-limits/config",
            get(api_rate_limit_config).post(api_save_rate_limit_config),
        )
        .route("/api/checkin", post(api_checkin))
        .route("/api/checkin/all", post(api_checkin_all))
        .route(
            "/api/checkin/config",
            get(api_checkin_config).post(api_save_checkin_config),
        )
        .route("/api/checkin/logs", get(api_checkin_logs))
        .route("/api/notifications", get(api_notifications))
        .route("/api/notifications/record", post(api_record_notification))
        .route("/api/notifications/clear", post(api_clear_notifications))
        .route("/api/travel/status", get(api_travel_status))
        .route(
            "/api/travel/config",
            get(api_travel_config).post(api_save_travel_config),
        )
        .route(
            "/api/rotate/config",
            get(api_rotate_config).post(api_save_rotate_config),
        )
        .route("/api/rotate/status", get(api_rotate_status))
        .route("/api/rotate/run", post(api_rotate_run))
        .route("/api/rotate/logs", get(api_rotate_logs))
        .route("/api/refresh-token", post(api_refresh_token))
        .route("/api/update/check", get(api_update_check))
        .route(
            "/api/update/config",
            get(api_update_config).post(api_save_update_config),
        )
        // ---- TRAE(仅 Windows 完整可用;非 Windows 由 trae-core 返回友好错误)----
        .route("/api/trae/accounts", get(api_trae_accounts))
        .route("/api/trae/accounts/import-desktop", post(api_trae_import_desktop))
        .route("/api/trae/accounts/update", post(api_trae_update_account))
        .route("/api/trae/accounts/delete", post(api_trae_delete_account))
        .route("/api/trae/accounts/add-jwt", post(api_trae_add_jwt))
        .route("/api/trae/checkin", post(api_trae_checkin))
        .route("/api/trae/checkin-all", post(api_trae_checkin_all))
        .route("/api/trae/points", post(api_trae_points))
        .route("/api/trae/logs", get(api_trae_logs))
        .route("/api/trae/logs/clear", post(api_trae_clear_logs))
        .route(
            "/api/trae/settings",
            get(api_trae_get_settings).post(api_trae_save_settings),
        )
        .route("/api/trae/scheduler/start", post(api_trae_scheduler_start))
        .route("/api/trae/scheduler/stop", post(api_trae_scheduler_stop))
        .route("/api/trae/next-run", get(api_trae_next_run))
        .route("/api/trae/launch", post(api_trae_launch))
        .route(
            "/api/trae/exe-path",
            get(api_trae_get_exe_path).post(api_trae_set_exe_path),
        )
        .route("/api/trae/exe-path/scan", post(api_trae_scan_exe_path))
        .route("/api/trae/instance-state", post(api_trae_instance_state))
        .route("/api/trae/focus", post(api_trae_focus))
        .route("/api/trae/login-instance", post(api_trae_open_login_instance))
        .route("/api/trae/instance-dirs", get(api_trae_instance_dirs))
        .route("/api/trae/import-dir", post(api_trae_import_dir))
        .route("/api/trae/refresh-credential", post(api_trae_refresh_credential))
        .route("/api/trae/jwt-preview", post(api_trae_jwt_preview))
        .route("/api/trae/refresh-jwt", post(api_trae_refresh_jwt))
        .route("/api/trae/cooldown/clear", post(api_trae_cooldown_clear))
        .route("/api/trae/cooldown/clear-all", post(api_trae_cooldown_clear_all))
        .route("/api/trae/device/reset", post(api_trae_device_reset))
        .route("/api/trae/credits/fetch", post(api_trae_credits_fetch))
        .route("/api/trae/credits/refresh-all", post(api_trae_credits_refresh_all))
        .route("/api/trae/credits/daily", get(api_trae_credits_daily))
        .route("/api/trae/open-url", post(api_trae_open_url))
        .route("/api/trae/migrate", post(api_trae_migrate))
        // ---- Qoder(信用平台:活动领取制签到;核心逻辑在 credit-core)----
        .route("/api/qoder/accounts", get(api_qoder_accounts))
        .route("/api/qoder/accounts/import-local", post(api_qoder_import_local))
        .route("/api/qoder/accounts/add", post(api_qoder_add_account))
        .route("/api/qoder/accounts/update", post(api_qoder_update_account))
        .route("/api/qoder/accounts/delete", post(api_qoder_delete_account))
        .route("/api/qoder/checkin", post(api_qoder_checkin))
        .route("/api/qoder/checkin-all", post(api_qoder_checkin_all))
        .route("/api/qoder/quota", post(api_qoder_quota))
        .route("/api/qoder/logs", get(api_qoder_logs))
        .route("/api/qoder/logs/clear", post(api_qoder_clear_logs))
        .route(
            "/api/qoder/settings",
            get(api_qoder_get_settings).post(api_qoder_save_settings),
        )
        .route("/api/qoder/next-run", get(api_qoder_next_run))
        // ---- ZCode(信用平台:无签到,套餐/活动领取)----
        .route("/api/zcode/accounts", get(api_zcode_accounts))
        .route("/api/zcode/accounts/import-local", post(api_zcode_import_local))
        .route("/api/zcode/accounts/add", post(api_zcode_add_account))
        .route("/api/zcode/accounts/update", post(api_zcode_update_account))
        .route("/api/zcode/accounts/delete", post(api_zcode_delete_account))
        .route("/api/zcode/claim", post(api_zcode_claim))
        .route("/api/zcode/claim-all", post(api_zcode_claim_all))
        .route("/api/zcode/quota", post(api_zcode_quota))
        .route("/api/zcode/logs", get(api_zcode_logs))
        .route("/api/zcode/logs/clear", post(api_zcode_clear_logs))
        .route(
            "/api/zcode/settings",
            get(api_zcode_get_settings).post(api_zcode_save_settings),
        )
        .route("/api/zcode/next-run", get(api_zcode_next_run))

        // ---- 灵犀(多用户签到:POST checkinUrl + Cookie;核心逻辑在 credit-core)----
        .route("/api/lingxi/accounts", get(api_lingxi_accounts))
        .route("/api/lingxi/accounts/add", post(api_lingxi_add_account))
        .route("/api/lingxi/accounts/import-local", post(api_lingxi_import_local))
        .route("/api/lingxi/accounts/update", post(api_lingxi_update_account))
        .route("/api/lingxi/accounts/delete", post(api_lingxi_delete_account))
        .route("/api/lingxi/checkin", post(api_lingxi_checkin))
        .route("/api/lingxi/checkin-all", post(api_lingxi_checkin_all))
        .route("/api/lingxi/logs", get(api_lingxi_logs))
        .route("/api/lingxi/logs/clear", post(api_lingxi_clear_logs))
        .route(
            "/api/lingxi/settings",
            get(api_lingxi_get_settings).post(api_lingxi_save_settings),
        )
        .route("/api/lingxi/next-run", get(api_lingxi_next_run))
        .fallback(static_handler)
}

fn json_ok(v: Value) -> Response {
    Json(v).into_response()
}

fn json_err(e: String, code: StatusCode) -> Response {
    (code, Json(json!({ "ok": false, "error": e }))).into_response()
}

/// 从 query string 解析档位（缺省国内版）。与 Tauri 命令的可选 `variant` 参数同义。
fn query_variant(query: Option<&str>) -> WbVariant {
    let raw = query.unwrap_or("").split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        (key == "variant").then_some(value)
    });
    WbVariant::parse(raw)
}

/// 从请求体解析档位（缺省国内版）。与 Tauri 命令的可选 `variant` 参数同义。
fn body_variant(body: &Value) -> WbVariant {
    WbVariant::parse(body.get("variant").and_then(Value::as_str))
}

// ---------------------------------------------------------------------------
// 状态 / 账号
// ---------------------------------------------------------------------------

async fn api_status(RawQuery(query): RawQuery) -> Response {
    let variant = query_variant(query.as_deref());
    let auth = auth_file::read_auth_file(variant);
    let current = auth.as_ref().map(|a| {
        let acct = a.get("account").cloned().unwrap_or_else(|| json!({}));
        json!({
            "uid": account::display_value(&acct, "uid"),
            "nickname": account::display_value(&acct, "nickname"),
            "email": account::display_value(&acct, "email"),
        })
    });
    json_ok(json!({
        "running": cached_workbuddy_running(variant),
        "authFile": auth_file::auth_file_path(variant).to_string_lossy(),
        "current": current,
        "appPath": auth_file::workbuddy_app_path(variant).to_string_lossy(),
        "version": update::APP_VERSION,
        "variant": variant.as_str(),
    }))
}

/// GET /api/accounts —— 返回全部档位的账号，`current` 取请求档位的登录态。
async fn api_accounts(RawQuery(query): RawQuery) -> Response {
    let variant = query_variant(query.as_deref());
    json_ok(json!({
        "accounts": account::load_accounts()
            .iter()
            .map(account::account_meta)
            .collect::<Vec<_>>(),
        "current": auth_file::read_auth_file(variant)
            .and_then(|a| a.get("account").and_then(|x| x.get("uid")).and_then(|x| x.as_str()).map(String::from)),
        "variant": variant.as_str(),
    }))
}

async fn api_codebuddy_cli_status() -> Response {
    json_ok(codebuddy_cli::status())
}

async fn api_codebuddy_cli_install_helper() -> Response {
    match codebuddy_cli::install_helper() {
        Ok(result) => json_ok(result),
        Err(error) => json_err(error, StatusCode::BAD_REQUEST),
    }
}

async fn api_codebuddy_cli_switch(Json(body): Json<Value>) -> Response {
    let id = body.get("accountId").and_then(|v| v.as_str()).unwrap_or("");
    // 入参 `closeRunningCli` 已废弃：后端一律先关闭正在运行的 CLI 再写状态，忽略该值。
    // 无头模式不投递系统通知，切号结果（含关闭数量）照常返回给调用方。
    match codebuddy_cli::switch_active_account(id) {
        Ok(result) => json_ok(result),
        Err(error) => json_err(error, StatusCode::BAD_REQUEST),
    }
}

async fn api_codebuddy_cn_ide_status() -> Response {
    json_ok(codebuddy_cn_ide::status())
}

async fn api_codebuddy_cn_ide_switch(Json(body): Json<Value>) -> Response {
    let account_id = body
        .get("accountId")
        .or_else(|| body.get("account_id"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let restart = body
        .get("restart")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    match codebuddy_cn_ide::switch_account(account_id, restart) {
        Ok(v) => json_ok(v),
        Err(e) => json_err(e, StatusCode::BAD_REQUEST),
    }
}

async fn api_codebuddy_cn_ide_detect() -> Response {
    match codebuddy_cn_ide::detect_current_account() {
        Ok(v) => json_ok(v),
        Err(e) => json_err(e, StatusCode::BAD_REQUEST),
    }
}

async fn api_vscode_ext_status() -> Response {
    json_ok(vscode_ext::status())
}

/// GET /api/vscode-ext/sessions —— 当前 VS Code 扩展账号可复制的会话（未登录返回空列表）。
async fn api_vscode_ext_sessions() -> Response {
    let result = tokio::task::spawn_blocking(|| match vscode_ext::active_ext_uid() {
        Some(uid) => vscode_session::list_vscode_sessions(&uid),
        None => json!({ "sourceUid": null, "sessions": [], "skipped": 0 }),
    })
    .await;
    match result {
        Ok(value) => json_ok(value),
        Err(error) => json_err(error.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}

async fn api_vscode_ext_switch(Json(body): Json<Value>) -> Response {
    let account_id = body
        .get("accountId")
        .or_else(|| body.get("account_id"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    // 默认重启（= 自动关闭并重开）：VS Code 运行时由后端先优雅退出再写入。
    // 显式传 restart=false 时退回「请先完全退出 VS Code」的手动模式。
    let restart = body
        .get("restart")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    // 可选：切换前把勾选会话复制到目标账号（与 /api/vscode-ext/* 命名风格一致）。
    // 任一条目非法即整包拒绝（与 Tauri 侧 `Option<Vec<CopyItem>>` 的 serde 整包报错同形），
    // 避免「部分成功 + 静默丢弃」让用户误以为全部复制成功。
    let copy_items: Vec<vscode_session::CopyItem> = match body
        .get("copySessions")
        .and_then(|v| v.as_array())
        .map(|array| {
            array
                .iter()
                .map(|item| serde_json::from_value::<vscode_session::CopyItem>(item.clone()))
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()
    {
        Ok(items) => items.unwrap_or_default(),
        Err(error) => {
            return json_err(
                format!("copySessions 条目非法：{error}"),
                StatusCode::BAD_REQUEST,
            )
        }
    };

    // 同步选择与桌面端同形（[{groupId, previewToken, mode}]），形状由 core 校验。
    let sync_selections = match session::parse_sync_selections(body.get("syncSelections")) {
        Ok(selections) => selections,
        Err(error) => return json_err(error, StatusCode::BAD_REQUEST),
    };

    let result = if copy_items.is_empty() && sync_selections.is_empty() {
        vscode_ext::switch_account(account_id, restart)
    } else {
        vscode_session::switch_vscode_ext_with_copy(
            account_id,
            restart,
            &copy_items,
            &sync_selections,
        )
    };
    match result {
        Ok(v) => json_ok(v),
        Err(e) => json_err(e, StatusCode::BAD_REQUEST),
    }
}

/// POST /api/vscode-ext/session-links —— 预览当前插件账号 → 目标账号的关联会话同步项。
///
/// 与桌面端 `vscode_session_links_preview` 同形：直接返回 core 的只读预览
/// （`supported` / `storeStatus` / `groups`），每组的 `defaultChecked` 与 `availableModes`
/// 是前端的勾选权限来源。
async fn api_vscode_ext_session_links_preview(Json(body): Json<Value>) -> Response {
    let target_account_id = body
        .get("targetAccountId")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if target_account_id.trim().is_empty() {
        return json_err("缺少 targetAccountId".to_string(), StatusCode::BAD_REQUEST);
    }
    let result = tokio::task::spawn_blocking(move || {
        let target = account::find_account(&target_account_id).ok_or("目标账号不存在")?;
        vscode_session_sync::links_preview(&target)
    })
    .await;
    match result {
        Ok(Ok(value)) => json_ok(value),
        Ok(Err(error)) => json_err(error, StatusCode::BAD_REQUEST),
        Err(error) => json_err(error.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}

async fn api_codebuddy_ide_status() -> Response {
    json_ok(codebuddy_ide::status())
}

async fn api_codebuddy_ide_switch(Json(body): Json<Value>) -> Response {
    let account_id = body
        .get("accountId")
        .or_else(|| body.get("account_id"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let restart = body
        .get("restart")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    match codebuddy_ide::switch_account(account_id, restart) {
        Ok(v) => json_ok(v),
        Err(e) => json_err(e, StatusCode::BAD_REQUEST),
    }
}

async fn api_vscode_ext_detect() -> Response {
    match vscode_ext::detect_current_account() {
        Ok(v) => json_ok(v),
        Err(e) => json_err(e, StatusCode::BAD_REQUEST),
    }
}

async fn api_codebuddy_ide_detect() -> Response {
    match codebuddy_ide::detect_current_account() {
        Ok(v) => json_ok(v),
        Err(e) => json_err(e, StatusCode::BAD_REQUEST),
    }
}

async fn api_delete(Json(body): Json<Value>) -> Response {
    let id = body.get("accountId").and_then(|v| v.as_str()).unwrap_or("");
    match account::delete_account(id) {
        Ok(()) => json_ok(json!({ "ok": true })),
        Err(e) => json_err(e, StatusCode::BAD_REQUEST),
    }
}

/// POST /api/import-local —— 导入本机当前账号（body 可选 `variant`，缺省国内版）。
///
/// body 允许缺失，保持改造前的调用方式可用。
async fn api_import_local(body: Option<Json<Value>>) -> Response {
    let variant = body
        .as_ref()
        .map(|Json(value)| body_variant(value))
        .unwrap_or_else(|| WbVariant::parse(None));
    match account::import_local(variant) {
        Ok(acc) => json_ok(json!({ "ok": true, "account": acc })),
        Err(e) => json_err(e, StatusCode::BAD_REQUEST),
    }
}

// ---------------------------------------------------------------------------
// 导出 / 导入账号
// ---------------------------------------------------------------------------

async fn api_export_accounts(Json(body): Json<Value>) -> Response {
    let ids: Vec<String> = body
        .get("accountIds")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    match export_import::export_accounts(&ids) {
        Ok(records) => json_ok(json!({ "ok": true, "accounts": records })),
        Err(e) => json_err(e, StatusCode::BAD_REQUEST),
    }
}

async fn api_export_accounts_to_path(Json(body): Json<Value>) -> Response {
    let ids: Vec<String> = body
        .get("accountIds")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let path = body
        .get("path")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    match export_import::export_accounts_to_path(&ids, &path) {
        Ok(path) => json_ok(json!({ "ok": true, "path": path })),
        Err(e) => json_err(e, StatusCode::BAD_REQUEST),
    }
}

async fn api_preview_import(Json(body): Json<Value>) -> Response {
    let text = body
        .get("fileText")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    match export_import::preview_accounts(&text) {
        Ok(v) => json_ok(v),
        Err(e) => json_err(e, StatusCode::BAD_REQUEST),
    }
}

async fn api_import(Json(body): Json<Value>) -> Response {
    let text = body
        .get("fileText")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let indexes: Vec<usize> = body
        .get("indexes")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_u64().map(|n| n as usize))
                .collect()
        })
        .unwrap_or_default();
    match export_import::import_accounts(&text, &indexes) {
        Ok(result) => json_ok(json!({
            "ok": true,
            "imported": result.imported,
            "skipped": result.skipped,
            "overwritten": result.overwritten,
        })),
        Err(e) => json_err(e, StatusCode::BAD_REQUEST),
    }
}

// ---------------------------------------------------------------------------
// OAuth 登录
// ---------------------------------------------------------------------------

/// POST /api/oauth/start —— 发起扫码登录（body 可选 `variant`，缺省国内版）。
async fn api_oauth_start(body: Option<Json<Value>>) -> Response {
    let variant = body
        .as_ref()
        .map(|Json(value)| body_variant(value))
        .unwrap_or_else(|| WbVariant::parse(None));
    match oauth::oauth_start(variant).await {
        Ok(v) => json_ok(v),
        Err(e) => json_err(e, StatusCode::BAD_REQUEST),
    }
}

/// POST /api/oauth/status —— 轮询采集结果（档位取发起时记录，无需传参）。
async fn api_oauth_status(Json(body): Json<Value>) -> Response {
    let login_id = body
        .get("loginId")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    json_ok(oauth::oauth_poll(&login_id).await)
}

// ---------------------------------------------------------------------------
// 切换
// ---------------------------------------------------------------------------

async fn api_switch(Json(body): Json<Value>) -> Response {
    let account_id = body
        .get("accountId")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if account_id.trim().is_empty() {
        return json_err("缺少 accountId".to_string(), StatusCode::BAD_REQUEST);
    }
    let restart = body
        .get("restart")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let share_sessions = body
        .get("shareSessions")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let copy_ids: Vec<String> = body
        .get("copySessionIds")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    // 同步选择与桌面端同形（[{groupId, previewToken, mode}]），形状由 core 校验。
    let sync_selections = match session::parse_sync_selections(body.get("syncSelections")) {
        Ok(selections) => selections,
        Err(error) => return json_err(error, StatusCode::BAD_REQUEST),
    };

    {
        let mut running = SWITCH_RUNNING.lock().unwrap();
        if *running {
            return json_err("已有切换任务进行中".to_string(), StatusCode::CONFLICT);
        }
        *running = true;
        *SWITCH_PROGRESS.lock().unwrap() = Some("开始切换账号…".to_string());
    }

    let progress: switch::ProgressFn = Box::new(|msg| {
        *SWITCH_PROGRESS.lock().unwrap() = Some(msg.to_string());
    });

    let result = tokio::task::spawn_blocking(move || {
        switch::switch_account(
            Some(&progress),
            &account_id,
            restart,
            share_sessions,
            &copy_ids,
            &sync_selections,
        )
    })
    .await;

    *SWITCH_RUNNING.lock().unwrap() = false;

    match result {
        Ok(Ok(v)) => json_ok(v),
        Ok(Err(e)) => json_err(e, StatusCode::BAD_REQUEST),
        Err(e) => json_err(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}

async fn api_switch_progress() -> Response {
    let p = SWITCH_PROGRESS.lock().unwrap().clone();
    let running = *SWITCH_RUNNING.lock().unwrap();
    json_ok(json!({ "running": running, "progress": p }))
}

// ---------------------------------------------------------------------------
// 会话
// ---------------------------------------------------------------------------

/// GET /api/sessions —— 当前账号的会话列表（query 可选 `variant`，缺省国内版）。
async fn api_sessions(RawQuery(query): RawQuery) -> Response {
    let variant = query_variant(query.as_deref());
    match session::current_user_uid(variant) {
        Some(uid) => json_ok(json!({
            "sessions": session::list_sessions_for_user(variant, &uid),
            "current": uid,
            "variant": variant.as_str(),
        })),
        None => json_ok(json!({ "sessions": [], "current": null, "variant": variant.as_str() })),
    }
}

async fn api_copy_sessions(Json(body): Json<Value>) -> Response {
    let target_account_id = body
        .get("targetAccountId")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let session_ids: Vec<String> = body
        .get("sessionIds")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let Some(target) = account::find_account(&target_account_id) else {
        return json_err("目标账号不存在".to_string(), StatusCode::BAD_REQUEST);
    };
    // 档位取目标账号自身（源 uid 也从该档位的登录态读）。
    let variant = account::variant_of(&target);
    // 与桌面端同形：直接返回 core 的复制报告（copied / alreadyLinked / errors / needsRecovery）。
    let mut report = match session::copy_sessions_for_switch(&target, &session_ids) {
        Ok(report) => report,
        Err(error) => {
            return json_err(error, StatusCode::BAD_REQUEST);
        }
    };
    report["variant"] = json!(variant.as_str());
    json_ok(report)
}

/// POST /api/session-links/preview —— 预览当前账号 → 目标账号的关联会话同步项。
///
/// 与桌面端 `session_links_preview` 同形：直接返回 core 的只读预览（`supported` /
/// `storeStatus` / `groups`），每组的 `defaultChecked` 与 `availableModes` 是前端的
/// 勾选权限来源。`variant` 缺省取目标账号自身档位。
async fn api_session_links_preview(Json(body): Json<Value>) -> Response {
    let target_account_id = body
        .get("targetAccountId")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if target_account_id.trim().is_empty() {
        return json_err("缺少 targetAccountId".to_string(), StatusCode::BAD_REQUEST);
    }
    let Some(target) = account::find_account(&target_account_id) else {
        return json_err("目标账号不存在".to_string(), StatusCode::BAD_REQUEST);
    };
    let variant = match body.get("variant").and_then(Value::as_str) {
        Some(raw) => WbVariant::parse(Some(raw)),
        None => account::variant_of(&target),
    };
    match session::session_links_preview(variant, &target) {
        Ok(report) => json_ok(report),
        Err(error) => json_err(error, StatusCode::BAD_REQUEST),
    }
}

// ---------------------------------------------------------------------------
// 签到 / 保活
// ---------------------------------------------------------------------------

/// GET /api/checkin/status —— 传 accountId 时只查询该账号；缺省保留旧批量响应。
/// 两种形式都遵守单账号自动签到开关，避免展示状态时触发已关闭账号的请求。
async fn api_checkin_status(Query(query): Query<HashMap<String, String>>) -> Response {
    if let Some(id) = query.get("accountId") {
        let Some(acc) = account::find_account(id) else {
            return json_err("账号不存在".to_string(), StatusCode::BAD_REQUEST);
        };
        let status = checkin::get_checkin_status_for_display(&acc).await;
        return json_ok(checkin_status_item(&acc, status));
    }
    let list = account::load_accounts();
    let mut items = Vec::new();
    for acc in &list {
        let status = checkin::get_checkin_status_for_display(acc).await;
        items.push(checkin_status_item(acc, status));
    }
    json_ok(json!({ "accounts": items }))
}

fn checkin_status_item(account: &Value, mut status: Value) -> Value {
    status["accountId"] = account.get("id").cloned().unwrap_or(Value::Null);
    status["email"] = json!(account::account_display_name(account));
    status["variant"] = json!(account::variant_of(account).as_str());
    status
}

async fn api_credits(Json(body): Json<Value>) -> Response {
    let id = body.get("accountId").and_then(|v| v.as_str()).unwrap_or("");
    let Some(acc) = account::find_account(id) else {
        return json_err("账号不存在".to_string(), StatusCode::BAD_REQUEST);
    };
    json_ok(credits::get_credit_expiry(&acc).await)
}

fn query_flag_enabled(query: Option<&str>, name: &str) -> bool {
    query.unwrap_or("").split('&').any(|pair| {
        let (key, value) = pair.split_once('=').unwrap_or((pair, "true"));
        key == name && matches!(value, "" | "1" | "true" | "yes")
    })
}

async fn api_credit_statistics(RawQuery(query): RawQuery) -> Response {
    json_ok(credit_usage::get_statistics(query_flag_enabled(query.as_deref(), "refresh")).await)
}

async fn api_token_statistics(RawQuery(query): RawQuery) -> Response {
    let days = query.as_deref().and_then(|value| {
        value
            .split('&')
            .find_map(|part| part.strip_prefix("days=")?.parse::<i64>().ok())
    });
    match tokio::task::spawn_blocking(move || token_stats::get_statistics(days)).await {
        Ok(statistics) => json_ok(statistics),
        Err(error) => json_err(
            format!("扫描 Token 统计失败: {error}"),
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    }
}

/// GET /api/rate-limits —— 模型限额台账（全部账号当前受限的模型与官方恢复时刻）。
///
/// 扫描本机日志文件，放 blocking 线程避免占用运行时线程；无受限模型时返回空数组。
async fn api_rate_limits() -> Response {
    match tokio::task::spawn_blocking(limits::get_rate_limits).await {
        Ok(payload) => json_ok(payload),
        Err(error) => json_err(
            format!("扫描模型限额失败: {error}"),
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    }
}

/// hook 状态 + 运行期字段（最近一次 hook 事件时刻）。
fn rate_limit_hook_status() -> Value {
    let mut status = rate_limit_hook::hook_status();
    status["lastEventAt"] = json!(rate_limit_events::last_event_at());
    status
}

/// GET /api/rate-limits/hook-status —— hook 安装状态（脚本 + 三处客户端配置逐项结果）。
async fn api_rate_limit_hook_status() -> Response {
    match tokio::task::spawn_blocking(rate_limit_hook_status).await {
        Ok(status) => json_ok(status),
        Err(error) => json_err(
            format!("查询限额 hook 状态失败: {error}"),
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    }
}

/// POST /api/rate-limits/install-hook —— 安装 hook（幂等，写前备份；同时清除「卸载过」标记）。
async fn api_install_rate_limit_hook() -> Response {
    match tokio::task::spawn_blocking(|| {
        let result = rate_limit_hook::install_hook();
        // 扫描范围随安装结果变化（只对未注册的来源扫日志），缓存必须作废。
        limits::invalidate_scan_cache();
        result.map(|_| rate_limit_hook_status())
    })
    .await
    {
        Ok(Ok(status)) => json_ok(status),
        Ok(Err(error)) => json_err(error, StatusCode::BAD_REQUEST),
        Err(error) => json_err(error.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// POST /api/rate-limits/uninstall-hook —— 卸载 hook（移除注册条目，尽量逐字节还原）。
///
/// 卸载即用户拒绝自动接入（`hookOptOut`），与安装逻辑同处 core，两个宿主共用同一语义。
async fn api_uninstall_rate_limit_hook() -> Response {
    match tokio::task::spawn_blocking(|| {
        let result = rate_limit_hook::uninstall_hook();
        limits::invalidate_scan_cache();
        result.map(|_| rate_limit_hook_status())
    })
    .await
    {
        Ok(Ok(status)) => json_ok(status),
        Ok(Err(error)) => json_err(error, StatusCode::BAD_REQUEST),
        Err(error) => json_err(error.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}

async fn api_rate_limit_config() -> Response {
    json_ok(config::load_rate_limit_config())
}

/// POST /api/rate-limits/config —— 保存限额监听配置。
///
/// 与桌面端同语义：`scanIdeLogs` 变化时作废扫描缓存，下一次按当前来源范围重算。
async fn api_save_rate_limit_config(Json(body): Json<Value>) -> Response {
    let submitted = body.get("config").unwrap_or(&body);
    match limits::save_rate_limit_config(submitted) {
        Ok(()) => json_ok(config::load_rate_limit_config()),
        Err(e) => json_err(e.to_string(), StatusCode::BAD_REQUEST),
    }
}

async fn api_checkin(Json(body): Json<Value>) -> Response {
    let id = body.get("accountId").and_then(|v| v.as_str()).unwrap_or("");
    let Some(acc) = account::find_account(id) else {
        return json_err("账号不存在".to_string(), StatusCode::BAD_REQUEST);
    };
    json_ok(checkin::checkin_account(&acc).await)
}

async fn api_checkin_all(body: Option<Json<Value>>) -> Response {
    // 缺省（无 body / 无 variant）= 全部档位，保持与桌面端 set 前的行为一致。
    let variant = body
        .as_ref()
        .and_then(|Json(value)| value.get("variant"))
        .and_then(|value| value.as_str())
        .map(|raw| WbVariant::parse(Some(raw)));
    json_ok(checkin::run_checkin_all(variant).await)
}

async fn api_checkin_config() -> Response {
    json_ok(config::load_checkin_config())
}

async fn api_save_checkin_config(Json(body): Json<Value>) -> Response {
    let submitted = body.get("config").unwrap_or(&body);
    match config::save_checkin_config(submitted) {
        Ok(()) => json_ok(config::load_checkin_config()),
        Err(e) => json_err(e.to_string(), StatusCode::BAD_REQUEST),
    }
}

/// GET /api/checkin/logs —— 签到日志（每行带 `variant`，便于前端按档位过滤）。
async fn api_checkin_logs() -> Response {
    json_ok(json!({ "logs": checkin::load_checkin_logs_with_variant() }))
}

async fn api_travel_status() -> Response {
    travel::reconcile_due_travel(None).await;
    let items = account::load_accounts()
        .iter()
        .map(|acc| {
            let id = acc.get("id").and_then(Value::as_str).unwrap_or("");
            let mut value = travel::travel_display(id);
            value["accountId"] = acc.get("id").cloned().unwrap_or(Value::Null);
            value["email"] = json!(account::account_display_name(acc));
            value
        })
        .collect::<Vec<_>>();
    json_ok(json!({ "accounts": items }))
}

async fn api_travel_config() -> Response {
    json_ok(config::load_travel_config())
}

async fn api_save_travel_config(Json(body): Json<Value>) -> Response {
    let submitted = body.get("config").unwrap_or(&body);
    match config::save_travel_config(submitted) {
        Ok(()) => {
            let saved = config::load_travel_config();
            if saved.get("enabled").and_then(Value::as_bool) == Some(true) {
                tokio::spawn(async {
                    let _ = travel::run_travel_cycle().await;
                    let _ = travel::run_travel_claim_cycle().await;
                });
            }
            json_ok(saved)
        }
        Err(e) => json_err(e.to_string(), StatusCode::BAD_REQUEST),
    }
}

async fn api_refresh_token(Json(body): Json<Value>) -> Response {
    let id = body.get("accountId").and_then(|v| v.as_str()).unwrap_or("");
    let Some(acc) = account::find_account(id) else {
        return json_err("账号不存在".to_string(), StatusCode::BAD_REQUEST);
    };
    json_ok(refresh::refresh_account_token(acc).await)
}

// ---------------------------------------------------------------------------
// 自动轮换（CodeBuddy CLI）
// ---------------------------------------------------------------------------

async fn api_rotate_config() -> Response {
    json_ok(config::load_auto_rotate_config())
}

async fn api_save_rotate_config(Json(body): Json<Value>) -> Response {
    match config::save_auto_rotate_config(&body) {
        Ok(()) => json_ok(json!({ "ok": true, "config": config::load_auto_rotate_config() })),
        Err(e) => json_err(e.to_string(), StatusCode::BAD_REQUEST),
    }
}

async fn api_rotate_status() -> Response {
    json_ok(rotate::rotate_status())
}

async fn api_rotate_run() -> Response {
    json_ok(rotate::run_rotate_cycle().await)
}

async fn api_rotate_logs() -> Response {
    json_ok(json!({ "logs": rotate::rotate_logs() }))
}

// ---------------------------------------------------------------------------
// 更新
// ---------------------------------------------------------------------------

async fn api_update_check() -> Response {
    json_ok(update::update_check(None, false).await)
}

async fn api_update_config() -> Response {
    json_ok(update::load_github_config())
}

async fn api_save_update_config(Json(body): Json<Value>) -> Response {
    match update::save_github_config(&body) {
        Ok(()) => json_ok(json!({ "ok": true, "config": update::load_github_config() })),
        Err(e) => json_err(e.to_string(), StatusCode::BAD_REQUEST),
    }
}

// ---------------------------------------------------------------------------
// 静态前端
// ---------------------------------------------------------------------------

fn content_type(path: &str) -> &'static str {
    if path.ends_with(".js") || path.ends_with(".mjs") {
        "text/javascript"
    } else if path.ends_with(".css") {
        "text/css"
    } else if path.ends_with(".html") {
        "text/html; charset=utf-8"
    } else if path.ends_with(".json") {
        "application/json"
    } else if path.ends_with(".svg") {
        "image/svg+xml"
    } else if path.ends_with(".png") {
        "image/png"
    } else if path.ends_with(".ico") {
        "image/x-icon"
    } else if path.ends_with(".woff2") {
        "font/woff2"
    } else {
        "application/octet-stream"
    }
}

async fn static_handler(uri: Uri) -> Response {
    let mut path = uri.path().trim_start_matches('/').to_string();
    if path.is_empty() || path == "index.html" {
        path = "index.html".to_string();
    }
    // 前端路由回退到 index.html
    let data = Assets::get(&path).or_else(|| Assets::get("index.html"));
    match data {
        Some(f) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, content_type(&path))
            .body(Body::from(f.data.into_owned()))
            .unwrap(),
        None => Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::from("not found"))
            .unwrap(),
    }
}

// ---------------------------------------------------------------------------
// 通知存档（toast 事后可查）
// ---------------------------------------------------------------------------

/// GET /api/notifications —— 最近的应用内提示（新的在前，最多 100 条）。
async fn api_notifications() -> Response {
    match notifications::list() {
        Ok(items) => json_ok(json!({ "items": items })),
        Err(error) => json_err(error, StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// POST /api/notifications/record —— 记录一条提示（前端 toast 同步写一份）。
async fn api_record_notification(Json(body): Json<Value>) -> Response {
    let level = body.get("level").and_then(|v| v.as_str()).unwrap_or("info");
    let title = body.get("title").and_then(|v| v.as_str()).unwrap_or("");
    let description = body.get("description").and_then(|v| v.as_str());
    match notifications::record(level, title, description) {
        Ok(()) => json_ok(json!({ "recorded": true })),
        Err(error) => json_err(error, StatusCode::BAD_REQUEST),
    }
}

/// POST /api/notifications/clear —— 清空通知存档。
async fn api_clear_notifications() -> Response {
    match notifications::clear() {
        Ok(()) => json_ok(json!({ "cleared": true })),
        Err(error) => json_err(error, StatusCode::BAD_REQUEST),
    }
}

#[cfg(test)]
mod tests {
    use super::{body_variant, checkin_status_item, query_variant};
    use serde_json::json;
    use wb_switch_core::modules::variant::WbVariant;

    /// 缺省档位必须与改造前一致（不传 variant 即国内版）。
    #[test]
    fn variant_query_defaults_to_cn() {
        assert_eq!(query_variant(None), WbVariant::Cn);
        assert_eq!(query_variant(Some("")), WbVariant::Cn);
        assert_eq!(query_variant(Some("refresh=true")), WbVariant::Cn);
        assert_eq!(
            query_variant(Some("refresh=true&variant=cn")),
            WbVariant::Cn
        );
        assert_eq!(query_variant(Some("variant=ai")), WbVariant::Ai);
        assert_eq!(
            query_variant(Some("variant=ai&refresh=true")),
            WbVariant::Ai
        );
        assert_eq!(query_variant(Some("variant=unknown")), WbVariant::Cn);
    }

    #[test]
    fn variant_body_defaults_to_cn() {
        assert_eq!(body_variant(&json!({})), WbVariant::Cn);
        assert_eq!(body_variant(&json!({"variant": null})), WbVariant::Cn);
        assert_eq!(body_variant(&json!({"variant": "ai"})), WbVariant::Ai);
        assert_eq!(body_variant(&json!({"accountId": "x"})), WbVariant::Cn);
    }

    #[test]
    fn web_checkin_status_keeps_account_identity() {
        let item = checkin_status_item(
            &json!({"id": "account-1", "email": "user@example.com"}),
            json!({"ok": true, "todayCheckedIn": true}),
        );

        assert_eq!(item["accountId"], "account-1");
        assert_eq!(item["email"], "user@example.com");
        assert_eq!(item["todayCheckedIn"], true);
        assert_eq!(item["variant"], "cn");
    }

    #[test]
    fn web_checkin_status_preserves_failure_state() {
        let item = checkin_status_item(
            &json!({"id": "account-2"}),
            json!({"ok": false, "todayCheckedIn": false, "error": "status failed"}),
        );

        assert_eq!(item["accountId"], "account-2");
        assert_eq!(item["ok"], false);
        assert_eq!(item["error"], "status failed");
    }

    #[test]
    fn web_checkin_status_preserves_exclusion_without_inventing_today_status() {
        let item = checkin_status_item(
            &json!({"id": "excluded"}),
            json!({"ok": false, "result": "skipped", "reason": "auto_checkin_disabled"}),
        );
        assert_eq!(item["accountId"], "excluded");
        assert_eq!(item["reason"], "auto_checkin_disabled");
        assert_eq!(item["result"], "skipped");
        assert!(item.get("todayCheckedIn").is_none());
    }

    #[test]
    fn web_checkin_status_row_carries_variant() {
        let item = checkin_status_item(
            &json!({"id": "ai-1", "variant": "ai"}),
            json!({"ok": false, "statusUnsupported": true}),
        );

        assert_eq!(item["variant"], "ai");
        assert_eq!(item["statusUnsupported"], true);
    }
}

// ---------------------------------------------------------------------------
// TRAE 子系统(对应桌面端 trae_commands.rs 的 35 个命令;核心逻辑在 trae-core)
// ---------------------------------------------------------------------------

fn trae_missing(param: &str) -> Response {
    json_err(format!("缺少 {param}"), StatusCode::BAD_REQUEST)
}

fn trae_not_found(id: &str) -> Response {
    json_err(format!("账号不存在: {id}"), StatusCode::NOT_FOUND)
}

async fn api_trae_accounts() -> Response {
    json_ok(
        serde_json::to_value(trae_core::views::build_account_views(trae_state()))
            .unwrap_or(json!([])),
    )
}

async fn api_trae_import_desktop() -> Response {
    trae_json(trae_core::accounts::import_desktop(trae_state()))
}

async fn api_trae_update_account(Json(body): Json<Value>) -> Response {
    let Some(id) = body_str(&body, "id") else {
        return trae_missing("id");
    };
    // 兼容两种形态:{id, updates:{...}} 或把更新字段直接平铺在 body 里
    let updates = body.get("updates").cloned().unwrap_or_else(|| body.clone());
    trae_json(trae_core::accounts::update(trae_state(), &id, updates))
}

async fn api_trae_delete_account(Json(body): Json<Value>) -> Response {
    match body_str(&body, "id") {
        Some(id) => trae_json(trae_core::accounts::delete(trae_state(), &id)),
        None => trae_missing("id"),
    }
}

async fn api_trae_add_jwt(Json(body): Json<Value>) -> Response {
    let Some(jwt) = body_str(&body, "jwt") else {
        return trae_missing("jwt");
    };
    let name = body_str(&body, "name").unwrap_or_default();
    let refresh_token = body_str(&body, "refreshToken");
    let enabled = body.get("enabled").and_then(Value::as_bool);
    trae_json(trae_core::accounts::add_jwt(
        trae_state(),
        name,
        &jwt,
        refresh_token,
        enabled,
    ))
}

async fn api_trae_checkin(Json(body): Json<Value>) -> Response {
    match body_str(&body, "id") {
        Some(id) => trae_json(
            trae_core::accounts::checkin_one(trae_state(), trae_client(), &id).await,
        ),
        None => trae_missing("id"),
    }
}

async fn api_trae_checkin_all() -> Response {
    // 手动一键签到:账号间保持 2s 快速执行(与桌面端一致;自动定时签到用分钟级间隔)
    let results =
        trae_core::checkin::perform_all_checkin(None, trae_client(), trae_state(), 2).await;
    json_ok(serde_json::to_value(results).unwrap_or(json!([])))
}

async fn api_trae_points(Json(body): Json<Value>) -> Response {
    match body_str(&body, "id") {
        Some(id) => {
            trae_json(trae_core::accounts::account_points(trae_state(), trae_client(), &id).await)
        }
        None => trae_missing("id"),
    }
}

async fn api_trae_logs(RawQuery(query): RawQuery) -> Response {
    let limit = query
        .as_deref()
        .and_then(|q| {
            q.split('&').find_map(|p| p.split_once('=')).and_then(|(k, v)| {
                (k == "limit").then_some(v).and_then(|v| v.parse::<usize>().ok())
            })
        })
        .unwrap_or(100);
    let data = trae_state().data.lock().unwrap();
    json_ok(serde_json::to_value(data.get_logs(limit)).unwrap_or(json!([])))
}

async fn api_trae_clear_logs() -> Response {
    let state = trae_state();
    let r = {
        let mut data = state.data.lock().unwrap();
        data.clear_logs();
        data.save(&state.store_file())
    };
    match r {
        Ok(()) => json_ok(json!(true)),
        Err(e) => json_err(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}

async fn api_trae_get_settings() -> Response {
    // server 形态无自启插件:launch_at_login 返回存储值
    // (桌面端 trae_get_settings 会以系统自启真实状态覆盖)
    let s = trae_state().data.lock().unwrap().get_settings();
    json_ok(serde_json::to_value(s).unwrap_or(json!({})))
}

async fn api_trae_save_settings(Json(body): Json<Value>) -> Response {
    // 前端发送 {settings: partial};兼容直接平铺的 partial
    let payload = body.get("settings").cloned().unwrap_or(body);
    let settings: trae_core::models::PartialAppSettings = match serde_json::from_value(payload) {
        Ok(s) => s,
        Err(e) => return json_err(format!("设置格式错误: {e}"), StatusCode::BAD_REQUEST),
    };
    let state = trae_state();
    let r = {
        let mut data = state.data.lock().unwrap();
        let s = data.save_settings(settings);
        data.save(&state.store_file()).map(|_| s)
    };
    match r {
        Ok(s) => {
            // 设置变更后重启定时任务(与桌面端 trae_save_settings 一致)
            trae_start_scheduler();
            json_ok(serde_json::to_value(s).unwrap_or(json!({})))
        }
        Err(e) => json_err(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}

async fn api_trae_scheduler_start() -> Response {
    json_ok(json!(trae_start_scheduler()))
}

async fn api_trae_scheduler_stop() -> Response {
    TRAE_SCHED_GEN.fetch_add(1, Ordering::SeqCst);
    json_ok(json!(true))
}

async fn api_trae_next_run() -> Response {
    let settings = trae_state().data.lock().unwrap().get_settings();
    let next = if settings.auto_checkin {
        trae_core::schedule::next_run_instant(&settings).map(|dt| dt.to_rfc3339())
    } else {
        None
    };
    json_ok(json!(next))
}

async fn api_trae_launch(Json(body): Json<Value>) -> Response {
    match body_str(&body, "id") {
        Some(id) => {
            // server 形态:exe 路径等宿主配置存 trae_dir()(桌面端存应用配置目录)
            let config_dir = config::trae_dir();
            trae_json(trae_core::trae_instance::launch_account(
                trae_state(),
                &config_dir,
                &id,
            ))
        }
        None => trae_missing("id"),
    }
}

async fn api_trae_get_exe_path() -> Response {
    let config_dir = config::trae_dir();
    match trae_core::trae_machine::get_saved_trae_path(&config_dir) {
        Ok(p) => json_ok(json!(p)),
        Err(e) => json_err(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}

async fn api_trae_set_exe_path(Json(body): Json<Value>) -> Response {
    let Some(path) = body_str(&body, "path") else {
        return trae_missing("path");
    };
    let config_dir = config::trae_dir();
    match trae_core::trae_machine::save_trae_path(&config_dir, &path) {
        Ok(()) => json_ok(json!({ "ok": true })),
        Err(e) => json_err(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}

async fn api_trae_scan_exe_path() -> Response {
    match trae_core::trae_machine::scan_trae_exe_path() {
        Ok(scanned) => {
            let config_dir = config::trae_dir();
            let _ = trae_core::trae_machine::save_trae_path(&config_dir, &scanned);
            json_ok(json!(scanned))
        }
        Err(e) => json_err(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}

async fn api_trae_instance_state(Json(body): Json<Value>) -> Response {
    let Some(id) = body_str(&body, "id") else {
        return trae_missing("id");
    };
    let state = trae_state();
    let Some(account) = trae_core::accounts::get_account(state, &id) else {
        return trae_not_found(&id);
    };
    let main = trae_core::trae_machine::probe_main_instance();
    let source = trae_core::trae_machine::account_state(&account, &main);
    let is_main_account = main.1.as_deref()
        == account
            .desktop_user_id
            .as_deref()
            .filter(|s| !s.is_empty());
    json_ok(json!({
        "running": !matches!(source, trae_core::trae_machine::InstanceSource::None),
        "source": serde_json::to_value(source).unwrap_or(json!(null)),
        "isMainAccount": is_main_account,
    }))
}

async fn api_trae_focus(Json(body): Json<Value>) -> Response {
    let Some(id) = body_str(&body, "id") else {
        return trae_missing("id");
    };
    let state = trae_state();
    let Some(account) = trae_core::accounts::get_account(state, &id) else {
        return trae_not_found(&id);
    };
    let main = trae_core::trae_machine::probe_main_instance();
    let r = match trae_core::trae_machine::account_state(&account, &main) {
        trae_core::trae_machine::InstanceSource::Tool => {
            match account.data_dir.as_deref().filter(|s| !s.is_empty()) {
                Some(d) => trae_core::trae_machine::focus_instance_window(d),
                None => Err(trae_core::error::AppError::Launch(
                    "该账号无工具实例数据目录".into(),
                )),
            }
        }
        trae_core::trae_machine::InstanceSource::Main => trae_core::trae_machine::main_data_dir()
            .and_then(|dir| {
                trae_core::trae_machine::focus_instance_window(&dir.to_string_lossy())
            }),
        trae_core::trae_machine::InstanceSource::None => Err(trae_core::error::AppError::Launch(
            "该账号尚未启动,无实例可聚焦".into(),
        )),
    };
    trae_json(r)
}

async fn api_trae_open_login_instance() -> Response {
    let Ok(appdata) = std::env::var("APPDATA") else {
        return json_err(
            "无法获取 APPDATA 环境变量".into(),
            StatusCode::INTERNAL_SERVER_ERROR,
        );
    };
    let config_dir = config::trae_dir();
    let exe_path = match trae_core::trae_machine::resolve_trae_path(&config_dir) {
        Ok(p) => p,
        Err(e) => return json_err(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    };
    // 临时 data-dir(带 uuid 后缀,避免与标准目录或多次操作冲突;与桌面端一致)
    let temp_dir = std::path::PathBuf::from(&appdata)
        .join(format!(
            "{} login {}",
            trae_core::trae_machine::DATA_DIR_NAME,
            uuid::Uuid::new_v4()
        ))
        .to_string_lossy()
        .to_string();
    let shared_ext = std::path::PathBuf::from(&appdata)
        .join(trae_core::trae_instance::SHARED_EXTENSIONS_DIR)
        .to_string_lossy()
        .to_string();
    if let Err(e) = trae_core::trae_machine::open_product_with_data_dir(
        &exe_path,
        &temp_dir,
        Some(&shared_ext),
    ) {
        return json_err(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR);
    }
    // server 无事件通道:导入在后台线程执行,结果仅打日志,webui 刷新账号列表可见
    let state = trae_state();
    let appdata_owned = appdata;
    let temp_dir_owned = temp_dir;
    std::thread::spawn(move || {
        let result =
            trae_core::trae_instance::import_logged_in_temp_dir(state, &appdata_owned, &temp_dir_owned);
        println!("[TRAE] 登录导入结果: {result}");
    });
    json_ok(json!({ "ok": true }))
}

async fn api_trae_instance_dirs() -> Response {
    json_ok(
        serde_json::to_value(trae_core::trae_instance::scan_bound_dirs(trae_state()))
            .unwrap_or(json!([])),
    )
}

async fn api_trae_import_dir(Json(body): Json<Value>) -> Response {
    match body_str(&body, "dataDir") {
        Some(dir) => trae_json(trae_core::accounts::import_from_dir(trae_state(), &dir)),
        None => trae_missing("dataDir"),
    }
}

async fn api_trae_refresh_credential(Json(body): Json<Value>) -> Response {
    let Some(id) = body_str(&body, "id") else {
        return trae_missing("id");
    };
    let state = trae_state();
    let Some(account) = trae_core::accounts::get_account(state, &id) else {
        return trae_not_found(&id);
    };
    if let Err(e) =
        trae_core::checkin::refresh_account_credential(&account, trae_client(), state).await
    {
        return json_err(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR);
    }
    match trae_core::accounts::get_account(state, &id) {
        Some(acc) => json_ok(
            serde_json::to_value(trae_core::models::PublicAccount::from(acc))
                .unwrap_or(json!(null)),
        ),
        None => trae_not_found(&id),
    }
}

async fn api_trae_jwt_preview(Json(body): Json<Value>) -> Response {
    let Some(jwt) = body_str(&body, "jwt") else {
        return trae_missing("jwt");
    };
    let info = trae_core::jwt::parse(&jwt);
    if info.user_id.is_none() {
        return json_err(
            "无法从 JWT 解析 user_id,请检查格式".into(),
            StatusCode::BAD_REQUEST,
        );
    }
    json_ok(serde_json::to_value(info).unwrap_or(json!(null)))
}

async fn api_trae_refresh_jwt(Json(body): Json<Value>) -> Response {
    match body_str(&body, "userId") {
        Some(uid) => {
            trae_json(trae_core::accounts::refresh_jwt(trae_state(), trae_client(), &uid).await)
        }
        None => trae_missing("userId"),
    }
}

async fn api_trae_cooldown_clear(Json(body): Json<Value>) -> Response {
    match body_str(&body, "userId") {
        Some(uid) => {
            trae_core::cooldown::clear_cooldown(trae_state(), &uid);
            json_ok(json!({ "ok": true }))
        }
        None => trae_missing("userId"),
    }
}

async fn api_trae_cooldown_clear_all() -> Response {
    json_ok(json!(trae_core::cooldown::clear_all_cooldowns(trae_state())))
}

async fn api_trae_device_reset(Json(body): Json<Value>) -> Response {
    match body_str(&body, "userId") {
        Some(uid) => {
            trae_core::device_map::reset_device_for(trae_state(), &uid);
            json_ok(json!({ "ok": true }))
        }
        None => trae_missing("userId"),
    }
}

async fn api_trae_credits_fetch(Json(body): Json<Value>) -> Response {
    match body_str(&body, "userId") {
        Some(uid) => {
            trae_json(trae_core::accounts::fetch_remaining(trae_state(), trae_client(), &uid).await)
        }
        None => trae_missing("userId"),
    }
}

async fn api_trae_credits_refresh_all() -> Response {
    trae_json(
        trae_core::credits::refresh_remaining_credits(trae_state(), trae_client())
            .await
            .map_err(trae_core::error::AppError::Credential),
    )
}

async fn api_trae_credits_daily() -> Response {
    let snaps: Vec<trae_core::models::CreditsDailySnapshot> =
        trae_core::fs_utils::read_json(&trae_state().data_path("credits_daily.json"));
    json_ok(serde_json::to_value(snaps).unwrap_or(json!([])))
}

async fn api_trae_open_url(Json(body): Json<Value>) -> Response {
    let Some(url) = body_str(&body, "url") else {
        return trae_missing("url");
    };
    #[cfg(windows)]
    {
        if let Err(e) = std::process::Command::new("cmd")
            .args(["/c", "start", "", &url])
            .spawn()
        {
            return json_err(
                format!("打开链接失败: {e}"),
                StatusCode::INTERNAL_SERVER_ERROR,
            );
        }
    }
    #[cfg(not(windows))]
    let _ = url;
    json_ok(json!({ "ok": true }))
}

async fn api_trae_migrate() -> Response {
    // server 无应用配置目录:exe 路径迁移目标传 None(仅迁移账号/日志数据)
    let trae_dir = config::trae_dir();
    let _ = std::fs::create_dir_all(&trae_dir);
    let report = trae_core::migrate::migrate_from_legacy(&trae_dir, None);
    json_ok(serde_json::to_value(report).unwrap_or(json!(null)))
}

// ---------------------------------------------------------------------------
// Qoder / ZCode 信用平台(自 CreditDaddy 合并;核心逻辑在 credit-core;
// 数据根 ~/.wb-switch/qoder/ 与 ~/.wb-switch/zcode/)
// ---------------------------------------------------------------------------

static QODER_STATE: OnceLock<QoderState> = OnceLock::new();
static ZCODE_STATE: OnceLock<ZcodeState> = OnceLock::new();
static LINGXI_STATE: OnceLock<LingxiState> = OnceLock::new();
/// 两平台共用一个 Client(与 TRAE 同款共享模式)。
static CREDIT_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
/// 调度循环代数:设置变更后 +1,旧循环自行退出(同 TRAE_SCHED_GEN)。
static CREDIT_SCHED_GEN: AtomicU64 = AtomicU64::new(0);
/// 调度器记录的下次执行时刻(Unix 毫秒;None=未调度),供 next-run 查询。
static CREDIT_NEXT_RUN_MS: Mutex<Option<i64>> = Mutex::new(None);
/// 灵犀今日已执行的时间点标记("YYYY-MM-DD|HH:MM"),防同一天重复执行。
static LINGXI_FIRED: Mutex<Vec<String>> = Mutex::new(Vec::new());

pub fn qoder_state() -> &'static QoderState {
    QODER_STATE.get_or_init(open_qoder_state)
}

pub fn zcode_state() -> &'static ZcodeState {
    ZCODE_STATE.get_or_init(open_zcode_state)
}

pub fn lingxi_state() -> &'static LingxiState {
    LINGXI_STATE.get_or_init(open_lingxi_state)
}

fn credit_client() -> &'static reqwest::Client {
    CREDIT_CLIENT.get_or_init(reqwest::Client::new)
}

/// server 启动时初始化信用平台子系统:三个 state(自动建目录)+ 共享 Client,
/// 按设置启动自动领取循环(由 main.rs spawn_background_loops 调用)。
pub fn init_credit() {
    let _ = lingxi_state();
    credit_start_scheduler();
}

/// 信用平台自动领取调度(server 形态):一个循环覆盖三平台,与桌面端
/// credit_scheduler 同语义——Qoder/ZCode 每轮 2h±10min 抖动(credit_core::schedule),
/// 灵犀每日定点时间点在分段 sleep 的 15s 粒度里检查;generation 计数控制任务
/// 生命周期(旧任务自行退出),分段 sleep 15s 及时响应设置变更;无系统通知能力,
/// 结果降级为 stdout 日志。
fn credit_start_scheduler() -> bool {
    let gen = CREDIT_SCHED_GEN.fetch_add(1, Ordering::SeqCst) + 1;
    let qoder_auto = qoder_state()
        .data
        .lock()
        .unwrap()
        .get_settings()
        .auto_claim_enabled;
    let zcode_auto = zcode_state()
        .data
        .lock()
        .unwrap()
        .get_settings()
        .auto_claim_enabled;
    let lingxi_auto = lingxi_enabled();
    if !qoder_auto && !zcode_auto && !lingxi_auto {
        return true;
    }
    tokio::spawn(async move {
        loop {
            if CREDIT_SCHED_GEN.load(Ordering::SeqCst) != gen {
                break;
            }
            let delay = credit_core::schedule::next_round_delay_default();
            // 记录下次执行时刻,供 /api/{qoder,zcode}/next-run 查询
            *CREDIT_NEXT_RUN_MS.lock().unwrap() =
                Some(config::now_ms() + delay.as_millis() as i64);
            // 分段 sleep,每 15s 检查 generation,及时响应设置变更;
            // 顺带做灵犀每日定点检查(到点即执行一轮,三平台同环不另开循环)
            let mut remaining = delay;
            while remaining > std::time::Duration::ZERO {
                if CREDIT_SCHED_GEN.load(Ordering::SeqCst) != gen {
                    break;
                }
                check_lingxi_due().await;
                let step = remaining.min(std::time::Duration::from_secs(15));
                tokio::time::sleep(step).await;
                remaining = remaining.saturating_sub(step);
            }
            if CREDIT_SCHED_GEN.load(Ordering::SeqCst) != gen {
                break;
            }
            credit_run_round().await;
        }
    });
    true
}

/// 灵犀是否参与调度:配置了有效签到时间点即参与(每日定点模型,无独立开关)。
fn lingxi_enabled() -> bool {
    let times = lingxi_state().data.lock().unwrap().get_settings().checkin_times;
    !credit_core::schedule::parse_hhmm_list(times.iter().map(String::as_str)).is_empty()
}

/// 灵犀到点检查(与桌面端 credit_scheduler 同语义):存在"已到点且今日未执行过"
/// 的时间点 → 执行一轮;fired 标记记 "YYYY-MM-DD|HH:MM",换日自动失效。
async fn check_lingxi_due() {
    let times = lingxi_state().data.lock().unwrap().get_settings().checkin_times;
    let now = chrono::Utc::now();
    let today = credit_core::schedule::shanghai_today(now);
    let tz = credit_core::schedule::shanghai_tz();
    let today_cn = now.with_timezone(&tz).date_naive();
    let mut due: Vec<String> = Vec::new();
    for (h, m) in credit_core::schedule::parse_hhmm_list(times.iter().map(String::as_str)) {
        let at = tz.with_ymd_and_hms(today_cn.year(), today_cn.month(), today_cn.day(), h, m, 0);
        if let Some(at) = at.single() {
            if at.with_timezone(&chrono::Utc) <= now {
                due.push(format!("{today}|{h:02}:{m:02}"));
            }
        }
    }
    if due.is_empty() {
        return;
    }
    {
        let mut fired = LINGXI_FIRED.lock().unwrap();
        fired.retain(|k| k.starts_with(&format!("{today}|")));
        let new_keys: Vec<String> = due.iter().filter(|k| !fired.contains(*k)).cloned().collect();
        if new_keys.is_empty() {
            return;
        }
        fired.extend(new_keys);
    }
    run_lingxi_round().await;
}

/// 一轮灵犀签到(无通知者,汇总打 stdout,失败明细走 eprintln)。
async fn run_lingxi_round() {
    let today = credit_core::schedule::shanghai_today(chrono::Utc::now());
    let results =
        credit_core::lingxi::checkin::checkin_all(lingxi_state(), credit_client(), &today).await;
    let ok = results.iter().filter(|(_, o)| o.outcome.is_success()).count();
    println!("[灵犀] 自动签到完成: 成功 {ok}, 共 {}", results.len());
    for (_, o) in &results {
        if o.outcome == credit_core::lingxi::Outcome::Failed {
            eprintln!("[灵犀] 签到失败: {}", o.message);
        }
    }
}

/// 一轮自动领取:Qoder checkin_all + ZCode claim_all(各自受平台自动开关约束;
/// 无通知者,汇总打 stdout,失败明细走 eprintln)。
async fn credit_run_round() {
    let client = credit_client();
    if qoder_state()
        .data
        .lock()
        .unwrap()
        .get_settings()
        .auto_claim_enabled
    {
        let results = credit_core::qoder::checkin::checkin_all(qoder_state(), client).await;
        let ok = results.iter().filter(|(_, o)| o.outcome.is_success()).count();
        println!("[Qoder] 自动领取完成: 成功 {ok}, 共 {}", results.len());
        for (_, o) in &results {
            if o.outcome == credit_core::models::ClaimOutcome::Failed {
                eprintln!("[Qoder] 领取失败: {}", o.message);
            }
        }
    }
    if zcode_state()
        .data
        .lock()
        .unwrap()
        .get_settings()
        .auto_claim_enabled
    {
        let results = credit_core::zcode::claim::claim_all(zcode_state(), client).await;
        let ok = results.iter().filter(|(_, o)| o.outcome.is_success()).count();
        println!("[ZCode] 自动领取完成: 成功 {ok}, 共 {}", results.len());
        for (_, o) in &results {
            if o.outcome == credit_core::models::ClaimOutcome::Failed {
                eprintln!("[ZCode] 领取失败: {}", o.message);
            }
        }
    }
}

/// 命令结果序列化:AppResult<T> 成功回 JSON 值,失败回 {ok:false,error}(同 trae_json)。
fn credit_json<T: serde::Serialize>(r: Result<T, credit_core::error::AppError>) -> Response {
    match r {
        Ok(v) => json_ok(serde_json::to_value(v).unwrap_or(json!(null))),
        Err(e) => json_err(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}

fn credit_missing(param: &str) -> Response {
    json_err(format!("缺少 {param}"), StatusCode::BAD_REQUEST)
}

fn credit_not_found(id: &str) -> Response {
    json_err(format!("账号不存在: {id}"), StatusCode::NOT_FOUND)
}

/// 调度器记录的下次执行时刻(RFC3339);自动领取关闭时视为未调度。
fn credit_next_run_rfc3339() -> Option<String> {
    let ms = (*CREDIT_NEXT_RUN_MS.lock().unwrap())?;
    chrono::DateTime::from_timestamp_millis(ms).map(|dt| dt.to_rfc3339())
}

fn find_qoder_account(id: &str) -> Option<credit_core::models::QoderAccount> {
    qoder_state()
        .data
        .lock()
        .unwrap()
        .get_accounts()
        .iter()
        .find(|a| a.id == id)
        .cloned()
}

fn find_zcode_account(id: &str) -> Option<credit_core::models::ZCodeAccount> {
    zcode_state()
        .data
        .lock()
        .unwrap()
        .get_accounts()
        .iter()
        .find(|a| a.id == id)
        .cloned()
}

/// Qoder 领取结果序列化(QoderCheckinOutcome 未实现 Serialize)。
fn qoder_outcome_json(id: &str, o: &credit_core::qoder::checkin::QoderCheckinOutcome) -> Value {
    json!({
        "id": id,
        "outcome": o.outcome.as_str(),
        "message": o.message,
        "claimedAmount": o.claimed_amount,
        "userId": o.uid,
        "risk": o.risk,
    })
}

/// ZCode 领取结果序列化(ZcodeClaimOutcome 未实现 Serialize)。
fn zcode_outcome_json(id: &str, o: &credit_core::zcode::claim::ZcodeClaimOutcome) -> Value {
    json!({
        "id": id,
        "outcome": o.outcome.as_str(),
        "message": o.message,
        "claims": o
            .claims
            .iter()
            .map(|c| {
                json!({
                    "plan": c.plan,
                    "ok": c.ok,
                    "already": c.already,
                    "needManual": c.need_manual,
                    "via": c.via,
                    "message": c.message,
                })
            })
            .collect::<Vec<_>>(),
    })
}

// ----- Qoder handlers -----

async fn api_qoder_accounts() -> Response {
    let data = qoder_state().data.lock().unwrap();
    json_ok(serde_json::to_value(data.get_accounts()).unwrap_or(json!([])))
}

async fn api_qoder_import_local() -> Response {
    // DPAPI 解密本机客户端 auth.v1.dat 是阻塞 IO,放 blocking 线程
    // (非 Windows 由 credit-core 返回空列表 + 错误说明,诚实降级)
    let (locals, errors) = match tokio::task::spawn_blocking(
        credit_core::qoder::identity::read_app_accounts,
    )
    .await
    {
        Ok(r) => r,
        Err(e) => return json_err(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    };
    let state = qoder_state();
    let region = state.data.lock().unwrap().get_settings().region;
    let now = config::now_ms();
    let mut imported = 0usize;
    let mut skipped = 0usize;
    let r = {
        let mut data = state.data.lock().unwrap();
        for la in locals {
            // 同 token 已存在:不重复导入
            if data.get_accounts().iter().any(|a| a.token == la.token) {
                skipped += 1;
                continue;
            }
            let name = la
                .user_name
                .or_else(|| la.user_email.clone())
                .unwrap_or(la.source);
            data.accounts.push(credit_core::models::QoderAccount {
                id: credit_core::store::generate_id(),
                name,
                token: la.token,
                refresh_token: la.refresh_token,
                user_id: la.user_id,
                email: la.user_email,
                region: region.clone(),
                source: "local-app".into(),
                expires_at: la.expires_at,
                created_at: now,
                enabled: true,
                ..Default::default()
            });
            imported += 1;
        }
        data.save(&state.store_file())
    };
    match r {
        Ok(()) => json_ok(json!({ "imported": imported, "skipped": skipped, "errors": errors })),
        Err(e) => json_err(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}

async fn api_qoder_add_account(Json(body): Json<Value>) -> Response {
    let token = match body_str(&body, "token") {
        Some(t) if !t.trim().is_empty() => t,
        _ => return credit_missing("token"),
    };
    let state = qoder_state();
    // 区域缺省取设置里的"新账号默认区域"
    let region = body_str(&body, "region")
        .unwrap_or_else(|| state.data.lock().unwrap().get_settings().region);
    let account = credit_core::models::QoderAccount {
        id: credit_core::store::generate_id(),
        name: body_str(&body, "name").unwrap_or_else(|| "Qoder 账号".into()),
        token: token.trim().into(),
        region,
        source: "manual".into(),
        created_at: config::now_ms(),
        enabled: body.get("enabled").and_then(Value::as_bool).unwrap_or(true),
        ..Default::default()
    };
    let r = {
        let mut data = state.data.lock().unwrap();
        data.accounts.push(account.clone());
        data.save(&state.store_file())
    };
    match r {
        Ok(()) => json_ok(serde_json::to_value(account).unwrap_or(json!(null))),
        Err(e) => json_err(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}

async fn api_qoder_update_account(Json(body): Json<Value>) -> Response {
    let Some(id) = body_str(&body, "id") else {
        return credit_missing("id");
    };
    // 兼容两种形态:{id, updates:{...}} 或把更新字段直接平铺在 body 里
    let updates = body.get("updates").cloned().unwrap_or_else(|| body.clone());
    let state = qoder_state();
    let mut data = state.data.lock().unwrap();
    let Some(acc) = data.update_account(&id, updates) else {
        return credit_not_found(&id);
    };
    match data.save(&state.store_file()).map(|_| acc) {
        Ok(acc) => json_ok(serde_json::to_value(acc).unwrap_or(json!(null))),
        Err(e) => json_err(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}

async fn api_qoder_delete_account(Json(body): Json<Value>) -> Response {
    let Some(id) = body_str(&body, "id") else {
        return credit_missing("id");
    };
    let state = qoder_state();
    let r = {
        let mut data = state.data.lock().unwrap();
        if data.get_accounts().iter().all(|a| a.id != id) {
            return credit_not_found(&id);
        }
        data.delete_account(&id);
        data.save(&state.store_file())
    };
    match r {
        Ok(()) => json_ok(json!({ "ok": true })),
        Err(e) => json_err(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}

async fn api_qoder_checkin(Json(body): Json<Value>) -> Response {
    let Some(id) = body_str(&body, "id") else {
        return credit_missing("id");
    };
    let Some(acc) = find_qoder_account(&id) else {
        return credit_not_found(&id);
    };
    let o = credit_core::qoder::checkin::checkin_one(qoder_state(), credit_client(), &acc).await;
    json_ok(qoder_outcome_json(&id, &o))
}

async fn api_qoder_checkin_all() -> Response {
    let results = credit_core::qoder::checkin::checkin_all(qoder_state(), credit_client()).await;
    json_ok(Value::Array(
        results
            .iter()
            .map(|(id, o)| qoder_outcome_json(id, o))
            .collect(),
    ))
}

async fn api_qoder_quota(Json(body): Json<Value>) -> Response {
    let Some(id) = body_str(&body, "id") else {
        return credit_missing("id");
    };
    let Some(acc) = find_qoder_account(&id) else {
        return credit_not_found(&id);
    };
    credit_json(
        credit_core::qoder::checkin::refresh_quota(qoder_state(), credit_client(), &acc).await,
    )
}

async fn api_qoder_logs(RawQuery(query): RawQuery) -> Response {
    let limit = query
        .as_deref()
        .and_then(|q| {
            q.split('&').find_map(|p| p.split_once('=')).and_then(|(k, v)| {
                (k == "limit").then_some(v).and_then(|v| v.parse::<usize>().ok())
            })
        })
        .unwrap_or(100);
    let data = qoder_state().data.lock().unwrap();
    json_ok(serde_json::to_value(data.list_logs(limit)).unwrap_or(json!([])))
}

async fn api_qoder_clear_logs() -> Response {
    let state = qoder_state();
    let r = {
        let mut data = state.data.lock().unwrap();
        data.clear_logs();
        data.save(&state.store_file())
    };
    match r {
        Ok(()) => json_ok(json!(true)),
        Err(e) => json_err(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}

async fn api_qoder_get_settings() -> Response {
    let s = qoder_state().data.lock().unwrap().get_settings();
    json_ok(serde_json::to_value(s).unwrap_or(json!({})))
}

async fn api_qoder_save_settings(Json(body): Json<Value>) -> Response {
    // 前端发送 {settings: partial};兼容直接平铺的 partial
    let payload = body.get("settings").cloned().unwrap_or(body);
    let partial: credit_core::models::PartialSettings = match serde_json::from_value(payload) {
        Ok(p) => p,
        Err(e) => return json_err(format!("设置格式错误: {e}"), StatusCode::BAD_REQUEST),
    };
    let state = qoder_state();
    let r = {
        let mut data = state.data.lock().unwrap();
        let s = data.save_settings(partial);
        data.save(&state.store_file()).map(|_| s)
    };
    match r {
        Ok(s) => {
            // 设置变更后重启自动领取调度(与桌面端同语义)
            credit_start_scheduler();
            json_ok(serde_json::to_value(s).unwrap_or(json!({})))
        }
        Err(e) => json_err(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}

async fn api_qoder_next_run() -> Response {
    let auto = qoder_state()
        .data
        .lock()
        .unwrap()
        .get_settings()
        .auto_claim_enabled;
    let next = if auto { credit_next_run_rfc3339() } else { None };
    json_ok(json!(next))
}

// ----- ZCode handlers -----

async fn api_zcode_accounts() -> Response {
    let data = zcode_state().data.lock().unwrap();
    json_ok(serde_json::to_value(data.get_accounts()).unwrap_or(json!([])))
}

async fn api_zcode_import_local() -> Response {
    // 读本机 ~/.zcode/v2 凭据 + 解密身份是阻塞 IO,放 blocking 线程
    let snapshot =
        match tokio::task::spawn_blocking(credit_core::zcode::credentials::read_local_snapshot)
            .await
        {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => return json_err(e.to_string(), StatusCode::BAD_REQUEST),
            Err(e) => return json_err(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
        };
    let hash = credit_core::zcode::credentials::canonical_hash(&snapshot.creds);
    let state = zcode_state();
    let account = credit_core::models::ZCodeAccount {
        id: credit_core::store::generate_id(),
        name: snapshot.label.clone(),
        // 本机导入:token 存可读标记,真实凭据在 credentials 快照
        token: format!(
            "zcode-creds:{}",
            snapshot.identity.user_id.clone().unwrap_or_default()
        ),
        user_id: snapshot.identity.user_id.clone(),
        email: snapshot.identity.email.clone(),
        region: "intl".into(),
        source: "local-app".into(),
        created_at: config::now_ms(),
        credentials: Some(snapshot.creds.clone()),
        config: snapshot.config.clone(),
        device_mid: snapshot.device_mid.clone(),
        canonical_hash: Some(hash.clone()),
        enabled: true,
        ..Default::default()
    };
    let r = {
        let mut data = state.data.lock().unwrap();
        // 同一登录(canonical_hash 一致)不重复导入
        if data
            .get_accounts()
            .iter()
            .any(|a| a.canonical_hash.as_deref() == Some(hash.as_str()))
        {
            return json_ok(json!({ "imported": 0, "skipped": 1, "errors": [] }));
        }
        data.accounts.push(account);
        data.save(&state.store_file())
    };
    match r {
        Ok(()) => json_ok(json!({ "imported": 1, "skipped": 0, "errors": [] })),
        Err(e) => json_err(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}

async fn api_zcode_add_account(Json(body): Json<Value>) -> Response {
    let token = match body_str(&body, "token") {
        Some(t) if !t.trim().is_empty() => t,
        _ => return credit_missing("token"),
    };
    let account = credit_core::models::ZCodeAccount {
        id: credit_core::store::generate_id(),
        name: body_str(&body, "name").unwrap_or_else(|| "ZCode 账号".into()),
        token: token.trim().into(),
        region: "intl".into(),
        source: "manual".into(),
        created_at: config::now_ms(),
        enabled: body.get("enabled").and_then(Value::as_bool).unwrap_or(true),
        ..Default::default()
    };
    let state = zcode_state();
    let r = {
        let mut data = state.data.lock().unwrap();
        data.accounts.push(account.clone());
        data.save(&state.store_file())
    };
    match r {
        Ok(()) => json_ok(serde_json::to_value(account).unwrap_or(json!(null))),
        Err(e) => json_err(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}

async fn api_zcode_update_account(Json(body): Json<Value>) -> Response {
    let Some(id) = body_str(&body, "id") else {
        return credit_missing("id");
    };
    // 兼容两种形态:{id, updates:{...}} 或把更新字段直接平铺在 body 里
    let updates = body.get("updates").cloned().unwrap_or_else(|| body.clone());
    let state = zcode_state();
    let mut data = state.data.lock().unwrap();
    let Some(acc) = data.update_account(&id, updates) else {
        return credit_not_found(&id);
    };
    match data.save(&state.store_file()).map(|_| acc) {
        Ok(acc) => json_ok(serde_json::to_value(acc).unwrap_or(json!(null))),
        Err(e) => json_err(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}

async fn api_zcode_delete_account(Json(body): Json<Value>) -> Response {
    let Some(id) = body_str(&body, "id") else {
        return credit_missing("id");
    };
    let state = zcode_state();
    let r = {
        let mut data = state.data.lock().unwrap();
        if data.get_accounts().iter().all(|a| a.id != id) {
            return credit_not_found(&id);
        }
        data.delete_account(&id);
        data.save(&state.store_file())
    };
    match r {
        Ok(()) => json_ok(json!({ "ok": true })),
        Err(e) => json_err(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}

async fn api_zcode_claim(Json(body): Json<Value>) -> Response {
    let Some(id) = body_str(&body, "id") else {
        return credit_missing("id");
    };
    let Some(acc) = find_zcode_account(&id) else {
        return credit_not_found(&id);
    };
    // None = preview 为空或查询失败(协议口径:无事可做)
    match credit_core::zcode::claim::claim_one(zcode_state(), credit_client(), &acc).await {
        Some(o) => json_ok(zcode_outcome_json(&id, &o)),
        None => json_ok(json!({
            "id": id,
            "outcome": "no-activity",
            "message": "当前无可领取的活动",
            "claims": [],
        })),
    }
}

async fn api_zcode_claim_all() -> Response {
    let results = credit_core::zcode::claim::claim_all(zcode_state(), credit_client()).await;
    json_ok(Value::Array(
        results
            .iter()
            .map(|(id, o)| zcode_outcome_json(id, o))
            .collect(),
    ))
}

async fn api_zcode_quota(Json(body): Json<Value>) -> Response {
    let Some(id) = body_str(&body, "id") else {
        return credit_missing("id");
    };
    let Some(acc) = find_zcode_account(&id) else {
        return credit_not_found(&id);
    };
    credit_json(
        credit_core::zcode::claim::refresh_quota(zcode_state(), credit_client(), &acc).await,
    )
}

async fn api_zcode_logs(RawQuery(query): RawQuery) -> Response {
    let limit = query
        .as_deref()
        .and_then(|q| {
            q.split('&').find_map(|p| p.split_once('=')).and_then(|(k, v)| {
                (k == "limit").then_some(v).and_then(|v| v.parse::<usize>().ok())
            })
        })
        .unwrap_or(100);
    let data = zcode_state().data.lock().unwrap();
    json_ok(serde_json::to_value(data.list_logs(limit)).unwrap_or(json!([])))
}

async fn api_zcode_clear_logs() -> Response {
    let state = zcode_state();
    let r = {
        let mut data = state.data.lock().unwrap();
        data.clear_logs();
        data.save(&state.store_file())
    };
    match r {
        Ok(()) => json_ok(json!(true)),
        Err(e) => json_err(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}

async fn api_zcode_get_settings() -> Response {
    let s = zcode_state().data.lock().unwrap().get_settings();
    json_ok(serde_json::to_value(s).unwrap_or(json!({})))
}

async fn api_zcode_save_settings(Json(body): Json<Value>) -> Response {
    // 前端发送 {settings: partial};兼容直接平铺的 partial
    let payload = body.get("settings").cloned().unwrap_or(body);
    let partial: credit_core::models::PartialSettings = match serde_json::from_value(payload) {
        Ok(p) => p,
        Err(e) => return json_err(format!("设置格式错误: {e}"), StatusCode::BAD_REQUEST),
    };
    let state = zcode_state();
    let r = {
        let mut data = state.data.lock().unwrap();
        let s = data.save_settings(partial);
        data.save(&state.store_file()).map(|_| s)
    };
    match r {
        Ok(s) => {
            // 设置变更后重启自动领取调度(与桌面端同语义)
            credit_start_scheduler();
            json_ok(serde_json::to_value(s).unwrap_or(json!({})))
        }
        Err(e) => json_err(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}

async fn api_zcode_next_run() -> Response {
    let auto = zcode_state()
        .data
        .lock()
        .unwrap()
        .get_settings()
        .auto_claim_enabled;
    let next = if auto { credit_next_run_rfc3339() } else { None };
    json_ok(json!(next))
}

// ----- 灵犀 handlers(多用户签到;checkinUrl/Cookie 由用户抓包获取) -----

fn find_lingxi_account(id: &str) -> Option<credit_core::lingxi::LingxiAccount> {
    lingxi_state()
        .data
        .lock()
        .unwrap()
        .get_accounts()
        .iter()
        .find(|a| a.id == id)
        .cloned()
}

/// 灵犀签到结果序列化(LingxiCheckinOutcome 未实现 Serialize)。
fn lingxi_outcome_json(
    id: &str,
    o: &credit_core::lingxi::checkin::LingxiCheckinOutcome,
) -> Value {
    json!({
        "id": id,
        "outcome": o.outcome.as_str(),
        "message": o.message,
    })
}

async fn api_lingxi_accounts() -> Response {
    let data = lingxi_state().data.lock().unwrap();
    json_ok(serde_json::to_value(data.get_accounts()).unwrap_or(json!([])))
}

async fn api_lingxi_add_account(Json(body): Json<Value>) -> Response {
    let checkin_url = match body_str(&body, "checkinUrl") {
        Some(t) if !t.trim().is_empty() => t,
        _ => return credit_missing("checkinUrl"),
    };
    let cookie = match body_str(&body, "cookie") {
        Some(t) if !t.trim().is_empty() => t,
        _ => return credit_missing("cookie"),
    };
    let name = body_str(&body, "name").unwrap_or_else(|| "灵犀账号".into());
    let account = credit_core::lingxi::LingxiAccount {
        id: credit_core::store::generate_id(),
        name,
        checkin_url: checkin_url.trim().into(),
        cookie: cookie.trim().into(),
        created_at: config::now_ms(),
        enabled: body.get("enabled").and_then(Value::as_bool).unwrap_or(true),
        ..Default::default()
    };
    let state = lingxi_state();
    let r = {
        let mut data = state.data.lock().unwrap();
        data.accounts.push(account.clone());
        data.save(&state.store_file())
    };
    match r {
        Ok(()) => json_ok(serde_json::to_value(account).unwrap_or(json!(null))),
        Err(e) => json_err(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// POST /api/lingxi/accounts/import-local —— 从本机灵犀客户端导入当前登录态。
/// DPAPI 解密 + SQLite 读取是阻塞 IO,放 blocking 线程(与 qoder import-local 同款);
/// 相同 checkinUrl+Cookie 已存在则原样返回该账号(前端提示已导入)。
async fn api_lingxi_import_local(Json(body): Json<Value>) -> Response {
    // checkinUrl 允许为空:core 按灵犀主域(lingxi.wps.cn)匹配 Cookie,签到地址可后补
    let url_for_import = body_str(&body, "checkinUrl")
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let imported = match tokio::task::spawn_blocking(move || {
        credit_core::lingxi::local_import::import_from_local(&url_for_import)
    })
    .await
    {
        Ok(r) => r,
        Err(e) => return json_err(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    };
    let imported = match imported {
        Ok(v) => v,
        Err(e) => return json_err(e.to_string(), StatusCode::BAD_REQUEST),
    };
    let account_url = body_str(&body, "checkinUrl")
        .map(|s| s.trim().to_string())
        .unwrap_or_default(); // 空串合法(默认域导入),签到地址由用户后补
    let name = match body_str(&body, "name").map(|s| s.trim().to_string()) {
        Some(n) if !n.is_empty() => n,
        _ => format!("灵犀-{}", imported.host),
    };
    let state = lingxi_state();
    let r = {
        let mut data = state.data.lock().unwrap();
        if let Some(existing) = data
            .get_accounts()
            .iter()
            .find(|a| a.checkin_url == account_url && a.cookie == imported.cookie_header)
        {
            return json_ok(serde_json::to_value(existing).unwrap_or(json!(null)));
        }
        let account = credit_core::lingxi::LingxiAccount {
            id: credit_core::store::generate_id(),
            name,
            checkin_url: account_url,
            cookie: imported.cookie_header,
            created_at: config::now_ms(),
            enabled: true,
            ..Default::default()
        };
        data.accounts.push(account.clone());
        data.save(&state.store_file()).map(|_| account)
    };
    match r {
        Ok(acc) => json_ok(serde_json::to_value(acc).unwrap_or(json!(null))),
        Err(e) => json_err(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}

async fn api_lingxi_update_account(Json(body): Json<Value>) -> Response {
    let Some(id) = body_str(&body, "id") else {
        return credit_missing("id");
    };
    // 兼容两种形态:{id, updates:{...}} 或把更新字段直接平铺在 body 里
    let updates = body.get("updates").cloned().unwrap_or_else(|| body.clone());
    let state = lingxi_state();
    let mut data = state.data.lock().unwrap();
    let Some(acc) = data.update_account(&id, updates) else {
        return credit_not_found(&id);
    };
    match data.save(&state.store_file()).map(|_| acc) {
        Ok(acc) => json_ok(serde_json::to_value(acc).unwrap_or(json!(null))),
        Err(e) => json_err(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}

async fn api_lingxi_delete_account(Json(body): Json<Value>) -> Response {
    let Some(id) = body_str(&body, "id") else {
        return credit_missing("id");
    };
    let state = lingxi_state();
    let r = {
        let mut data = state.data.lock().unwrap();
        if data.get_accounts().iter().all(|a| a.id != id) {
            return credit_not_found(&id);
        }
        data.delete_account(&id);
        data.save(&state.store_file())
    };
    match r {
        Ok(()) => json_ok(json!({ "ok": true })),
        Err(e) => json_err(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}

async fn api_lingxi_checkin(Json(body): Json<Value>) -> Response {
    let Some(id) = body_str(&body, "id") else {
        return credit_missing("id");
    };
    let Some(acc) = find_lingxi_account(&id) else {
        return credit_not_found(&id);
    };
    let today = credit_core::schedule::shanghai_today(chrono::Utc::now());
    let o = credit_core::lingxi::checkin::checkin_one(lingxi_state(), credit_client(), &acc, &today)
        .await;
    json_ok(lingxi_outcome_json(&id, &o))
}

async fn api_lingxi_checkin_all() -> Response {
    let today = credit_core::schedule::shanghai_today(chrono::Utc::now());
    let results =
        credit_core::lingxi::checkin::checkin_all(lingxi_state(), credit_client(), &today).await;
    json_ok(Value::Array(
        results
            .iter()
            .map(|(id, o)| lingxi_outcome_json(id, o))
            .collect(),
    ))
}

async fn api_lingxi_logs(RawQuery(query): RawQuery) -> Response {
    let limit = query
        .as_deref()
        .and_then(|q| {
            q.split('&').find_map(|p| p.split_once('=')).and_then(|(k, v)| {
                (k == "limit").then_some(v).and_then(|v| v.parse::<usize>().ok())
            })
        })
        .unwrap_or(100);
    let data = lingxi_state().data.lock().unwrap();
    json_ok(serde_json::to_value(data.list_logs(limit)).unwrap_or(json!([])))
}

async fn api_lingxi_clear_logs() -> Response {
    let state = lingxi_state();
    let r = {
        let mut data = state.data.lock().unwrap();
        data.clear_logs();
        data.save(&state.store_file())
    };
    match r {
        Ok(()) => json_ok(json!(true)),
        Err(e) => json_err(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}

async fn api_lingxi_get_settings() -> Response {
    let s = lingxi_state().data.lock().unwrap().get_settings();
    json_ok(serde_json::to_value(s).unwrap_or(json!({})))
}

async fn api_lingxi_save_settings(Json(body): Json<Value>) -> Response {
    // 前端发送 {settings: partial};兼容直接平铺的 partial
    let payload = body.get("settings").cloned().unwrap_or(body);
    let partial: credit_core::lingxi::PartialLingxiSettings = match serde_json::from_value(payload)
    {
        Ok(p) => p,
        Err(e) => return json_err(format!("设置格式错误: {e}"), StatusCode::BAD_REQUEST),
    };
    let state = lingxi_state();
    let r = {
        let mut data = state.data.lock().unwrap();
        let mut merged = data.get_settings();
        merged.merge_partial(&partial);
        data.settings = merged.clone();
        data.save(&state.store_file()).map(|_| merged)
    };
    match r {
        Ok(s) => {
            // 设置变更后重启调度(与桌面端同语义:三平台任一有调度需求则继续跑)
            credit_start_scheduler();
            json_ok(serde_json::to_value(s).unwrap_or(json!({})))
        }
        Err(e) => json_err(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// 下次执行时间(RFC3339):按设置的时间点纯计算,Asia/Shanghai 当日未来最近,
/// 否则明日最早;列表为空/全非法 → null。
async fn api_lingxi_next_run() -> Response {
    let times = lingxi_state().data.lock().unwrap().get_settings().checkin_times;
    let next = credit_core::schedule::lingxi_next_run(&times, chrono::Utc::now())
        .map(|dt| dt.to_rfc3339());
    json_ok(json!(next))
}
