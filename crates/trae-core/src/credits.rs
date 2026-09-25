//! 积分查询/历史/每日快照。移植自参考项目 commands/accounts.rs。
//! 依赖请求:POST /trae/api/v2/pay/ide_user_ent_usage 计算剩余积分(含最早过期时间与今日购买)。

use std::path::PathBuf;

use crate::cooldown;
use crate::fs_utils;
use crate::models::{
    Account, CreditsDailyFile, CreditsDailySnapshot, CreditsFile, RemainingCreditsFile,
};

/// 北京时间固定偏移(不依赖 chrono::Local,某些 Windows 环境可能误判时区)
fn cst() -> chrono::FixedOffset {
    chrono::FixedOffset::east_opt(8 * 3600).unwrap()
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// 调用 TRAE API 计算剩余积分。
/// 返回 (剩余积分, 最早过期时间(Unix秒), 今日购买获得积分)。
/// 计算:遍历 user_entitlement_pack_list,仅对 quota.credits_limit 存在的包,
/// 剩余 = credits_limit - usage.credits_amount(≥0) 求和;expire_time > now 取最早;
/// start_time 今日本地时间且 charge_amount>0 计"今日购买"。
pub async fn calc_remaining_credits(
    jwt: &str,
    client: &reqwest::Client,
) -> Result<(f64, Option<i64>, f64), String> {
    let auth = crate::jwt::normalize_full(jwt);
    let resp = client
        .post("https://api.trae.cn/trae/api/v2/pay/ide_user_ent_usage")
        .header("authorization", &auth)
        .header("content-type", "application/json")
        .header("accept", "*/*")
        .json(&serde_json::json!({ "require_usage": true, "req_source": 2 }))
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| format!("API 请求失败: {e}"))?;

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("解析响应失败: {e}"))?;

    let packs = body
        .get("user_entitlement_pack_list")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "响应中缺少 user_entitlement_pack_list".to_string())?;

    let mut total: f64 = 0.0;
    let mut earliest_expire: Option<i64> = None;
    let mut today_non_checkin_earned: f64 = 0.0;

    let now_ts = chrono::Utc::now().timestamp();
    // 今日北京时间范围 [00:00:00 +08:00, 次日 00:00:00 +08:00]
    let today_start = chrono::Utc::now()
        .with_timezone(&cst())
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_local_timezone(cst())
        .unwrap()
        .timestamp();
    let today_end = today_start + 86_400;

    for pack in packs {
        let credits_limit = pack
            .get("entitlement_base_info")
            .and_then(|e| e.get("quota"))
            .and_then(|q| q.get("credits_limit"))
            .and_then(|v| v.as_f64());
        if let Some(limit) = credits_limit {
            let used = pack
                .get("usage")
                .and_then(|u| u.get("credits_amount"))
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            total += (limit - used).max(0.0);

            // expire_time 在 pack 顶层,取最早的(且未过期)
            let expire = pack.get("expire_time").and_then(|v| v.as_i64());
            if let Some(exp) = expire {
                if exp > now_ts {
                    earliest_expire = Some(earliest_expire.map_or(exp, |e| e.min(exp)));
                }
            }

            // 今日购买获得的积分:start_time 在今日北京时间范围内且 charge_amount > 0
            // (签到获得的 pack charge_amount=0,不会误判为购买积分)
            let start_time = pack
                .get("entitlement_base_info")
                .and_then(|e| e.get("start_time"))
                .and_then(|v| v.as_i64());
            let charge_amount = pack
                .get("entitlement_base_info")
                .and_then(|e| e.get("charge_amount"))
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            let is_purchased = charge_amount > 0;
            if let Some(st) = start_time {
                if st >= today_start && st < today_end && is_purchased {
                    today_non_checkin_earned += limit;
                }
            }
        }
    }

    Ok((
        round2(total),
        earliest_expire,
        round2(today_non_checkin_earned),
    ))
}

fn credits_history_path(state: &crate::store::TraeState) -> PathBuf {
    state.data_path("credits_history.json")
}

/// 把账号最新积分与本次新增写入 credits_history.json(按日期追加,裁剪到 90 天内)。
pub fn save_credits_history(
    state: &crate::store::TraeState,
    user_id: &str,
    credits: i64,
    delta: i64,
) {
    let path = credits_history_path(state);
    let mut file: CreditsFile = fs_utils::read_json(&path);
    let today = fs_utils::today_prefix();
    file.records.push(crate::models::CreditRecord {
        date: today.clone(),
        user_id: user_id.to_string(),
        credits,
        delta,
    });
    let cutoff = (chrono::Utc::now() - chrono::Duration::days(90))
        .format("%Y-%m-%d")
        .to_string();
    file.records.retain(|r| r.date >= cutoff);
    let _ = fs_utils::write_json(&path, &file);
}

/// 该账号最近日期的积分记录(同日期取较大值,跨日期取较新日期)。
pub fn latest_credits_of(state: &crate::store::TraeState, user_id: &str) -> Option<i64> {
    let file: CreditsFile = fs_utils::read_json(&credits_history_path(state));
    let mut best: Option<(String, i64)> = None;
    for r in &file.records {
        if r.user_id != user_id {
            continue;
        }
        match &best {
            None => best = Some((r.date.clone(), r.credits)),
            Some((d, c)) => {
                if r.date > *d || (r.date == *d && r.credits > *c) {
                    best = Some((r.date.clone(), r.credits));
                }
            }
        }
    }
    best.map(|(_, c)| c)
}

/// 删除账号联动:清理 remaining_credits.json 与 credits_history.json 中该 user_id 的条目。
pub fn remove_account_data(state: &crate::store::TraeState, user_id: &str) {
    let rc_path = state.data_path("remaining_credits.json");
    let mut rc: RemainingCreditsFile = fs_utils::read_json(&rc_path);
    if rc.credits.remove(user_id).is_some() || rc.expire_times.remove(user_id).is_some() {
        rc.updated_at = Some(fs_utils::now_iso());
        let _ = fs_utils::write_json(&rc_path, &rc);
    }
    let ch_path = credits_history_path(state);
    let mut ch: CreditsFile = fs_utils::read_json(&ch_path);
    let before = ch.records.len();
    ch.records.retain(|r| r.user_id != user_id);
    if ch.records.len() != before {
        let _ = fs_utils::write_json(&ch_path, &ch);
    }
}

/// 记录每日积分快照(每天计算一次):total=所有账号剩余积分之和;
/// earned=今日签到获得(delta 之和)+ 购买获得;consumed=|total-earned-昨日total|。
pub fn record_daily_snapshot(
    state: &crate::store::TraeState,
    rc: &RemainingCreditsFile,
    non_checkin_earned: f64,
) {
    let today = fs_utils::today_prefix();
    let total = round2(rc.credits.values().sum());

    let mut file: CreditsDailyFile = fs_utils::read_json(&state.data_path("credits_daily.json"));

    let credits_file: CreditsFile = fs_utils::read_json(&credits_history_path(state));
    let checkin_earned: f64 = credits_file
        .records
        .iter()
        .filter(|r| r.date == today && r.user_id != "_daily_total")
        .map(|r| r.delta as f64)
        .sum();
    let earned = round2(checkin_earned + non_checkin_earned);

    let yesterday_total = file
        .snapshots
        .iter()
        .filter(|s| s.date < today)
        .last()
        .map(|s| s.total)
        .unwrap_or(0.0);
    let consumed = round2((total - earned - yesterday_total).abs());

    if let Some(existing) = file.snapshots.iter_mut().find(|s| s.date == today) {
        existing.total = total;
        existing.earned = earned;
        existing.consumed = consumed;
    } else {
        file.snapshots.push(CreditsDailySnapshot {
            date: today,
            total,
            earned,
            consumed,
        });
    }

    let cutoff = (chrono::Utc::now() - chrono::Duration::days(90))
        .format("%Y-%m-%d")
        .to_string();
    file.snapshots.retain(|s| s.date >= cutoff);
    let _ = fs_utils::write_json(&state.data_path("credits_daily.json"), &file);
}

/// 解析账号当前可用的完整 JWT(desktop: 实例回读凭据的 token;jwt: 存储的 jwt)。
/// 供积分查询/自动解冻使用,不写入冷却或触发网络刷新。
pub fn resolve_account_jwt(account: &Account) -> Option<String> {
    match account.source {
        crate::models::AccountSource::Jwt => {
            let enc = account.encrypted_jwt.as_deref()?;
            let sec = crate::credentials::decrypt_jwt_secret(enc).ok()?;
            Some(crate::jwt::normalize_full(&sec.jwt))
        }
        crate::models::AccountSource::Desktop => {
            let enc = account.encrypted_credential.as_deref()?;
            let cred = crate::credentials::decrypt_credential(enc).ok()?;
            // 从实例目录回读最新 token(与签到引擎一致的凭据解析路径)
            let (synced, _) = crate::trae_instance::sync_credential_from_instance(account, &cred);
            Some(crate::jwt::normalize_full(&synced.token))
        }
    }
}

/// 刷新所有账号剩余积分(批量请求 API),返回成功数量。
/// 同时执行自动解冻:签到成功且有积分(credits>0)且冷却类型非 SessionDead → 清除冷却。
pub async fn refresh_remaining_credits(
    state: &crate::store::TraeState,
    client: &reqwest::Client,
) -> Result<usize, String> {
    let accounts: Vec<Account> = {
        let data = state.data.lock().unwrap();
        data.get_accounts().to_vec()
    };
    let mut rc: RemainingCreditsFile =
        fs_utils::read_json(&state.data_path("remaining_credits.json"));
    let mut ok_count = 0usize;
    let mut thawed_count = 0usize;
    let mut total_non_checkin_earned: f64 = 0.0;

    for a in &accounts {
        let Some(uid) = crate::store::effective_user_id(a) else {
            continue;
        };
        let Some(jwt) = resolve_account_jwt(a) else {
            continue;
        };
        match calc_remaining_credits(&jwt, client).await {
            Ok((credits, expire_at, non_checkin_earned)) => {
                rc.credits.insert(uid.to_string(), credits);
                if let Some(exp) = expire_at {
                    rc.expire_times.insert(uid.to_string(), exp);
                }
                total_non_checkin_earned += non_checkin_earned;
                ok_count += 1;
                // 自动解冻:有积分 + 冷却类型非 SessionDead → 清除
                if credits > 0.0 {
                    if let Some((et, _, _)) = cooldown::cooldown_of(state, uid, chrono::Utc::now().timestamp()) {
                        if et != "SessionDead" {
                            cooldown::clear_cooldown(state, uid);
                            thawed_count += 1;
                            fs_utils::app_log(
                                &state.base_dir,
                                &format!("自动解冻 [{}]: 类型={} 积分={}", a.name, et, credits),
                            );
                        }
                    }
                }
            }
            Err(e) => {
                fs_utils::app_log(
                    &state.base_dir,
                    &format!("获取剩余积分失败 [{}]: {e}", a.name),
                );
            }
        }
    }

    rc.updated_at = Some(fs_utils::now_iso());
    let _ = fs_utils::write_json(&state.data_path("remaining_credits.json"), &rc);
    record_daily_snapshot(state, &rc, total_non_checkin_earned);

    if thawed_count > 0 {
        fs_utils::app_log(
            &state.base_dir,
            &format!("积分刷新完成: 成功 {ok_count}, 自动解冻 {thawed_count}"),
        );
    }
    Ok(ok_count)
}
