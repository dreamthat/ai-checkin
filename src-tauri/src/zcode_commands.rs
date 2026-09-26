// ZCode 子系统 Tauri 命令(方案见 .trae/documents/merge-creditdaddy-qoder-zcode.md)。
// 核心逻辑在 credit-core(领取编排 claim.rs + 本机凭据 credentials.rs),这里只做宿主薄层:
// 状态 = credit-core 存储(~/.wb-switch/zcode/)+ 平台专属 reqwest::Client,与 TRAE/主功能隔离。
// ZCode 无签到语义,只有套餐领取(3007/3001 需验证码时诚实降级"需手动领取",见 core claim.rs)。

use tauri::{AppHandle, State};

use credit_core::error::{AppError, AppResult};
use credit_core::log::{LogEntry, LogStore};
use credit_core::models::{ClaimOutcome, PartialSettings, ZCodeAccount, ZcodeSettings};
use credit_core::store;
use credit_core::zcode::{self, client::QuotaSnapshot};

use crate::credit_scheduler;

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// ZCode 子系统宿主状态(tauri::manage):core 存储状态 + 独立 HTTP 客户端。
pub struct ZcodeState {
    pub core: store::ZcodeState,
    pub client: reqwest::Client,
}

/// 单账号领取结果的 IPC 视图(core 的 ZcodeClaimOutcome/PlanClaim 未实现 Serialize)。
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ZcodeClaimView {
    pub outcome: ClaimOutcome,
    pub message: String,
    /// 逐活动领取明细(面板展示"已领取:A、B"或"需手动领取"依据)
    pub claims: Vec<ZcodePlanClaimView>,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ZcodePlanClaimView {
    pub plan: String,
    pub ok: bool,
    pub already: bool,
    /// 需要验证码 → 无打码提供者,诚实降级"需手动领取"
    pub need_manual: bool,
    /// "direct" | ""(captcha 路径本层不接)
    pub via: String,
    pub message: Option<String>,
}

impl From<zcode::claim::ZcodeClaimOutcome> for ZcodeClaimView {
    fn from(o: zcode::claim::ZcodeClaimOutcome) -> Self {
        Self {
            outcome: o.outcome,
            message: o.message,
            claims: o
                .claims
                .into_iter()
                .map(|c| ZcodePlanClaimView {
                    plan: c.plan,
                    ok: c.ok,
                    already: c.already,
                    need_manual: c.need_manual,
                    via: c.via,
                    message: c.message,
                })
                .collect(),
        }
    }
}

/// 一键领取结果项:账号 id + 领取结果(flatten 平铺给前端)。
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ZcodeClaimAllItem {
    pub account_id: String,
    #[serde(flatten)]
    pub result: ZcodeClaimView,
}

/// 本地导入报告(单快照:ZCode 客户端同一时刻只有一个登录)。
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ZcodeImportReport {
    pub accounts: Vec<ZCodeAccount>,
}

// ===== 账号 =====

#[tauri::command]
pub fn zcode_get_accounts(state: State<'_, ZcodeState>) -> Vec<ZCodeAccount> {
    let data = state.core.data.lock().unwrap();
    data.get_accounts().to_vec()
}

/// 导入本机 ZCode 客户端登录账号(读 ~/.zcode/v2/credentials.json;enc:v1 字段保持密文,
/// 领取/额度时由 core 按账号快照解密)。按 canonical_hash / user_id 去重,命中则更新快照。
#[tauri::command]
pub fn zcode_import_local(state: State<'_, ZcodeState>) -> AppResult<ZcodeImportReport> {
    let snap = zcode::credentials::read_local_snapshot()?;
    let hash = zcode::credentials::canonical_hash(&snap.creds);
    let mut data = state.core.data.lock().unwrap();
    let existing = data
        .get_accounts()
        .iter()
        .find(|a| {
            a.canonical_hash.as_deref() == Some(hash.as_str())
                || a.user_id.is_some() && a.user_id == snap.identity.user_id
        })
        .map(|a| a.id.clone());
    let updates = serde_json::json!({
        "token": format!("zcode-creds:{}", snap.identity.user_id.clone().unwrap_or_default()),
        "userId": snap.identity.user_id,
        "email": snap.identity.email,
        "credentials": snap.creds,
        "config": snap.config,
        "deviceMid": snap.device_mid,
        "canonicalHash": hash,
        "source": "local-app",
    });
    let account = match existing {
        Some(id) => data.update_account(&id, updates),
        None => {
            let id = store::generate_id();
            data.accounts.push(ZCodeAccount {
                id: id.clone(),
                name: snap.label.clone(),
                region: "intl".into(),
                enabled: true,
                created_at: now_ms(),
                ..Default::default()
            });
            data.update_account(&id, updates)
        }
    };
    drop(data);
    state.core.save()?;
    Ok(ZcodeImportReport { accounts: account.into_iter().collect() })
}

/// 手动录入 token 账号(zcodejwttoken 或 API Key,由 core candidate/billing_tokens 统一解析)。
#[tauri::command]
pub fn zcode_add_account(
    state: State<'_, ZcodeState>,
    name: String,
    token: String,
    enabled: Option<bool>,
) -> AppResult<ZCodeAccount> {
    let token = token.trim().to_string();
    if token.is_empty() {
        return Err(AppError::Credential("token 不能为空".into()));
    }
    let trimmed = name.trim();
    let account = ZCodeAccount {
        id: store::generate_id(),
        name: if trimmed.is_empty() { "ZCode 账号".into() } else { trimmed.to_string() },
        token,
        region: "intl".into(),
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
pub fn zcode_update_account(
    id: String,
    updates: serde_json::Value,
    state: State<'_, ZcodeState>,
) -> AppResult<ZCodeAccount> {
    let mut data = state.core.data.lock().unwrap();
    let updated = data
        .update_account(&id, updates)
        .ok_or_else(|| AppError::NotFound(id.clone()))?;
    data.save(&state.core.store_file())?;
    Ok(updated)
}

#[tauri::command]
pub fn zcode_delete_account(id: String, state: State<'_, ZcodeState>) -> AppResult<bool> {
    let mut data = state.core.data.lock().unwrap();
    if !data.get_accounts().iter().any(|a| a.id == id) {
        return Err(AppError::NotFound(id));
    }
    data.delete_account(&id);
    data.save(&state.core.store_file())?;
    Ok(true)
}

// ===== 领取 / 额度 =====

/// 单账号领取。None = 当前无可领取活动(preview 为空或查询失败;核心协议口径:不留痕迹)。
#[tauri::command]
pub async fn zcode_claim_account(
    id: String,
    state: State<'_, ZcodeState>,
) -> AppResult<Option<ZcodeClaimView>> {
    let account = {
        let data = state.core.data.lock().unwrap();
        data.get_accounts().iter().find(|a| a.id == id).cloned()
    }
    .ok_or_else(|| AppError::NotFound(id.clone()))?;
    Ok(zcode::claim::claim_one(&state.core, &state.client, &account)
        .await
        .map(Into::into))
}

/// 一键领取(全部启用账号;preview 为空/查询失败的账号不产生结果项)。
/// 注:含引用参数的 async 命令必须返回 Result(Tauri 约定,与 trae_commands 一致)。
#[tauri::command]
pub async fn zcode_claim_all(state: State<'_, ZcodeState>) -> AppResult<Vec<ZcodeClaimAllItem>> {
    Ok(zcode::claim::claim_all(&state.core, &state.client)
        .await
        .into_iter()
        .map(|(account_id, o)| ZcodeClaimAllItem { account_id, result: o.into() })
        .collect())
}

/// 实时查询账号额度(先 BigModel Coding Plan,再 Z.ai / Start Plan;写回账号 quota 缓存)。
#[tauri::command]
pub async fn zcode_get_account_quota(
    id: String,
    state: State<'_, ZcodeState>,
) -> AppResult<QuotaSnapshot> {
    let account = {
        let data = state.core.data.lock().unwrap();
        data.get_accounts().iter().find(|a| a.id == id).cloned()
    }
    .ok_or_else(|| AppError::NotFound(id.clone()))?;
    zcode::claim::refresh_quota(&state.core, &state.client, &account).await
}

// ===== 日志 / 设置 / 调度 =====

#[tauri::command]
pub fn zcode_get_logs(limit: Option<usize>, state: State<'_, ZcodeState>) -> Vec<LogEntry> {
    let data = state.core.data.lock().unwrap();
    data.list_logs(limit.unwrap_or(100))
}

#[tauri::command]
pub fn zcode_clear_logs(state: State<'_, ZcodeState>) -> AppResult<bool> {
    let mut data = state.core.data.lock().unwrap();
    data.clear_logs();
    data.save(&state.core.store_file())?;
    Ok(true)
}

#[tauri::command]
pub fn zcode_get_settings(state: State<'_, ZcodeState>) -> ZcodeSettings {
    let data = state.core.data.lock().unwrap();
    data.get_settings()
}

#[tauri::command]
pub fn zcode_save_settings(
    settings: PartialSettings,
    state: State<'_, ZcodeState>,
    app: AppHandle,
) -> AppResult<ZcodeSettings> {
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
pub fn zcode_get_next_run_time(state: State<'_, ZcodeState>, app: AppHandle) -> Option<String> {
    let enabled = state.core.data.lock().unwrap().get_settings().auto_claim_enabled;
    credit_scheduler::get_next_run_time(&app, enabled)
}
