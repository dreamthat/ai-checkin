// 统一签到引擎。移植自参考项目 auto_checkin.py 的处理模式(Rust 内移植):
// 解析凭据(desktop 实例回读 / jwt 存储) → 稳定伪设备ID → 完整请求头 → status/claim
// → 错误分类冷却状态机 → 积分历史。直连 api.trae.cn(reqwest 默认不读系统代理,天然绕过本地代理)。

use std::time::Duration;

use serde_json::{json, Value};

use crate::cooldown;
use crate::credentials::{decrypt_credential, decrypt_jwt_secret, encrypt_credential, encrypt_jwt_secret};
use crate::error::{AppError, AppResult};
use crate::models::{
    credential_status, Account, CheckinLog, CheckinResult, Credential, PointsResult, PublicAccount,
};
use crate::store::{effective_user_id, generate_id, TraeState};

const STATUS_PATH: &str = "/trae/api/v2/ug/checkin_credits/status";
const CLAIM_PATH: &str = "/trae/api/v2/ug/checkin_credits/claim";
const DEFAULT_HOST: &str = "https://api.trae.cn";

/// 签到事件通知者：core 不依赖 Tauri，宿主层自行实现。
/// 桌面端转发为 Tauri 事件（前端刷新用），server 端降级为 stdout 日志，测试传 `NoopNotifier`。
pub trait CheckinNotifier: Send + Sync {
    fn emit(&self, event: &str, payload: Value);
}

/// 空通知者：吞掉所有事件。
pub struct NoopNotifier;

impl CheckinNotifier for NoopNotifier {
    fn emit(&self, _event: &str, _payload: Value) {}
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn api_succeeded(data: &Value) -> bool {
    data.get("code").and_then(|v| v.as_i64()).map(|c| c == 0 || c == 200).unwrap_or(false)
        || data.get("code").and_then(|v| v.as_str()).map(|s| s == "0" || s == "200").unwrap_or(false)
        || data.get("success").and_then(|v| v.as_bool()).unwrap_or(false)
        || data.get("status").and_then(|v| v.as_str()).map(|s| s == "success").unwrap_or(false)
}

// ===== 请求头伪造(参考 _build_headers) =====

/// 完整请求头:按账号独立伪设备 id 与 session。x-request-id / x-tt-trace-id 每次请求刷新。
fn build_headers(jwt: &str, dev: &crate::models::DeviceEntry) -> reqwest::header::HeaderMap {
    let mut h = reqwest::header::HeaderMap::new();
    let mut ins = |k: &'static str, v: String| {
        if let Ok(v) = reqwest::header::HeaderValue::from_str(&v) {
            h.insert(reqwest::header::HeaderName::from_static(k), v);
        }
    };
    let auth = if jwt.starts_with("Cloud-IDE-JWT ") || jwt.starts_with("Bearer ") {
        jwt.to_string()
    } else {
        format!("Cloud-IDE-JWT {jwt}")
    };
    ins("authorization", auth);
    ins("accept", "*/*".into());
    ins("accept-encoding", "gzip, deflate".into());
    ins("accept-language", "zh-CN".into());
    ins("content-type", "application/json".into());
    ins("user-agent", "VSCode 1.107.1 (TRAE SOLO CN)".into());
    ins("x-market-client-id", "VSCode 1.107.1".into());
    ins("x-market-user-id", dev.market_user_id.clone());
    ins("x-user-region", "CN".into());
    ins("x-device-id", dev.device_id.clone());
    ins("x-lgw-req-sdk-type", "3".into());
    ins("package-type", "stable_cn".into());
    ins("x-lscbd-aid", "787976".into());
    ins("x-lscbd-platform", "windows".into());
    ins("app-version", "0.1.45".into());
    ins("x-tt-trace-id", format!("00-{}-01", uuid::Uuid::new_v4().simple().to_string()[..16].to_string()));
    ins("vscode-sessionid", dev.session_id.clone());
    ins("sec-fetch-dest", "empty".into());
    ins("sec-fetch-mode", "no-cors".into());
    ins("sec-fetch-site", "none".into());
    h
}

/// 统一 POST:注入每次请求独立的 x-request-id,30s 超时。返回 (http_status, JSON body)。
async fn http_post(
    client: &reqwest::Client,
    base: &str,
    path: &str,
    jwt: &str,
    dev: &crate::models::DeviceEntry,
    body: Value,
) -> Result<(u16, Value), String> {
    let url = format!("{}{}", base.trim_end_matches('/'), path);
    let mut headers = build_headers(jwt, dev);
    if let Ok(v) = reqwest::header::HeaderValue::from_str(&uuid::Uuid::new_v4().to_string()) {
        headers.insert(reqwest::header::HeaderName::from_static("x-request-id"), v);
    }
    let resp = client
        .post(&url)
        .headers(headers)
        .json(&body)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| format!("请求失败: {e}"))?;
    let status = resp.status().as_u16();
    let data: Value = resp.json().await.unwrap_or(Value::Null);
    Ok((status, data))
}

/// 预检签到状态。返回 (ok, checked_in, credits, code, message)。
async fn status_check(
    client: &reqwest::Client,
    base: &str,
    jwt: &str,
    dev: &crate::models::DeviceEntry,
) -> (bool, Option<bool>, Option<i64>, Option<i64>, String) {
    let (status, data) = match http_post(client, base, STATUS_PATH, jwt, dev, json!({})).await {
        Ok(v) => v,
        Err(e) => return (false, None, None, None, e),
    };
    if status >= 400 || data.is_null() {
        let msg = data
            .get("message")
            .or_else(|| data.get("msg"))
            .and_then(|v| v.as_str())
            .unwrap_or(&format!("HTTP {status}"))
            .to_string();
        return (false, None, None, None, msg);
    }
    let code = data.get("code").and_then(|v| v.as_i64());
    let checked_in = data.get("checked_in").and_then(|v| v.as_bool());
    let credits = data
        .get("credits")
        .and_then(|v| v.as_i64())
        .or_else(|| data.get("data").and_then(|d| d.get("credits")).and_then(|v| v.as_i64()));
    let msg = data
        .get("message")
        .or_else(|| data.get("msg"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    (code == Some(0) || code.is_none(), checked_in, credits, code, msg)
}

/// 执行签到(仅网络异常重试,业务失败不重试)。返回 (ok, message, code, http_status)。
async fn signin_with_retry(
    client: &reqwest::Client,
    base: &str,
    jwt: &str,
    dev: &crate::models::DeviceEntry,
    retry: u32,
) -> (bool, String, Option<i64>, u16) {
    let mut last = (false, "无重试".to_string(), None, 0u16);
    for attempt in 0..=retry {
        let (status, data) = match http_post(client, base, CLAIM_PATH, jwt, dev, json!({})).await {
            Ok(v) => v,
            Err(e) => {
                last = (false, e, None, 0);
                if attempt < retry {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
                continue;
            }
        };
        let code = if data.is_null() { None } else { data.get("code").and_then(|v| v.as_i64()) };
        let msg = if data.is_null() {
            format!("HTTP {status}")
        } else {
            data.get("message")
                .or_else(|| data.get("msg"))
                .and_then(|v| v.as_str())
                .unwrap_or(if api_succeeded(&data) { "签到成功" } else { "签到失败" })
                .to_string()
        };
        let ok = !data.is_null() && api_succeeded(&data);
        last = (ok, msg, code, status);
        // 业务失败(code 有值)不重试;仅网络异常(code 为 None 且 status==0)重试
        if ok || code.is_some() || status > 0 {
            return last;
        }
        if attempt < retry {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
    last
}

// ===== 凭据解析 =====

/// 桌面账号:解密凭据并从实例目录回读最新 token(保留现状,应用不主动刷新)。
async fn get_valid_credential(account: &Account, state: &TraeState) -> AppResult<Credential> {
    let encrypted = account
        .encrypted_credential
        .as_ref()
        .ok_or_else(|| AppError::Credential("该账号尚未导入 TRAE 桌面凭证".into()))?;
    let mut cred = decrypt_credential(encrypted)?;
    let now = now_ms();
    let (synced, adopted) = crate::trae_instance::sync_credential_from_instance(account, &cred);
    if synced.expires_at > now || adopted {
        persist_credential(state, account, &synced);
        cred = synced;
    }
    if cred.expires_at <= now {
        mark_credential_expired(state, &account.id);
        return Err(AppError::Credential(
            "token 已过期，请打开该账号的 TRAE 实例（TRAE 会自动刷新），刷新后重试".into(),
        ));
    }
    Ok(cred)
}

/// 将回读采纳的凭据加密回写应用存储(应用不主动刷新 token,不回写实例目录)
fn persist_credential(state: &TraeState, account: &Account, cred: &Credential) {
    let status = credential_status(cred.expires_at, now_ms());
    let Ok(new_encrypted) = encrypt_credential(cred) else {
        return;
    };
    let mut data = state.data.lock().unwrap();
    data.update_account(
        &account.id,
        json!({ "encryptedCredential": new_encrypted, "credentialStatus": status }),
    );
    let _ = data.save(&state.store_file());
}

fn mark_credential_expired(state: &TraeState, account_id: &str) {
    let mut data = state.data.lock().unwrap();
    data.update_account(account_id, json!({ "credentialStatus": "expired" }));
    let _ = data.save(&state.store_file());
}

/// JWT 账号:解密存储的 JWT;有 refresh_token 且 24h 内过期/已过期时,持锁自动 ExchangeToken 刷新并原子写回。
/// 返回 (完整 jwt, base URL, 本次是否刷新)。token 已过期且无 refresh_token 时返回明确指引。
async fn get_jwt_credential(
    account: &Account,
    state: &TraeState,
    client: &reqwest::Client,
) -> AppResult<(String, String, bool)> {
    let encrypted = account
        .encrypted_jwt
        .as_ref()
        .ok_or_else(|| AppError::Credential("该账号尚未录入 JWT".into()))?;
    let mut sec = decrypt_jwt_secret(encrypted)?;
    let mut refreshed = false;

    // 需要刷新:有 refresh_token 且 exp<=24h(或已过期/无法解析)
    let need_refresh = {
        let info = crate::jwt::parse(&sec.jwt);
        sec.refresh_token
            .as_deref()
            .map(|rt| !rt.is_empty())
            .unwrap_or(false)
            && info
                .exp_hours
                .map(|h| h <= 24.0)
                .unwrap_or(true)
    };
    if need_refresh {
        // 并发安全:持锁后重读账号(double-check)
        let _guard = state.jwt_refresh_lock.lock().await;
        let current = {
            let data = state.data.lock().unwrap();
            data.get_accounts().iter().find(|a| a.id == account.id).cloned()
        };
        if let Some(current) = current {
            if let Some(enc) = current.encrypted_jwt.as_deref() {
                if let Ok(cur_sec) = decrypt_jwt_secret(enc) {
                    sec = cur_sec;
                }
            }
        }
        let info = crate::jwt::parse(&sec.jwt);
        let still_need = sec
            .refresh_token
            .as_deref()
            .map(|rt| !rt.is_empty())
            .unwrap_or(false)
            && info
                .exp_hours
                .map(|h| h <= 24.0)
                .unwrap_or(true);
        if still_need {
            let rt = sec.refresh_token.clone().unwrap_or_default();
            let (new_jwt, new_rt) = crate::jwt::exchange_token(&rt, client)
                .await
                .map_err(|e| AppError::Credential(format!("JWT 自动刷新失败: {e}")))?;
            // 校验新 user_id 一致
            let old_uid = effective_user_id(account);
            let new_uid = crate::jwt::parse(&new_jwt).user_id;
            if let (Some(ou), Some(nu)) = (old_uid, new_uid.as_deref()) {
                if ou != nu {
                    return Err(AppError::Credential(format!(
                        "刷新后 user_id 不匹配: 期望={ou}, 实际={nu}"
                    )));
                }
            }
            sec.jwt = new_jwt;
            if let Some(rt2) = new_rt {
                sec.refresh_token = Some(rt2);
            }
            refreshed = true;
        }
    }

    // 持久化刷新结果(即使未刷新也回写展示态?不,仅刷新时回写)
    if refreshed {
        if let Ok(enc) = encrypt_jwt_secret(&sec) {
            let mut data = state.data.lock().unwrap();
            data.update_account(&account.id, json!({ "encryptedJwt": enc }));
            let _ = data.save(&state.store_file());
        }
    }

    if crate::jwt::parse(&sec.jwt).exp_hours.unwrap_or(-1.0) <= 0.0 {
        mark_credential_expired(state, &account.id);
        return Err(AppError::Credential(
            "JWT 已过期且无 refresh_token，无法自动刷新，请重新录入".into(),
        ));
    }
    Ok((sec.jwt, DEFAULT_HOST.to_string(), refreshed))
}

// ===== 统一签到引擎 =====

/// 单账号签到结果(结构化,供事件与状态回写)
#[derive(Debug, Clone)]
pub struct CheckinOutcome {
    pub success: bool,
    pub message: String,
    /// skip_already / claim_ok / claim(失败)
    pub action: String,
    pub code: Option<i64>,
    pub http_status: u16,
    pub credits_before: Option<i64>,
    pub credits_delta: i64,
    pub error_type: Option<String>,
    pub cooldown_until: Option<i64>,
    /// 本次签到前是否自动刷新了 jwt
    pub refreshed: bool,
}

/// 统一签到:解析凭据 → 伪设备 → status/claim → 错误分类冷却 → 积分历史。
pub async fn checkin_engine(
    state: &TraeState,
    account: &Account,
    client: &reqwest::Client,
) -> CheckinOutcome {
    // 1. 凭据与 base URL
    let (jwt, base, refreshed) = match account.source {
        crate::models::AccountSource::Desktop => match get_valid_credential(account, state).await {
            Ok(c) => (
                if c.token.starts_with("Cloud-IDE-JWT ") || c.token.starts_with("Bearer ") {
                    c.token.clone()
                } else {
                    format!("Cloud-IDE-JWT {}", c.token)
                },
                if c.host.is_empty() { DEFAULT_HOST.to_string() } else { c.host },
                false,
            ),
            Err(e) => {
                return CheckinOutcome {
                    success: false,
                    message: e.to_string(),
                    action: "claim".into(),
                    code: None,
                    http_status: 0,
                    credits_before: None,
                    credits_delta: 0,
                    error_type: None,
                    cooldown_until: None,
                    refreshed: false,
                }
            }
        },
        crate::models::AccountSource::Jwt => match get_jwt_credential(account, state, client).await {
            Ok((j, b, r)) => (j, b, r),
            Err(e) => {
                return CheckinOutcome {
                    success: false,
                    message: e.to_string(),
                    action: "claim".into(),
                    code: None,
                    http_status: 0,
                    credits_before: None,
                    credits_delta: 0,
                    error_type: None,
                    cooldown_until: None,
                    refreshed: false,
                }
            }
        },
    };

    let Some(uid) = effective_user_id(account) else {
        return CheckinOutcome {
            success: false,
            message: "账号缺少 user_id".into(),
            action: "claim".into(),
            code: None,
            http_status: 0,
            credits_before: None,
            credits_delta: 0,
            error_type: None,
            cooldown_until: None,
            refreshed,
        };
    };

    // 2. 稳定伪设备身份
    let dev = crate::device_map::get_device_for(state, uid);

    // 3. status 预检:已签到则跳过 claim
    let (ok_s, checked_in, credits_before, code_s, msg_s) =
        status_check(client, &base, &jwt, &dev).await;
    if ok_s && checked_in == Some(true) {
        crate::credits::save_credits_history(state, uid, credits_before.unwrap_or(0), 0);
        return CheckinOutcome {
            success: true,
            message: if msg_s.is_empty() { "今日已签到".into() } else { msg_s },
            action: "skip_already".into(),
            code: code_s,
            http_status: 200,
            credits_before,
            credits_delta: 0,
            error_type: None,
            cooldown_until: None,
            refreshed,
        };
    }
    if !ok_s {
        eprintln!("[checkin] status 预检失败 (code={code_s:?}) {msg_s} —— 仍尝试 claim");
    }

    // 4. claim
    let retry = {
        let data = state.data.lock().unwrap();
        data.get_settings().retry_count
    };
    let (ok, msg, code, http_status) =
        signin_with_retry(client, &base, &jwt, &dev, retry).await;

    let (error_type, cooldown_until, credits_delta) = if ok {
        // 成功:清除冷却 + 记录积分历史(credits=delta=status 额度)
        cooldown::clear_cooldown(state, uid);
        let delta = credits_before.unwrap_or(0);
        crate::credits::save_credits_history(state, uid, delta, delta);
        (None, None, delta)
    } else {
        // 失败:分类错误并写入冷却
        let (et, secs) = cooldown::classify_error(http_status, code, &msg);
        let until = if secs != 0 && et != "Unknown" {
            let now = chrono::Utc::now().timestamp();
            cooldown::save_cooldown(state, uid, &et, secs, &msg);
            Some(if secs < 0 { crate::cooldown::PERMANENT_UNTIL } else { now + secs })
        } else {
            None
        };
        (if et == "Unknown" { None } else { Some(et) }, until, 0)
    };

    CheckinOutcome {
        success: ok,
        message: msg,
        action: if ok { "claim_ok" } else { "claim" }.into(),
        code,
        http_status,
        credits_before,
        credits_delta,
        error_type,
        cooldown_until,
        refreshed,
    }
}

// ===== 事件契约(trae-checkin-start / trae-checkin-progress / trae-checkin-done) =====

/// 空通知者复用值,避免每次调用分配。
pub const NOOP: NoopNotifier = NoopNotifier;

fn emit_start(notify: &dyn CheckinNotifier, total: usize) {
    notify.emit("trae-checkin-start", json!({ "type": "start", "total": total }));
}

fn emit_progress(notify: &dyn CheckinNotifier, index: usize, total: usize, account: &Account, o: &CheckinOutcome) {
    let status = if o.success {
        if o.action == "skip_already" { "already" } else { "success" }
    } else {
        "fail"
    };
    notify.emit(
        "trae-checkin-progress",
        json!({
            "type": "account",
            "index": index,
            "total": total,
            "user_id": effective_user_id(account),
            "name": account.name,
            "status": status,
            "code": o.code,
            "http_status": o.http_status,
            "refreshed": o.refreshed,
            "message": o.message,
            "credits": o.credits_before,
            "delta": if o.credits_delta > 0 { json!(o.credits_delta) } else { Value::Null },
            "error_type": o.error_type,
            "cooldown_until": o.cooldown_until,
        }),
    );
}

fn emit_done(notify: &dyn CheckinNotifier, ok: usize, already: usize, failed: usize) {
    notify.emit("trae-checkin-done", json!({ "type": "done", "ok": ok, "already": already, "failed": failed }));
}

/// 单账号签到:引擎 + 状态回写。notify 传 None 则不发事件。
pub async fn perform_checkin(
    notify: Option<&dyn CheckinNotifier>,
    account: &Account,
    client: &reqwest::Client,
    state: &TraeState,
) -> CheckinResult {
    if let Some(n) = notify {
        emit_start(n, 1);
    }
    let outcome = checkin_engine(state, account, client).await;
    if let Some(n) = notify {
        emit_progress(n, 1, 1, account, &outcome);
        let (ok, already, failed) = match outcome.action.as_str() {
            "skip_already" => (0, 1, 0),
            "claim_ok" => (1, 0, 0),
            _ => (0, 0, 1),
        };
        emit_done(n, ok, already, failed);
    }

    // 更新账号状态与日志
    let new_points = match (outcome.credits_delta, account.points) {
        (d, Some(base)) if d > 0 => Some(base + d),
        _ => account.points,
    };
    let mut data = state.data.lock().unwrap();
    data.update_account(
        &account.id,
        json!({
            "lastCheckinAt": now_ms(),
            "lastCheckinResult": if outcome.success { "success" } else { "failed" },
            "lastCheckinMessage": outcome.message,
            "points": new_points,
        }),
    );
    data.add_log(CheckinLog {
        id: generate_id(),
        account_id: account.id.clone(),
        account_name: account.name.clone(),
        time: now_ms(),
        result: if outcome.success { "success".into() } else { "failed".into() },
        message: outcome.message.clone(),
        points_gained: if outcome.credits_delta > 0 { Some(outcome.credits_delta) } else { None },
    });
    let _ = data.save(&state.store_file());
    drop(data);

    CheckinResult {
        success: outcome.success,
        message: outcome.message,
        points: if outcome.credits_delta > 0 { Some(outcome.credits_delta) } else { None },
    }
}

/// 执行所有启用账号签到:过滤冷却/已过期 → 积分过期感知排序 → 逐账号引擎 → 写 checkin_summary。
/// `account_interval_secs`:账号之间的执行间隔(手动一键签到传 2s;自动定时签到传配置的分钟数×60)。
/// notify 传 None 则不发事件。
pub async fn perform_all_checkin(
    notify: Option<&dyn CheckinNotifier>,
    client: &reqwest::Client,
    state: &TraeState,
    account_interval_secs: u64,
) -> Vec<(PublicAccount, CheckinResult)> {
    // 1. 取启用账号
    let accounts: Vec<Account> = {
        let data = state.data.lock().unwrap();
        data.get_accounts().iter().filter(|a| a.enabled).cloned().collect()
    };

    // 2. 过滤:冷却中(含 SessionDead 永久)/ JWT 已过期且无 refresh_token
    let now = chrono::Utc::now().timestamp();
    let mut pending: Vec<Account> = Vec::new();
    for a in accounts {
        let Some(uid) = effective_user_id(&a) else { continue };
        if let Some((_, _, _)) = cooldown::cooldown_of(state, uid, now) {
            continue; // 冷却中跳过
        }
        if a.source == crate::models::AccountSource::Jwt {
            if let Some(enc) = a.encrypted_jwt.as_deref() {
                if let Ok(sec) = decrypt_jwt_secret(enc) {
                    let has_rt = sec.refresh_token.as_deref().map(|r| !r.is_empty()).unwrap_or(false);
                    let info = crate::jwt::parse(&sec.jwt);
                    let expired = info.exp_hours.map(|h| h <= 0.0).unwrap_or(false);
                    if expired && !has_rt {
                        continue; // 已过期且无 refresh_token,无法自动刷新
                    }
                }
            }
        }
        pending.push(a);
    }

    // 3. 积分过期感知排序:credits_expire_at 升序(最近过期优先),无过期排后,同过期按剩余积分降序
    let rc: crate::models::RemainingCreditsFile =
        crate::fs_utils::read_json(&state.data_path("remaining_credits.json"));
    pending.sort_by(|a, b| {
        let ea = effective_user_id(a).and_then(|u| rc.expire_times.get(u).copied());
        let eb = effective_user_id(b).and_then(|u| rc.expire_times.get(u).copied());
        match (ea, eb) {
            (Some(ta), Some(tb)) if ta != tb => ta.cmp(&tb),
            (Some(_ta), Some(_)) => {
                let ca = effective_user_id(a).and_then(|u| rc.credits.get(u).copied()).unwrap_or(0.0);
                let cb = effective_user_id(b).and_then(|u| rc.credits.get(u).copied()).unwrap_or(0.0);
                cb.partial_cmp(&ca).unwrap_or(std::cmp::Ordering::Equal)
            }
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        }
    });

    // 4. 逐账号执行(间隔 2s),写 summary
    if let Some(n) = notify {
        emit_start(n, pending.len());
    }
    let mut results = Vec::new();
    let mut ok = 0usize;
    let mut already = 0usize;
    let mut failed = 0usize;
    for (i, account) in pending.iter().enumerate() {
        let outcome = checkin_engine(state, account, client).await;
        if let Some(n) = notify {
            emit_progress(n, i + 1, pending.len(), account, &outcome);
        }
        match outcome.action.as_str() {
            "skip_already" => already += 1,
            "claim_ok" => ok += 1,
            _ => failed += 1,
        }
        results.push((
            account.clone().into(),
            CheckinResult {
                success: outcome.success,
                message: outcome.message.clone(),
                points: if outcome.credits_delta > 0 { Some(outcome.credits_delta) } else { None },
            },
        ));
        // 更新账号状态与日志(复用 perform_checkin 的回写逻辑)
        {
            let mut data = state.data.lock().unwrap();
            data.update_account(
                &account.id,
                json!({
                    "lastCheckinAt": now_ms(),
                    "lastCheckinResult": if outcome.success { "success" } else { "failed" },
                    "lastCheckinMessage": outcome.message.clone(),
                }),
            );
            data.add_log(CheckinLog {
                id: generate_id(),
                account_id: account.id.clone(),
                account_name: account.name.clone(),
                time: now_ms(),
                result: if outcome.success { "success".into() } else { "failed".into() },
                message: outcome.message.clone(),
                points_gained: if outcome.credits_delta > 0 { Some(outcome.credits_delta) } else { None },
            });
            let _ = data.save(&state.store_file());
        }

        if i + 1 < pending.len() {
            tokio::time::sleep(Duration::from_secs(account_interval_secs)).await;
        }
    }
    if let Some(n) = notify {
        emit_done(n, ok, already, failed);
    }

    // 写批次摘要
    let summary = crate::models::CheckinSummary {
        time: Some(crate::fs_utils::now_iso()),
        results: results
            .iter()
            .map(|(p, r): &(PublicAccount, CheckinResult)| {
                json!({
                    "name": p.name,
                    "ok": r.success,
                    "message": r.message,
                    "points": r.points,
                })
            })
            .collect(),
        total_ok: ok as i32,
        already: already as i32,
        failed: failed as i32,
    };
    let _ = crate::fs_utils::write_json(&state.data_path("checkin_summary.json"), &summary);

    results
}

// ===== 积分查询 =====

/// 查询账号总积分(走参考 calc_remaining_credits),同步写 remaining_credits.json 与 account.points。
pub async fn get_total_points(
    account: &Account,
    client: &reqwest::Client,
    state: &TraeState,
) -> PointsResult {
    let Some(jwt) = crate::credits::resolve_account_jwt(account) else {
        return PointsResult {
            success: false,
            message: "账号无可用凭据".into(),
            total_points: None,
        };
    };
    let Some(uid) = effective_user_id(account) else {
        return PointsResult {
            success: false,
            message: "账号缺少 user_id".into(),
            total_points: None,
        };
    };
    match crate::credits::calc_remaining_credits(&jwt, client).await {
        Ok((credits, expire_at, _)) => {
            let mut rc: crate::models::RemainingCreditsFile =
                crate::fs_utils::read_json(&state.data_path("remaining_credits.json"));
            rc.credits.insert(uid.to_string(), credits);
            if let Some(exp) = expire_at {
                rc.expire_times.insert(uid.to_string(), exp);
            }
            rc.updated_at = Some(crate::fs_utils::now_iso());
            let _ = crate::fs_utils::write_json(&state.data_path("remaining_credits.json"), &rc);
            PointsResult {
                success: true,
                message: "获取积分成功".into(),
                total_points: Some(credits as i64),
            }
        }
        Err(e) => PointsResult {
            success: false,
            message: e,
            total_points: None,
        },
    }
}

/// 手动刷新桌面账号凭证:仅从实例目录回读(保留现有命令语义)。
pub async fn refresh_account_credential(
    account: &Account,
    client: &reqwest::Client,
    state: &TraeState,
) -> AppResult<Credential> {
    let _ = client;
    let encrypted = account
        .encrypted_credential
        .as_ref()
        .ok_or_else(|| AppError::Credential("该账号尚未导入 TRAE 桌面凭证".into()))?;
    let cred = decrypt_credential(encrypted)?;
    let (synced, adopted) = crate::trae_instance::sync_credential_from_instance(account, &cred);
    if synced.expires_at > now_ms() || adopted {
        persist_credential(state, account, &synced);
        Ok(synced)
    } else {
        mark_credential_expired(state, &account.id);
        Err(AppError::Credential(
            "token 已过期，请打开该账号的 TRAE 实例（TRAE 会自动刷新），刷新后重试".into(),
        ))
    }
}

#[cfg(test)]
mod e2e_tests {
    use super::*;
    use crate::credentials::encrypt_credential;
    use crate::trae_auth::get_trae_desktop_credentials;

    /// 端到端:读取桌面凭据 -> DPAPI 加密 -> checkin_engine 真实签到。
    /// 今日已签到则返回 skip_already(无副作用);未签到则执行 claim。
    #[tokio::test]
    async fn e2e_engine_checkin() {
        let cred = match get_trae_desktop_credentials() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[e2e] 未读取到 TRAE 桌面凭据(可能未登录桌面客户端): {e}");
                return;
            }
        };
        let encrypted = encrypt_credential(&cred).expect("DPAPI 加密失败");
        let account = Account {
            id: "e2e".into(),
            name: cred.account_name.clone(),
            cookie: String::new(),
            created_at: 0,
            enabled: true,
            desktop_user_id: Some(cred.user_id.clone()),
            encrypted_credential: Some(encrypted),
            ..Default::default()
        };
        let tmp = std::env::temp_dir().join("trae-engine-e2e");
        let _ = std::fs::remove_dir_all(&tmp);
        let state = TraeState::new(tmp.clone());
        {
            let mut data = state.data.lock().unwrap();
            data.accounts = vec![account.clone()];
            let _ = data.save(&state.store_file());
        }
        let client = reqwest::Client::new();
        let outcome = checkin_engine(&state, &account, &client).await;
        eprintln!(
            "[e2e] 签到结果: success={}, action={}, message={}, delta={}",
            outcome.success, outcome.action, outcome.message, outcome.credits_delta
        );
        // 已签到(skip_already)或成功(claim_ok)都算通过;失败但非鉴权类也算过(限频/网络波动)
        assert!(
            outcome.success || outcome.action == "claim",
            "签到异常: {}",
            outcome.message
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
