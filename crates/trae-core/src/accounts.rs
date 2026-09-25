// 账号管理操作(自宿主命令层下沉,桌面端与 HTTP server 共用):
// 导入桌面账号 / 更新 / 删除 / JWT 录入与刷新 / 多开目录导入 / 积分查询。
// 宿主层只补托盘刷新、事件通知等宿主职责。

use serde_json::{json, Value};

use crate::credentials;
use crate::cooldown;
use crate::credits;
use crate::device_map;
use crate::error::{AppError, AppResult};
use crate::fs_utils;
use crate::jwt;
use crate::models::{
    credential_status, Account, AccountSource, CheckinResult, JwtCredential, PointsResult,
    PublicAccount, RemainingCreditsFile,
};
use crate::store::{effective_user_id, generate_id, TraeState};
use crate::trae_auth;
use crate::trae_machine;

pub fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 按 ID 取账号(克隆快照)。
pub fn get_account(state: &TraeState, id: &str) -> Option<Account> {
    let data = state.data.lock().unwrap();
    data.get_accounts().iter().find(|a| a.id == id).cloned()
}

/// 导入当前 TRAE 桌面账号:读取桌面凭据 -> DPAPI 加密 -> upsert 存储。
pub fn import_desktop(state: &TraeState) -> AppResult<PublicAccount> {
    let cred = trae_auth::get_trae_desktop_credentials()?;
    let encrypted = credentials::encrypt_credential(&cred)?;
    let now = now_ms();
    let status = credential_status(cred.expires_at, now);
    let account = Account {
        id: generate_id(),
        name: cred.account_name.clone(),
        cookie: String::new(),
        created_at: now,
        last_checkin_at: None,
        last_checkin_result: None,
        last_checkin_message: None,
        points: None,
        enabled: true,
        desktop_user_id: Some(cred.user_id.clone()),
        encrypted_credential: Some(encrypted),
        credential_status: Some(status.to_string()),
        data_dir: None,
        machine_id: None,
        source: AccountSource::Desktop,
        ..Default::default()
    };
    let mut data = state.data.lock().unwrap();
    let saved = data.upsert_desktop_account(account);
    let _ = data.save(&state.store_file());
    Ok(saved.into())
}

/// 更新账号(name/enabled 等;jwt 账号更新 jwt/refreshToken 时重新加密写回并同步 user_id)。
pub fn update(state: &TraeState, id: &str, mut updates: Value) -> AppResult<PublicAccount> {
    let mut data = state.data.lock().unwrap();
    let current = get_account(state, id).ok_or_else(|| AppError::NotFound(id.to_string()))?;
    if current.source == AccountSource::Jwt {
        let mut sec = current
            .encrypted_jwt
            .as_deref()
            .and_then(|e| credentials::decrypt_jwt_secret(e).ok())
            .unwrap_or_else(|| JwtCredential {
                jwt: String::new(),
                refresh_token: None,
            });
        if let Some(j) = updates.get("jwt").and_then(Value::as_str) {
            let j = j.trim().to_string();
            if !j.is_empty() {
                let info = jwt::parse(&j);
                if let Some(uid) = info.user_id {
                    if let Some(obj) = updates.as_object_mut() {
                        obj.insert("user_id".into(), json!(uid));
                    }
                }
                sec.jwt = jwt::normalize_full(&j);
            }
        }
        if let Some(rt) = updates.get("refreshToken").and_then(Value::as_str) {
            let t = rt.trim().to_string();
            sec.refresh_token = if t.is_empty() { None } else { Some(t) };
        }
        let encrypted = credentials::encrypt_jwt_secret(&sec)?;
        if let Some(obj) = updates.as_object_mut() {
            obj.insert("encryptedJwt".into(), json!(encrypted));
            obj.remove("jwt");
            obj.remove("refreshToken");
        }
    }
    let acc = data
        .update_account(id, updates)
        .ok_or_else(|| AppError::NotFound(id.to_string()))?;
    data.save(&state.store_file())?;
    Ok(acc.into())
}

/// 删除账号:关闭该账号的工具实例(若在运行;数据目录保留,可日后重新导入),
/// 并联动清理设备/冷却/积分数据文件中的该账号条目。
pub fn delete(state: &TraeState, id: &str) -> AppResult<bool> {
    let uid_to_cleanup = {
        let data = state.data.lock().unwrap();
        if let Some(acc) = data.get_accounts().iter().find(|a| a.id == id) {
            if let Some(dir) = acc.data_dir.as_deref().filter(|s| !s.is_empty()) {
                if trae_machine::is_instance_running(dir).0 {
                    let _ = trae_machine::kill_instance(dir);
                }
            }
            effective_user_id(acc).map(|s| s.to_string())
        } else {
            None
        }
    };
    {
        let mut data = state.data.lock().unwrap();
        data.delete_account(id);
        data.save(&state.store_file())?;
    }
    if let Some(uid) = &uid_to_cleanup {
        device_map::reset_device_for(state, uid);
        cooldown::remove_cooldown(state, uid);
        credits::remove_account_data(state, uid);
    }
    Ok(true)
}

/// 手动录入 JWT 账号(参考模式):解析 user_id -> 查重 -> DPAPI 加密 -> 保存。
pub fn add_jwt(
    state: &TraeState,
    name: String,
    jwt_token: &str,
    refresh_token: Option<String>,
    enabled: Option<bool>,
) -> AppResult<PublicAccount> {
    let info = jwt::parse(jwt_token);
    let uid = info
        .user_id
        .ok_or_else(|| AppError::Credential("无法从 JWT 解析 user_id,请检查格式".into()))?;
    let mut data = state.data.lock().unwrap();
    if data.get_accounts().iter().any(|a| {
        a.desktop_user_id.as_deref() == Some(uid.as_str())
            || a.user_id.as_deref() == Some(uid.as_str())
    }) {
        return Err(AppError::Credential("该账号已存在".into()));
    }
    let sec = JwtCredential {
        jwt: jwt::normalize_full(jwt_token),
        refresh_token,
    };
    let encrypted = credentials::encrypt_jwt_secret(&sec)?;
    let account = Account {
        id: generate_id(),
        name,
        cookie: String::new(),
        created_at: now_ms(),
        enabled: enabled.unwrap_or(true),
        source: AccountSource::Jwt,
        user_id: Some(uid.clone()),
        encrypted_jwt: Some(encrypted),
        ..Default::default()
    };
    let saved = data.upsert_desktop_account(account);
    let _ = data.save(&state.store_file());
    Ok(saved.into())
}

/// 手动刷新 jwt 账号的 JWT(refresh_token -> ExchangeToken),返回更新后账号。
pub async fn refresh_jwt(
    state: &TraeState,
    client: &reqwest::Client,
    user_id: &str,
) -> AppResult<PublicAccount> {
    let account = crate::views::find_by_user_id(state, user_id)
        .ok_or_else(|| AppError::NotFound(user_id.to_string()))?;
    if account.source != AccountSource::Jwt {
        return Err(AppError::Credential("仅 JWT 账号可手动刷新".into()));
    }
    let _guard = state.jwt_refresh_lock.lock().await;
    // 持锁后重读(防并发)
    let current = crate::views::find_by_user_id(state, user_id)
        .ok_or_else(|| AppError::NotFound(user_id.to_string()))?;
    let enc = current
        .encrypted_jwt
        .as_deref()
        .ok_or_else(|| AppError::Credential("该账号无 JWT 凭据".into()))?;
    let sec = credentials::decrypt_jwt_secret(enc)?;
    let rt = sec
        .refresh_token
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::Credential("该账号无 refresh_token,无法自动刷新".into()))?
        .to_string();
    let (new_jwt, new_rt) = jwt::exchange_token(&rt, client)
        .await
        .map_err(AppError::Credential)?;
    // 校验 user_id 一致
    let new_uid = jwt::parse(&new_jwt).user_id;
    if let Some(nu) = new_uid.as_deref() {
        if nu != user_id {
            return Err(AppError::Credential(format!(
                "刷新后 user_id 不匹配: 期望={user_id}, 实际={nu}"
            )));
        }
    }
    let mut sec2 = sec;
    sec2.jwt = new_jwt;
    if let Some(rt2) = new_rt {
        sec2.refresh_token = Some(rt2);
    }
    let encrypted = credentials::encrypt_jwt_secret(&sec2)?;
    let mut data = state.data.lock().unwrap();
    let acc = data
        .update_account(
            &current.id,
            json!({ "encryptedJwt": encrypted, "user_id": new_uid }),
        )
        .ok_or_else(|| AppError::NotFound(current.id.clone()))?;
    let _ = data.save(&state.store_file());
    Ok(acc.into())
}

/// 从已有多开目录导入账号(扫描列表的"导入"入口):
/// 完整凭据优先,缺失签名密钥时退宽松读取(token 可用但过期后无法自动刷新)。
/// 同 userId 账号已存在则仅更新凭据并绑定目录,保留名称/积分/签到历史。
pub fn import_from_dir(state: &TraeState, data_dir: &str) -> AppResult<PublicAccount> {
    let path = std::path::PathBuf::from(data_dir);
    if !path
        .join("User")
        .join("globalStorage")
        .join("storage.json")
        .exists()
    {
        return Err(AppError::NotFound(format!("目录无 storage.json: {data_dir}")));
    }
    let cred = trae_auth::read_credentials_from_data_dir(&path)
        .or_else(|_| trae_auth::read_auth_from_data_dir_loose(&path, &crate::models::Credential::empty()))?;
    if cred.user_id.is_empty() {
        return Err(AppError::Credential("目录登录信息缺少 userId".into()));
    }

    let encrypted = credentials::encrypt_credential(&cred)?;
    let now = now_ms();
    let status = credential_status(cred.expires_at, now);
    let machine_id = if cred.machine_id.is_empty() {
        None
    } else {
        Some(cred.machine_id.clone())
    };

    let mut data = state.data.lock().unwrap();
    let existing = data
        .get_accounts()
        .iter()
        .find(|a| a.desktop_user_id.as_deref() == Some(cred.user_id.as_str()))
        .cloned();
    let saved = match existing {
        Some(acc) => data
            .update_account(
                &acc.id,
                json!({
                    "encryptedCredential": encrypted,
                    "credentialStatus": status,
                    "dataDir": data_dir,
                    "machineId": machine_id,
                }),
            )
            .ok_or_else(|| AppError::NotFound(acc.id.clone()))?,
        None => {
            let account = Account {
                id: generate_id(),
                name: cred.account_name.clone(),
                cookie: String::new(),
                created_at: now,
                last_checkin_at: None,
                last_checkin_result: None,
                last_checkin_message: None,
                points: None,
                enabled: true,
                desktop_user_id: Some(cred.user_id.clone()),
                encrypted_credential: Some(encrypted),
                credential_status: Some(status.to_string()),
                data_dir: Some(data_dir.to_string()),
                machine_id,
                source: AccountSource::Desktop,
                ..Default::default()
            };
            data.upsert_desktop_account(account)
        }
    };
    data.save(&state.store_file())?;
    Ok(saved.into())
}

/// 查询账号总积分,成功时回写存储。
pub async fn account_points(
    state: &TraeState,
    client: &reqwest::Client,
    id: &str,
) -> AppResult<PointsResult> {
    let account = get_account(state, id).ok_or_else(|| AppError::NotFound(id.to_string()))?;
    let result = crate::checkin::get_total_points(&account, client, state).await;
    if let (true, Some(tp)) = (result.success, result.total_points) {
        let mut data = state.data.lock().unwrap();
        data.update_account(id, json!({ "points": tp }));
        let _ = data.save(&state.store_file());
    }
    Ok(result)
}

/// 手动执行单账号签到(不通知,宿主按需自行 emit)。
pub async fn checkin_one(
    state: &TraeState,
    client: &reqwest::Client,
    id: &str,
) -> AppResult<CheckinResult> {
    let account = get_account(state, id).ok_or_else(|| AppError::NotFound(id.to_string()))?;
    Ok(crate::checkin::perform_checkin(None, &account, client, state).await)
}

/// 实时查询某账号剩余积分并写 remaining_credits.json 缓存。
pub async fn fetch_remaining(
    state: &TraeState,
    client: &reqwest::Client,
    user_id: &str,
) -> AppResult<f64> {
    let account = crate::views::find_by_user_id(state, user_id)
        .ok_or_else(|| AppError::NotFound(user_id.to_string()))?;
    let jwt_token = credits::resolve_account_jwt(&account)
        .ok_or_else(|| AppError::Credential("账号无可用凭据".into()))?;
    let (credits_val, expire_at, _) = credits::calc_remaining_credits(&jwt_token, client)
        .await
        .map_err(AppError::Credential)?;
    let mut rc: RemainingCreditsFile = fs_utils::read_json(&state.data_path("remaining_credits.json"));
    rc.credits.insert(user_id.to_string(), credits_val);
    if let Some(exp) = expire_at {
        rc.expire_times.insert(user_id.to_string(), exp);
    }
    rc.updated_at = Some(fs_utils::now_iso());
    let _ = fs_utils::write_json(&state.data_path("remaining_credits.json"), &rc);
    Ok(credits_val)
}
