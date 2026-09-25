//! 签到错误分类 + 冷却状态机(account_cooldowns.json)。移植自参考项目 auto_checkin.py。
//! 按 HTTP 状态码/业务码分类签到错误,每种类型对应不同冷却策略,写入持久化冷却状态;
//! 签到成功自动清除冷却(参考还要求积分>0,本实现由调用方决定时机)。

use std::path::PathBuf;

use crate::fs_utils;
use crate::models::{AccountCooldownsFile, CooldownEntry};

/// SessionDead 永久冷却的 until 哨兵值
pub const PERMANENT_UNTIL: i64 = 9_999_999_999;

fn cooldown_path(state: &crate::store::TraeState) -> PathBuf {
    state.data_path("account_cooldowns.json")
}

fn now_secs() -> i64 {
    chrono::Utc::now().timestamp()
}

/// 分类签到错误,返回 (error_type, cooldown_seconds)。
/// cooldown_seconds:-1=永久,0=不冷却(仅记录错误计数),>0=冷却秒数。
pub fn classify_error(http_status: u16, code: Option<i64>, _message: &str) -> (String, i64) {
    if http_status == 200 && code == Some(1005) {
        return ("PlanLimit".into(), 43_200);
    }
    if http_status == 429 {
        return ("SoftRate".into(), 60);
    }
    if http_status == 401 {
        return ("SessionDead".into(), -1);
    }
    if http_status == 404 {
        return ("NotFound".into(), 60);
    }
    if (500..600).contains(&http_status) {
        return ("Server".into(), 600);
    }
    if (400..500).contains(&http_status) {
        return ("Client".into(), 600);
    }
    if let Some(c) = code {
        if c != 0 {
            return ("BusinessError".into(), 300);
        }
    }
    ("Unknown".into(), 0)
}

/// 写入/更新账号冷却状态。cooldown_seconds:-1 永久,0 清除,>0 冷却秒数。
pub fn save_cooldown(
    state: &crate::store::TraeState,
    user_id: &str,
    error_type: &str,
    cooldown_seconds: i64,
    reason: &str,
) {
    let path = cooldown_path(state);
    let mut file: AccountCooldownsFile = fs_utils::read_json(&path);
    let now = now_secs();

    if cooldown_seconds == -1 {
        file.cooldowns.insert(
            user_id.to_string(),
            CooldownEntry {
                error_type: error_type.to_string(),
                until: PERMANENT_UNTIL,
                reason: reason.to_string(),
                error_count: 0,
            },
        );
    } else if cooldown_seconds == 0 {
        file.cooldowns.remove(user_id);
    } else {
        let existing = file.cooldowns.get(user_id).cloned().unwrap_or_default();
        let (until, error_count) = if error_type == "Server" || error_type == "Client" {
            let count = existing.error_count + 1;
            if count < 3 {
                file.cooldowns.insert(
                    user_id.to_string(),
                    CooldownEntry {
                        error_type: error_type.to_string(),
                        until: 0,
                        reason: reason.to_string(),
                        error_count: count,
                    },
                );
                file.updated_at = Some(fs_utils::now_iso());
                let _ = fs_utils::write_json(&path, &file);
                return;
            }
            (now + cooldown_seconds, 0)
        } else {
            (now + cooldown_seconds, 0)
        };
        file.cooldowns.insert(
            user_id.to_string(),
            CooldownEntry {
                error_type: error_type.to_string(),
                until,
                reason: reason.to_string(),
                error_count,
            },
        );
    }
    file.updated_at = Some(fs_utils::now_iso());
    let _ = fs_utils::write_json(&path, &file);
}

/// 清除账号冷却状态
pub fn clear_cooldown(state: &crate::store::TraeState, user_id: &str) {
    save_cooldown(state, user_id, "", 0, "");
}

/// 是否仍在冷却中(until>now 且类型非空;SessionDead 的 until=哨兵 天然永久)
pub fn is_cooled(entry: &CooldownEntry, now: i64) -> bool {
    entry.until > now && !entry.error_type.is_empty()
}

/// 查询账号冷却状态,未冷却返回 None。返回 (type, until, reason)。
pub fn cooldown_of(
    state: &crate::store::TraeState,
    user_id: &str,
    now: i64,
) -> Option<(String, i64, String)> {
    let file: AccountCooldownsFile = fs_utils::read_json(&cooldown_path(state));
    let entry = file.cooldowns.get(user_id)?;
    if is_cooled(entry, now) {
        Some((entry.error_type.clone(), entry.until, entry.reason.clone()))
    } else {
        None
    }
}

/// 读取全部冷却状态(供前端展示)
pub fn all_cooldowns(state: &crate::store::TraeState, now: i64) -> Vec<(String, CooldownEntry)> {
    let file: AccountCooldownsFile = fs_utils::read_json(&cooldown_path(state));
    file.cooldowns
        .iter()
        .filter(|(_, e)| is_cooled(e, now))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// 删除账号冷却记录(删除账号联动)
pub fn remove_cooldown(state: &crate::store::TraeState, user_id: &str) {
    let path = cooldown_path(state);
    let mut file: AccountCooldownsFile = fs_utils::read_json(&path);
    if file.cooldowns.remove(user_id).is_some() {
        file.updated_at = Some(fs_utils::now_iso());
        let _ = fs_utils::write_json(&path, &file);
    }
}

/// 清除所有冷却,返回清除数量
pub fn clear_all_cooldowns(state: &crate::store::TraeState) -> usize {
    let path = cooldown_path(state);
    let mut file: AccountCooldownsFile = fs_utils::read_json(&path);
    let n = file.cooldowns.len();
    if n > 0 {
        file.cooldowns.clear();
        file.updated_at = Some(fs_utils::now_iso());
        let _ = fs_utils::write_json(&path, &file);
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::AccountCooldownsFile;

    #[test]
    fn classify_all_branches() {
        assert_eq!(classify_error(200, Some(1005), ""), ("PlanLimit".into(), 43_200));
        assert_eq!(classify_error(429, None, ""), ("SoftRate".into(), 60));
        assert_eq!(classify_error(401, None, ""), ("SessionDead".into(), -1));
        assert_eq!(classify_error(404, None, ""), ("NotFound".into(), 60));
        assert_eq!(classify_error(500, None, ""), ("Server".into(), 600));
        assert_eq!(classify_error(502, None, ""), ("Server".into(), 600));
        assert_eq!(classify_error(400, None, ""), ("Client".into(), 600));
        assert_eq!(classify_error(403, Some(1000), ""), ("Client".into(), 600));
        assert_eq!(classify_error(200, Some(1000), ""), ("BusinessError".into(), 300));
        assert_eq!(classify_error(200, Some(0), ""), ("Unknown".into(), 0));
        assert_eq!(classify_error(200, None, ""), ("Unknown".into(), 0));
    }

    /// 文件往返:写入 -> 读取 -> 清除
    #[test]
    fn cooldown_file_roundtrip() {
        let tmp = std::env::temp_dir().join("trae-cooldown-test");
        let _ = std::fs::remove_dir_all(&tmp);
        let state = crate::store::TraeState::new(tmp.clone());
        let now = now_secs();

        // 普通错误直接冷却 300s
        save_cooldown(&state, "u1", "BusinessError", 300, "业务错误");
        let c = cooldown_of(&state, "u1", now);
        assert!(c.is_some());
        assert_eq!(c.unwrap().0, "BusinessError");

        // Server/Client 需连续 3 次才真正冷却
        for i in 1..=2 {
            save_cooldown(&state, "u2", "Server", 600, "5xx");
            assert!(
                cooldown_of(&state, "u2", now).is_none(),
                "第 {i} 次 Server 不应进入冷却(until=0)"
            );
            let file: AccountCooldownsFile = fs_utils::read_json(&cooldown_path(&state));
            assert_eq!(file.cooldowns.get("u2").unwrap().error_count, i);
        }
        save_cooldown(&state, "u2", "Server", 600, "5xx");
        assert!(cooldown_of(&state, "u2", now).is_some(), "第 3 次 Server 应进入冷却");

        // SessionDead 永久
        save_cooldown(&state, "u3", "SessionDead", -1, "401");
        let (t, until, _) = cooldown_of(&state, "u3", now).unwrap();
        assert_eq!(t, "SessionDead");
        assert_eq!(until, PERMANENT_UNTIL);

        // 清除
        clear_cooldown(&state, "u1");
        assert!(cooldown_of(&state, "u1", now).is_none());

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
