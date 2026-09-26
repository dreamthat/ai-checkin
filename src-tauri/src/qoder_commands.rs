// Qoder 子系统 Tauri 命令(方案见 .trae/documents/merge-creditdaddy-qoder-zcode.md)。
// 核心逻辑在 credit-core(领取编排 checkin.rs + 设备风控身份 identity.rs),这里只做宿主薄层:
// 状态 = credit-core 存储(~/.wb-switch/qoder/)+ 平台专属 reqwest::Client,与 TRAE/主功能隔离。
// 本地凭据 DPAPI 解密仅 Windows 可用(core 内已 stub,非 Windows 返回友好错误,仍可手动录入 token)。

use tauri::{AppHandle, State};

use credit_core::error::{AppError, AppResult};
use credit_core::log::{LogEntry, LogStore};
use credit_core::models::{ClaimOutcome, PartialSettings, QoderAccount, QoderSettings};
use credit_core::qoder;
use credit_core::store;

use crate::credit_scheduler;

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Qoder 子系统宿主状态(tauri::manage):core 存储状态 + 独立 HTTP 客户端。
pub struct QoderState {
    pub core: store::QoderState,
    pub client: reqwest::Client,
}

/// 单账号领取结果的 IPC 视图(core 的 QoderCheckinOutcome 未实现 Serialize,在此对齐 camelCase)。
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QoderCheckinView {
    pub outcome: ClaimOutcome,
    pub message: String,
    /// 本次领取到的 Credits 总额
    pub claimed_amount: i64,
    pub user_id: Option<String>,
    /// 本次是否用上了设备风控身份(国际版每日活动的关键)
    pub risk: bool,
}

impl From<qoder::checkin::QoderCheckinOutcome> for QoderCheckinView {
    fn from(o: qoder::checkin::QoderCheckinOutcome) -> Self {
        Self {
            outcome: o.outcome,
            message: o.message,
            claimed_amount: o.claimed_amount,
            user_id: o.uid,
            risk: o.risk,
        }
    }
}

/// 一键签到结果项:账号 id + 领取结果(flatten 平铺给前端)。
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QoderCheckinAllItem {
    pub account_id: String,
    #[serde(flatten)]
    pub result: QoderCheckinView,
}

/// 本地导入报告:成功导入/更新的账号 + 客户端读取错误(不致命,逐项展示)。
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QoderImportReport {
    pub accounts: Vec<QoderAccount>,
    pub errors: Vec<String>,
}

// ===== 账号 =====

#[tauri::command]
pub fn qoder_get_accounts(state: State<'_, QoderState>) -> Vec<QoderAccount> {
    let data = state.core.data.lock().unwrap();
    data.get_accounts().to_vec()
}

/// 导入本机 Qoder 客户端登录的账号(Win DPAPI 解 auth.v1.dat,国际版/国内版各一个)。
/// 按 user_id(缺失时按 token)去重:命中则只更新凭据,不改动用户改过的 name。
#[tauri::command]
pub fn qoder_import_local(state: State<'_, QoderState>) -> AppResult<QoderImportReport> {
    let (locals, errors) = qoder::identity::read_app_accounts();
    let mut imported = Vec::new();
    if !locals.is_empty() {
        let mut data = state.core.data.lock().unwrap();
        for local in locals {
            let region = if local.provider == "qoder-cn" { "cn" } else { "intl" };
            let existing = data
                .get_accounts()
                .iter()
                .find(|a| match (&a.user_id, &local.user_id) {
                    (Some(x), Some(y)) => x == y,
                    _ => a.token == local.token,
                })
                .map(|a| a.id.clone());
            let updates = serde_json::json!({
                "token": local.token,
                "refreshToken": local.refresh_token,
                "expiresAt": local.expires_at,
                "userId": local.user_id,
                "email": local.user_email,
                "region": region,
                "source": "local-app",
            });
            let account = match existing {
                Some(id) => data.update_account(&id, updates),
                None => {
                    let id = store::generate_id();
                    let name = local
                        .user_name
                        .clone()
                        .or_else(|| local.user_email.clone())
                        .unwrap_or_else(|| "Qoder 账号".into());
                    data.accounts.push(QoderAccount {
                        id: id.clone(),
                        name,
                        enabled: true,
                        created_at: now_ms(),
                        ..Default::default()
                    });
                    data.update_account(&id, updates)
                }
            };
            if let Some(a) = account {
                imported.push(a);
            }
        }
        drop(data);
    }
    state.core.save()?;
    Ok(QoderImportReport { accounts: imported, errors })
}

/// 手动录入 token 账号。region 缺省取设置里的"新账号默认区域"。
#[tauri::command]
pub fn qoder_add_account(
    state: State<'_, QoderState>,
    name: String,
    token: String,
    region: Option<String>,
    enabled: Option<bool>,
) -> AppResult<QoderAccount> {
    let token = token.trim().to_string();
    if token.is_empty() {
        return Err(AppError::Credential("token 不能为空".into()));
    }
    let default_region = state.core.data.lock().unwrap().get_settings().region;
    let region = region.filter(|r| r == "intl" || r == "cn").unwrap_or(default_region);
    let trimmed = name.trim();
    let account = QoderAccount {
        id: store::generate_id(),
        name: if trimmed.is_empty() { "Qoder 账号".into() } else { trimmed.to_string() },
        token,
        region,
        enabled: enabled.unwrap_or(true),
        created_at: now_ms(),
        source: "manual".into(),
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
pub fn qoder_update_account(
    id: String,
    updates: serde_json::Value,
    state: State<'_, QoderState>,
) -> AppResult<QoderAccount> {
    let mut data = state.core.data.lock().unwrap();
    let updated = data
        .update_account(&id, updates)
        .ok_or_else(|| AppError::NotFound(id.clone()))?;
    data.save(&state.core.store_file())?;
    Ok(updated)
}

#[tauri::command]
pub fn qoder_delete_account(id: String, state: State<'_, QoderState>) -> AppResult<bool> {
    let mut data = state.core.data.lock().unwrap();
    if !data.get_accounts().iter().any(|a| a.id == id) {
        return Err(AppError::NotFound(id));
    }
    data.delete_account(&id);
    data.save(&state.core.store_file())?;
    Ok(true)
}

// ===== 签到 / 额度 =====

/// 单账号签到(领取所有可领活动;状态机与回写在 core)。
#[tauri::command]
pub async fn qoder_checkin_account(
    id: String,
    state: State<'_, QoderState>,
) -> AppResult<QoderCheckinView> {
    let account = {
        let data = state.core.data.lock().unwrap();
        data.get_accounts().iter().find(|a| a.id == id).cloned()
    }
    .ok_or_else(|| AppError::NotFound(id.clone()))?;
    let outcome = qoder::checkin::checkin_one(&state.core, &state.client, &account).await;
    Ok(outcome.into())
}

/// 一键签到(全部启用账号;含国际版"每台设备每天限领一次"的归并记忆,见 core checkin_all)。
/// 注:含引用参数的 async 命令必须返回 Result(Tauri 约定,与 trae_commands 一致)。
#[tauri::command]
pub async fn qoder_checkin_all(
    state: State<'_, QoderState>,
) -> AppResult<Vec<QoderCheckinAllItem>> {
    Ok(qoder::checkin::checkin_all(&state.core, &state.client)
        .await
        .into_iter()
        .map(|(account_id, o)| QoderCheckinAllItem { account_id, result: o.into() })
        .collect())
}

/// 实时查询账号额度(/api/v2/quota/usage 原始 JSON,写回账号 quota 缓存)。
#[tauri::command]
pub async fn qoder_get_account_quota(
    id: String,
    state: State<'_, QoderState>,
) -> AppResult<serde_json::Value> {
    let account = {
        let data = state.core.data.lock().unwrap();
        data.get_accounts().iter().find(|a| a.id == id).cloned()
    }
    .ok_or_else(|| AppError::NotFound(id.clone()))?;
    qoder::checkin::refresh_quota(&state.core, &state.client, &account).await
}

// ===== 日志 / 设置 / 调度 =====

#[tauri::command]
pub fn qoder_get_logs(limit: Option<usize>, state: State<'_, QoderState>) -> Vec<LogEntry> {
    let data = state.core.data.lock().unwrap();
    data.list_logs(limit.unwrap_or(100))
}

#[tauri::command]
pub fn qoder_clear_logs(state: State<'_, QoderState>) -> AppResult<bool> {
    let mut data = state.core.data.lock().unwrap();
    data.clear_logs();
    data.save(&state.core.store_file())?;
    Ok(true)
}

#[tauri::command]
pub fn qoder_get_settings(state: State<'_, QoderState>) -> QoderSettings {
    let data = state.core.data.lock().unwrap();
    data.get_settings()
}

#[tauri::command]
pub fn qoder_save_settings(
    settings: PartialSettings,
    state: State<'_, QoderState>,
    app: AppHandle,
) -> AppResult<QoderSettings> {
    let mut data = state.core.data.lock().unwrap();
    let s = data.save_settings(settings);
    data.save(&state.core.store_file())?;
    drop(data);
    // 设置变更后重建调度:任一平台开启自动领取则重启循环,两平台都关则停止
    // (均以 generation 计数使旧任务退出,同 trae_scheduler 模式)。
    if credit_scheduler::any_auto_enabled(&app) {
        credit_scheduler::start_scheduler(app);
    } else {
        credit_scheduler::stop_scheduler(&app);
    }
    Ok(s)
}

#[tauri::command]
pub fn qoder_get_next_run_time(state: State<'_, QoderState>, app: AppHandle) -> Option<String> {
    let enabled = state.core.data.lock().unwrap().get_settings().auto_claim_enabled;
    credit_scheduler::get_next_run_time(&app, enabled)
}
