// 账号视图聚合（自宿主命令层下沉，桌面端与 HTTP server 共用）：
// 把 store 内账号与参考式数据文件（device_map / 冷却 / 剩余积分 / 今日签到）聚合成前端视图。

use std::collections::HashSet;

use crate::credentials;
use crate::cooldown;
use crate::credits;
use crate::device_map;
use crate::fs_utils;
use crate::jwt;
use crate::models::{Account, AccountSource, PublicAccount};
use crate::store::{effective_user_id, TraeState};

/// 聚合 device_map / 冷却 / 剩余积分 / JWT 状态 / 今日签到（对应参考 build_account_views）。
pub fn build_account_views(state: &TraeState) -> Vec<PublicAccount> {
    let now_ts = chrono::Utc::now().timestamp();
    let device_map_map = device_map::load_device_map(state);
    let cooldowns = cooldown::all_cooldowns(state, now_ts);
    let rc: crate::models::RemainingCreditsFile =
        fs_utils::read_json(&state.data_path("remaining_credits.json"));
    let summary: crate::models::CheckinSummary =
        fs_utils::read_json(&state.data_path("checkin_summary.json"));
    let today = fs_utils::today_prefix();
    let summary_today = summary
        .time
        .as_deref()
        .map(|t| t.starts_with(&today))
        .unwrap_or(false);
    let checked_names: HashSet<String> = if summary_today {
        summary
            .results
            .iter()
            .filter(|r| {
                let ok = r.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
                ok || r.get("message").and_then(|v| v.as_str()).is_some()
            })
            .filter_map(|r| r.get("name").and_then(|v| v.as_str()).map(String::from))
            .collect()
    } else {
        HashSet::new()
    };

    let accounts: Vec<Account> = {
        let data = state.data.lock().unwrap();
        data.get_accounts().to_vec()
    };

    accounts
        .into_iter()
        .map(|mut a| {
            let uid_owned = effective_user_id(&a).map(String::from);
            if let Some(uid) = uid_owned.as_deref() {
                // JWT 过期状态(仅 jwt 账号)
                if a.source == AccountSource::Jwt {
                    if let Some(sec) = a
                        .encrypted_jwt
                        .as_deref()
                        .and_then(|e| credentials::decrypt_jwt_secret(e).ok())
                    {
                        let info = jwt::parse(&sec.jwt);
                        a.jwt_exp_timestamp = info.exp_timestamp;
                        a.jwt_status = Some(jwt::status_of(info.exp_hours).to_string());
                        let has_rt = sec
                            .refresh_token
                            .as_deref()
                            .map(|r| !r.is_empty())
                            .unwrap_or(false);
                        a.has_refresh_token = has_rt;
                        a.jwt_auto_refresh =
                            has_rt && info.exp_hours.map(|h| h <= 24.0).unwrap_or(true);
                    }
                } else if let Some(status) = a.credential_status.clone() {
                    a.jwt_status = Some(status);
                }
                // 剩余积分 / 过期时间
                a.remaining_credits = rc.credits.get(uid).copied();
                a.credits_expire_at = rc.expire_times.get(uid).copied();
                // 设备标识(脱敏)
                a.device_id_masked = device_map_map.get(uid).map(|d| fs_utils::mask(&d.device_id));
                // 冷却状态
                if let Some((_, entry)) = cooldowns.iter().find(|(u, _)| u == uid) {
                    a.cooldown_type = Some(entry.error_type.clone());
                    a.cooldown_until = Some(entry.until);
                    a.cooldown_reason =
                        if entry.reason.is_empty() { None } else { Some(entry.reason.clone()) };
                }
                // 今日签到
                a.checked_today = Some(summary_today && checked_names.contains(&a.name));
                // 积分展示:优先 remaining_credits → credits_history → 既有值
                if let Some(c) = rc.credits.get(uid) {
                    a.points = Some(*c as i64);
                } else if let Some(c) = credits::latest_credits_of(state, uid) {
                    a.points = Some(c);
                }
            }
            a.into()
        })
        .collect()
}

/// 按 user_id 查找账号(desktop_user_id 与 user_id 都参与匹配)。
pub fn find_by_user_id(state: &TraeState, user_id: &str) -> Option<Account> {
    let data = state.data.lock().unwrap();
    data.get_accounts()
        .iter()
        .find(|a| {
            effective_user_id(a).map(|u| u == user_id).unwrap_or(false)
                || a.desktop_user_id.as_deref() == Some(user_id)
        })
        .cloned()
}
