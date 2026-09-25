// TRAE 子系统 Tauri 命令(自 trae-mate commands.rs 平移,统一加 trae_ 前缀)。
// 状态来自 app.manage 的 TraeState(数据根 ~/.wb-switch/trae/)与共享 reqwest::Client,
// 与 workbuddy 主功能(commands.rs)完全独立。核心逻辑在 trae-core,非 Windows 由
// core 内 stub 返回友好错误,保证全平台编译通过。

use std::path::PathBuf;

use tauri::{AppHandle, Emitter, Manager, State};

use trae_core::accounts;
use trae_core::error::{AppError, AppResult};
use trae_core::models::{
    AppSettings, CheckinLog, CheckinResult, LaunchResult, PartialAppSettings, PointsResult,
    PublicAccount,
};
use trae_core::store::TraeState;
use trae_core::{cooldown, credits, device_map, fs_utils, jwt, trae_instance, trae_machine};

use crate::trae_scheduler;

// ===== 通知适配:把 trae-core 的 CheckinNotifier 桥接到 Tauri 事件 =====

/// 桌面端通知者:转发 trae-checkin-start/progress/done 事件给前端。
pub struct AppNotifier(pub AppHandle);

impl trae_core::checkin::CheckinNotifier for AppNotifier {
    fn emit(&self, event: &str, payload: serde_json::Value) {
        let _ = self.0.emit(event, payload);
    }
}

/// 通知托盘 TRAE 账号集变化(导入/删除/启动实例)。桌面端由 tray.rs 重建菜单。
fn notify_tray(app: &AppHandle) {
    #[cfg(desktop)]
    crate::tray::on_trae_accounts_changed(app);
    #[cfg(not(desktop))]
    let _ = app;
}

// ===== 账号 =====

#[tauri::command]
pub fn trae_get_accounts(state: State<'_, TraeState>) -> Vec<PublicAccount> {
    trae_core::views::build_account_views(&state)
}

/// 导入当前 TRAE 桌面账号:核心逻辑已下沉 trae_core::accounts
#[tauri::command]
pub fn trae_import_desktop_account(state: State<'_, TraeState>) -> AppResult<PublicAccount> {
    accounts::import_desktop(&state)
}

#[tauri::command]
pub fn trae_update_account(
    id: String,
    updates: serde_json::Value,
    state: State<'_, TraeState>,
) -> AppResult<PublicAccount> {
    accounts::update(&state, &id, updates)
}

#[tauri::command]
pub fn trae_delete_account(id: String, state: State<'_, TraeState>, app: AppHandle) -> AppResult<bool> {
    // 关闭实例/清理联动数据/删除账号均在核心层;托盘刷新是宿主职责
    let result = accounts::delete(&state, &id);
    if result.is_ok() {
        notify_tray(&app);
    }
    result
}

#[tauri::command]
pub async fn trae_checkin_account(
    id: String,
    state: State<'_, TraeState>,
    client: State<'_, reqwest::Client>,
    app: AppHandle,
) -> AppResult<CheckinResult> {
    let account = {
        let data = state.data.lock().unwrap();
        data.get_accounts().iter().find(|a| a.id == id).cloned()
    };
    let account = account.ok_or_else(|| AppError::NotFound(id.clone()))?;
    let notifier = AppNotifier(app);
    Ok(trae_core::checkin::perform_checkin(
        Some(&notifier),
        &account,
        client.inner(),
        state.inner(),
    )
    .await)
}

#[tauri::command]
pub async fn trae_checkin_all(
    state: State<'_, TraeState>,
    client: State<'_, reqwest::Client>,
    app: AppHandle,
) -> AppResult<Vec<(PublicAccount, CheckinResult)>> {
    // 手动一键签到:账号间保持 2s 快速执行(自动定时签到用可配置的分钟级间隔,见 trae_scheduler)
    let notifier = AppNotifier(app);
    Ok(trae_core::checkin::perform_all_checkin(
        Some(&notifier),
        client.inner(),
        state.inner(),
        2,
    )
    .await)
}

#[tauri::command]
pub async fn trae_get_account_points(
    id: String,
    state: State<'_, TraeState>,
    client: State<'_, reqwest::Client>,
) -> AppResult<PointsResult> {
    accounts::account_points(&state, client.inner(), &id).await
}

/// 手动录入 JWT 账号(参考模式):核心逻辑已下沉 trae_core::accounts
#[tauri::command]
pub fn trae_account_add_jwt(
    state: State<'_, TraeState>,
    name: String,
    jwt: String,
    refresh_token: Option<String>,
    enabled: Option<bool>,
) -> AppResult<PublicAccount> {
    accounts::add_jwt(&state, name, &jwt, refresh_token, enabled)
}

/// JWT 解析预览(弹窗实时显示 user_id / 剩余小时)
#[tauri::command]
pub fn trae_jwt_parse_preview(jwt: String) -> Result<jwt::JwtInfo, String> {
    let info = jwt::parse(&jwt);
    if info.user_id.is_none() {
        return Err("无法从 JWT 解析 user_id,请检查格式".into());
    }
    Ok(info)
}

/// 手动刷新 jwt 账号的 JWT(refresh_token -> ExchangeToken),返回更新后账号
#[tauri::command]
pub async fn trae_refresh_jwt_account(
    user_id: String,
    state: State<'_, TraeState>,
    client: State<'_, reqwest::Client>,
) -> AppResult<PublicAccount> {
    accounts::refresh_jwt(&state, client.inner(), &user_id).await
}

// ===== 冷却 / 设备 / 积分 =====

/// 清除单个账号冷却状态
#[tauri::command]
pub fn trae_cooldown_clear(state: State<'_, TraeState>, user_id: String) -> AppResult<()> {
    cooldown::clear_cooldown(&state, &user_id);
    Ok(())
}

/// 清除所有账号冷却状态
#[tauri::command]
pub fn trae_cooldown_clear_all(state: State<'_, TraeState>) -> AppResult<usize> {
    Ok(cooldown::clear_all_cooldowns(&state))
}

/// 重置某账号的伪设备身份(下次签到重新派生)
#[tauri::command]
pub fn trae_device_reset(state: State<'_, TraeState>, user_id: String) -> AppResult<()> {
    device_map::reset_device_for(&state, &user_id);
    Ok(())
}

/// 实时查询某账号剩余积分(写 remaining_credits.json 缓存)
#[tauri::command]
pub async fn trae_fetch_remaining_credits(
    user_id: String,
    state: State<'_, TraeState>,
    client: State<'_, reqwest::Client>,
) -> AppResult<f64> {
    accounts::fetch_remaining(&state, client.inner(), &user_id).await
}

/// 刷新所有账号剩余积分(含自动解冻)
#[tauri::command]
pub async fn trae_refresh_all_remaining_credits(
    state: State<'_, TraeState>,
    client: State<'_, reqwest::Client>,
) -> AppResult<usize> {
    Ok(credits::refresh_remaining_credits(&state, client.inner())
        .await
        .map_err(AppError::Credential)?)
}

/// 每日积分快照列表(积分看板三线趋势数据源)
#[tauri::command]
pub fn trae_credits_daily_list(
    state: State<'_, TraeState>,
) -> Vec<trae_core::models::CreditsDailySnapshot> {
    fs_utils::read_json(&state.data_path("credits_daily.json"))
}

/// 用系统默认浏览器打开外部链接
#[tauri::command]
pub fn trae_open_url(url: String) -> AppResult<()> {
    #[cfg(windows)]
    {
        std::process::Command::new("cmd")
            .args(["/c", "start", "", &url])
            .spawn()
            .map_err(|e| AppError::Launch(format!("打开链接失败: {e}")))?;
    }
    #[cfg(not(windows))]
    {
        let _ = url;
    }
    Ok(())
}

// ===== 日志 / 设置 / 调度 =====

#[tauri::command]
pub fn trae_get_logs(limit: Option<usize>, state: State<'_, TraeState>) -> Vec<CheckinLog> {
    let data = state.data.lock().unwrap();
    data.get_logs(limit.unwrap_or(100))
}

#[tauri::command]
pub fn trae_clear_logs(state: State<'_, TraeState>) -> AppResult<bool> {
    let mut data = state.data.lock().unwrap();
    data.clear_logs();
    data.save(&state.store_file())?;
    Ok(true)
}

#[tauri::command]
pub fn trae_get_settings(state: State<'_, TraeState>, app: AppHandle) -> AppSettings {
    let mut s = {
        let data = state.data.lock().unwrap();
        data.get_settings()
    };
    // 用插件真实状态覆盖(反映系统设置/任务管理器等外部修改)。
    // 合并后与 workbuddy 主设置的自启是同一个 OS 开关(同一 app),两处 UI 读取时
    // 都以插件状态为准,不会互相矛盾。
    use tauri_plugin_autostart::ManagerExt;
    if let Ok(enabled) = app.autolaunch().is_enabled() {
        s.launch_at_login = enabled;
    }
    s
}

#[tauri::command]
pub fn trae_save_settings(
    settings: PartialAppSettings,
    state: State<'_, TraeState>,
    app: AppHandle,
) -> AppResult<AppSettings> {
    let has_launch = settings.launch_at_login.is_some();
    let mut data = state.data.lock().unwrap();
    let s = data.save_settings(settings);
    data.save(&state.store_file())?;
    drop(data);
    // 同步开机自启插件状态(仅在本次提交了该字段时)
    if has_launch {
        use tauri_plugin_autostart::ManagerExt;
        let mgr = app.autolaunch();
        let currently = mgr.is_enabled().unwrap_or(false);
        if s.launch_at_login && !currently {
            let _ = mgr.enable();
        } else if !s.launch_at_login && currently {
            let _ = mgr.disable();
        }
    }
    // 设置变更后重启定时任务
    trae_scheduler::start_scheduler(app);
    Ok(s)
}

#[tauri::command]
pub fn trae_start_scheduler(app: AppHandle) -> bool {
    trae_scheduler::start_scheduler(app);
    true
}

#[tauri::command]
pub fn trae_stop_scheduler(app: AppHandle) -> bool {
    trae_scheduler::stop_scheduler(&app);
    true
}

#[tauri::command]
pub fn trae_get_next_run_time(app: AppHandle) -> Option<String> {
    trae_scheduler::get_next_run_time(&app)
}

// ===== 多开实例 =====

/// 启动账号独立实例(前端命令与托盘点击共用):核心逻辑已下沉 trae_instance::launch_account,
/// 这里只负责取配置目录与托盘刷新。
pub fn launch_account_by_id(app: &AppHandle, account_id: &str) -> AppResult<LaunchResult> {
    let state = app.state::<TraeState>();
    let config_dir = app
        .path()
        .app_config_dir()
        .map_err(|e| AppError::Launch(format!("获取配置目录失败: {e}")))?;
    let result = trae_instance::launch_account(state.inner(), &config_dir, account_id);
    // 启动成功后刷新托盘菜单:新实例已运行,状态前缀更新
    if result.is_ok() {
        notify_tray(app);
    }
    result
}

/// 启动账号独立实例(前端命令入口)
#[tauri::command]
pub fn trae_launch_account_multi(id: String, app: AppHandle) -> AppResult<LaunchResult> {
    launch_account_by_id(&app, &id)
}

/// 获取已保存的 TRAE exe 路径(未设置返回 None)
#[tauri::command]
pub fn trae_get_trae_exe_path(app: AppHandle) -> AppResult<Option<String>> {
    let config_dir = app
        .path()
        .app_config_dir()
        .map_err(|e| AppError::Launch(format!("获取配置目录失败: {e}")))?;
    trae_machine::get_saved_trae_path(&config_dir)
}

/// 手动设置 TRAE exe 路径
#[tauri::command]
pub fn trae_set_trae_exe_path(path: String, app: AppHandle) -> AppResult<()> {
    let config_dir = app
        .path()
        .app_config_dir()
        .map_err(|e| AppError::Launch(format!("获取配置目录失败: {e}")))?;
    trae_machine::save_trae_path(&config_dir, &path)
}

/// 自动扫描 TRAE exe 路径并保存
#[tauri::command]
pub fn trae_scan_trae_exe_path(app: AppHandle) -> AppResult<String> {
    let scanned = trae_machine::scan_trae_exe_path()?;
    let config_dir = app
        .path()
        .app_config_dir()
        .map_err(|e| AppError::Launch(format!("获取配置目录失败: {e}")))?;
    let _ = trae_machine::save_trae_path(&config_dir, &scanned);
    Ok(scanned)
}

/// 账号实例运行状态(返回前端)
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstanceState {
    pub running: bool,
    pub source: trae_machine::InstanceSource,
    pub is_main_account: bool,
}

/// 查询账号实例运行状态:未运行 / 主实例(用户手动启动的 TRAE)/ 工具实例(本应用启动的独立 data-dir)
#[tauri::command]
pub fn trae_get_account_instance_state(
    id: String,
    state: State<'_, TraeState>,
) -> AppResult<InstanceState> {
    let account = {
        let data = state.data.lock().unwrap();
        data.get_accounts()
            .iter()
            .find(|a| a.id == id)
            .cloned()
    }
    .ok_or_else(|| AppError::NotFound(id.clone()))?;
    let main = trae_machine::probe_main_instance();
    let source = trae_machine::account_state(&account, &main);
    let is_main_account = main.1.as_deref()
        == account
            .desktop_user_id
            .as_deref()
            .filter(|s| !s.is_empty());
    Ok(InstanceState {
        running: !matches!(source, trae_machine::InstanceSource::None),
        source,
        is_main_account,
    })
}

/// 聚焦账号实例窗口:工具实例运行聚焦其 data-dir 窗口,主实例运行聚焦主目录窗口,未运行则报错。
#[tauri::command]
pub fn trae_focus_account_instance(id: String, state: State<'_, TraeState>) -> AppResult<()> {
    let account = {
        let data = state.data.lock().unwrap();
        data.get_accounts()
            .iter()
            .find(|a| a.id == id)
            .cloned()
    }
    .ok_or_else(|| AppError::NotFound(id.clone()))?;
    let main = trae_machine::probe_main_instance();
    match trae_machine::account_state(&account, &main) {
        trae_machine::InstanceSource::Tool => {
            let d = account
                .data_dir
                .as_deref()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| AppError::Launch("该账号无工具实例数据目录".into()))?;
            trae_machine::focus_instance_window(d)
        }
        trae_machine::InstanceSource::Main => {
            let main_dir = trae_machine::main_data_dir()?;
            trae_machine::focus_instance_window(&main_dir.to_string_lossy())
        }
        trae_machine::InstanceSource::None => {
            Err(AppError::Launch("该账号尚未启动,无实例可聚焦".into()))
        }
    }
}

/// 打开新的空白 TRAE 实例供用户登录,后台轮询登录完成后自动导入账号:
/// 检测登录 -> 读凭据 -> 杀实例 -> 改名临时目录为标准 TRAE SOLO CN_{userId} -> upsert 账号绑定 -> emit 事件。
#[tauri::command]
pub fn trae_open_new_login_instance(app: AppHandle) -> AppResult<()> {
    let appdata = std::env::var("APPDATA")
        .map_err(|_| AppError::Launch("无法获取 APPDATA 环境变量".into()))?;
    let config_dir = app
        .path()
        .app_config_dir()
        .map_err(|e| AppError::Launch(format!("获取配置目录失败: {e}")))?;
    let exe_path = trae_machine::resolve_trae_path(&config_dir)?;

    // 临时 data-dir(带 uuid 后缀,避免与标准目录或多次操作冲突)
    let temp_dir = PathBuf::from(&appdata)
        .join(format!(
            "{} login {}",
            trae_machine::DATA_DIR_NAME,
            uuid::Uuid::new_v4()
        ))
        .to_string_lossy()
        .to_string();
    let shared_ext = PathBuf::from(&appdata)
        .join(trae_instance::SHARED_EXTENSIONS_DIR)
        .to_string_lossy()
        .to_string();

    // 启动空白实例(不写凭据,用户自行登录)
    trae_machine::open_product_with_data_dir(&exe_path, &temp_dir, Some(&shared_ext))?;

    // 后台轮询登录 + 导入,完成后 emit 事件
    let app_handle = app.clone();
    let appdata_owned = appdata;
    let temp_dir_owned = temp_dir;
    std::thread::spawn(move || {
        let result = wait_login_and_import(&app_handle, &appdata_owned, &temp_dir_owned);
        let _ = app_handle.emit("trae-login-imported", result);
    });
    Ok(())
}

/// 轮询临时实例登录态,登录后导入账号并改名目录。核心逻辑已下沉
/// trae_instance::import_logged_in_temp_dir,这里只负责托盘刷新。返回事件 payload。
fn wait_login_and_import(app: &AppHandle, appdata: &str, temp_dir: &str) -> serde_json::Value {
    let state = app.state::<TraeState>();
    let result = trae_instance::import_logged_in_temp_dir(state.inner(), appdata, temp_dir);
    if result.get("success").and_then(|v| v.as_bool()) == Some(true) {
        notify_tray(app);
    }
    result
}

/// 手动刷新账号凭证(卡片"刷新凭证"按钮):实例目录回读 + ExchangeToken 刷新 + 回写
#[tauri::command]
pub async fn trae_refresh_account_credential(
    id: String,
    state: State<'_, TraeState>,
    client: State<'_, reqwest::Client>,
) -> AppResult<PublicAccount> {
    let account = {
        let data = state.data.lock().unwrap();
        data.get_accounts().iter().find(|a| a.id == id).cloned()
    }
    .ok_or_else(|| AppError::NotFound(id.clone()))?;
    trae_core::checkin::refresh_account_credential(&account, client.inner(), state.inner()).await?;
    let acc = {
        let data = state.data.lock().unwrap();
        data.get_accounts()
            .iter()
            .find(|a| a.id == id)
            .cloned()
            .ok_or_else(|| AppError::NotFound(id.clone()))?
    };
    Ok(acc.into())
}

/// 扫描 %APPDATA% 下已存在的多开/登录临时目录,标注是否已绑定应用内账号
#[tauri::command]
pub fn trae_scan_instance_dirs(state: State<'_, TraeState>) -> Vec<trae_instance::InstanceDirInfo> {
    trae_instance::scan_bound_dirs(&state)
}

/// 从已有多开目录导入账号:核心逻辑已下沉 trae_core::accounts,这里补托盘刷新
#[tauri::command]
pub fn trae_import_account_from_dir(
    data_dir: String,
    state: State<'_, TraeState>,
    app: AppHandle,
) -> AppResult<PublicAccount> {
    let result = accounts::import_from_dir(&state, &data_dir);
    if result.is_ok() {
        notify_tray(&app);
    }
    result
}

// ===== 数据迁移 =====

/// 手动触发旧 TraeMate 数据迁移(自动迁移在启动时已静默执行,此命令用于重跑/查看报告)。
#[tauri::command]
pub fn trae_migrate_legacy_data(app: AppHandle) -> trae_core::migrate::MigrationReport {
    let config_dir = app.path().app_config_dir().ok();
    let trae_dir = wb_switch_core::modules::config::trae_dir();
    let _ = std::fs::create_dir_all(&trae_dir);
    trae_core::migrate::migrate_from_legacy(&trae_dir, config_dir.as_deref())
}
