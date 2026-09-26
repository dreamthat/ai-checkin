// 灵犀子系统 Tauri 命令。核心逻辑在 credit-core(lingxi 模块:POST checkinUrl + Cookie,
// 响应文本三级判定),这里只做宿主薄层:状态 = credit-core 存储(~/.wb-switch/lingxi/)
// + 平台专属 reqwest::Client,与 TRAE/Qoder/ZCode 隔离。
// checkinUrl 与 Cookie 手动抓包获取;登录态也可从本机灵犀客户端一键导入(local_import)。

use tauri::{AppHandle, State};

use credit_core::error::{AppError, AppResult};
use credit_core::lingxi::{self, checkin::LingxiCheckinOutcome, LingxiAccount, Outcome, PartialLingxiSettings};
use credit_core::log::{LogEntry, LogStore};
use credit_core::schedule::{lingxi_next_run, shanghai_today};
use credit_core::store;

use crate::credit_scheduler;

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// 灵犀子系统宿主状态(tauri::manage):core 存储状态 + 独立 HTTP 客户端。
pub struct LingxiState {
    pub core: store::LingxiState,
    pub client: reqwest::Client,
}

/// 单账号签到结果的 IPC 视图(Outcome 已实现 Serialize,camelCase 对齐前端)。
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LingxiCheckinView {
    pub outcome: Outcome,
    pub message: String,
}

impl From<LingxiCheckinOutcome> for LingxiCheckinView {
    fn from(o: LingxiCheckinOutcome) -> Self {
        Self { outcome: o.outcome, message: o.message }
    }
}

/// 一键签到结果项:账号 id + 签到结果(flatten 平铺给前端)。
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LingxiCheckinAllItem {
    pub account_id: String,
    #[serde(flatten)]
    pub result: LingxiCheckinView,
}

// ===== 账号 =====

#[tauri::command]
pub fn lingxi_get_accounts(state: State<'_, LingxiState>) -> Vec<LingxiAccount> {
    let data = state.core.data.lock().unwrap();
    data.get_accounts().to_vec()
}

/// 手动添加账号(name / checkinUrl / cookie 均由用户从浏览器抓包获取)。
#[tauri::command]
pub fn lingxi_add_account(
    state: State<'_, LingxiState>,
    name: String,
    checkin_url: String,
    cookie: String,
) -> AppResult<LingxiAccount> {
    let checkin_url = checkin_url.trim().to_string();
    let cookie = cookie.trim().to_string();
    if checkin_url.is_empty() {
        return Err(AppError::Credential("checkinUrl 不能为空".into()));
    }
    if cookie.is_empty() {
        return Err(AppError::Credential("cookie 不能为空".into()));
    }
    let trimmed = name.trim();
    let account = LingxiAccount {
        id: store::generate_id(),
        name: if trimmed.is_empty() { "灵犀账号".into() } else { trimmed.to_string() },
        checkin_url,
        cookie,
        enabled: true,
        created_at: now_ms(),
        ..Default::default()
    };
    {
        let mut data = state.core.data.lock().unwrap();
        data.accounts.push(account.clone());
    }
    state.core.save()?;
    Ok(account)
}

#[tauri::command]
pub fn lingxi_update_account(
    id: String,
    updates: serde_json::Value,
    state: State<'_, LingxiState>,
) -> AppResult<LingxiAccount> {
    let mut data = state.core.data.lock().unwrap();
    let updated = data
        .update_account(&id, updates)
        .ok_or_else(|| AppError::NotFound(id.clone()))?;
    data.save(&state.core.store_file())?;
    Ok(updated)
}

#[tauri::command]
pub fn lingxi_delete_account(id: String, state: State<'_, LingxiState>) -> AppResult<bool> {
    let mut data = state.core.data.lock().unwrap();
    if !data.get_accounts().iter().any(|a| a.id == id) {
        return Err(AppError::NotFound(id));
    }
    data.delete_account(&id);
    data.save(&state.core.store_file())?;
    Ok(true)
}

/// 从本机导入请求体(camelCase 对齐前端)。
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LingxiImportLocalBody {
    pub checkin_url: String,
    pub name: Option<String>,
}

/// 从本机 WPS 灵犀客户端导入当前登录态:core 解密 Cookies SQLite → upsert 账号。
/// DPAPI + SQLite 是阻塞 IO,放 blocking 线程;相同 checkinUrl+Cookie 已存在则原样返回。
#[tauri::command]
pub async fn lingxi_import_local(
    body: LingxiImportLocalBody,
    state: State<'_, LingxiState>,
) -> AppResult<LingxiAccount> {
    let checkin_url = body.checkin_url.trim().to_string();
    if checkin_url.is_empty() {
        return Err(AppError::Credential("checkinUrl 不能为空".into()));
    }
    let url_for_import = checkin_url.clone();
    let imported = tokio::task::spawn_blocking(move || {
        credit_core::lingxi::local_import::import_from_local(&url_for_import)
    })
    .await
    .map_err(|e| AppError::Credential(format!("导入任务失败: {e}")))?;
    let imported = imported?;

    let name = {
        let trimmed = body.name.as_deref().unwrap_or("").trim();
        if trimmed.is_empty() {
            format!("灵犀-{}", imported.host)
        } else {
            trimmed.to_string()
        }
    };
    let mut data = state.core.data.lock().unwrap();
    if let Some(existing) = data
        .get_accounts()
        .iter()
        .find(|a| a.checkin_url == checkin_url && a.cookie == imported.cookie_header)
    {
        return Ok(existing.clone()); // 已导入过:不重复建号,前端提示已存在
    }
    let account = LingxiAccount {
        id: store::generate_id(),
        name,
        checkin_url,
        cookie: imported.cookie_header,
        enabled: true,
        created_at: now_ms(),
        ..Default::default()
    };
    data.accounts.push(account.clone());
    drop(data);
    state.core.save()?;
    Ok(account)
}

// ===== 签到 =====

/// 单账号签到(今日 = Asia/Shanghai 本地日期;幂等与判定在 core)。
#[tauri::command]
pub async fn lingxi_checkin_account(
    id: String,
    state: State<'_, LingxiState>,
) -> AppResult<LingxiCheckinView> {
    let account = {
        let data = state.core.data.lock().unwrap();
        data.get_accounts().iter().find(|a| a.id == id).cloned()
    }
    .ok_or_else(|| AppError::NotFound(id.clone()))?;
    let today = shanghai_today(chrono::Utc::now());
    let outcome = lingxi::checkin::checkin_one(&state.core, &state.client, &account, &today).await;
    Ok(outcome.into())
}

/// 一键签到(全部启用账号;当日已成功的账号返回 skipped)。
/// 注:含引用参数的 async 命令必须返回 Result(Tauri 约定,与 qoder_commands 一致)。
#[tauri::command]
pub async fn lingxi_checkin_all(
    state: State<'_, LingxiState>,
) -> AppResult<Vec<LingxiCheckinAllItem>> {
    let today = shanghai_today(chrono::Utc::now());
    Ok(lingxi::checkin::checkin_all(&state.core, &state.client, &today)
        .await
        .into_iter()
        .map(|(account_id, o)| LingxiCheckinAllItem { account_id, result: o.into() })
        .collect())
}

// ===== 日志 / 设置 / 调度 =====

#[tauri::command]
pub fn lingxi_get_logs(limit: Option<usize>, state: State<'_, LingxiState>) -> Vec<LogEntry> {
    let data = state.core.data.lock().unwrap();
    data.list_logs(limit.unwrap_or(100))
}

#[tauri::command]
pub fn lingxi_clear_logs(state: State<'_, LingxiState>) -> AppResult<bool> {
    let mut data = state.core.data.lock().unwrap();
    data.clear_logs();
    data.save(&state.core.store_file())?;
    Ok(true)
}

#[tauri::command]
pub fn lingxi_get_settings(state: State<'_, LingxiState>) -> lingxi::LingxiSettings {
    let data = state.core.data.lock().unwrap();
    data.get_settings()
}

/// 保存灵犀设置(checkinTimes 过滤非法格式;保存后与其他平台同款重启调度循环)。
#[tauri::command]
pub fn lingxi_save_settings(
    settings: PartialLingxiSettings,
    state: State<'_, LingxiState>,
    app: AppHandle,
) -> AppResult<lingxi::LingxiSettings> {
    let s = {
        let mut data = state.core.data.lock().unwrap();
        let mut merged = data.get_settings();
        merged.merge_partial(&settings);
        data.settings = merged.clone();
        data.save(&state.core.store_file())?;
        merged
    };
    // 设置变更后重建调度:任一平台有调度需求则重启循环,都无则停止
    // (均以 generation 计数使旧任务退出,同 qoder/zcode_save_settings 模式)。
    if credit_scheduler::any_auto_enabled(&app) {
        credit_scheduler::start_scheduler(app);
    } else {
        credit_scheduler::stop_scheduler(&app);
    }
    Ok(s)
}

/// 下次执行时间(ISO8601;按设置的时间点纯计算,Asia/Shanghai 当日未来最近,否则明日最早)。
#[tauri::command]
pub fn lingxi_get_next_run_time(state: State<'_, LingxiState>) -> Option<String> {
    let times = state.core.data.lock().unwrap().get_settings().checkin_times;
    lingxi_next_run(&times, chrono::Utc::now()).map(|dt| dt.to_rfc3339())
}
