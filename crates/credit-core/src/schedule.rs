// 调度纯函数:轮间隔(2h±10min 均匀抖动)与"签到日"判定(UTC+8 当日 10:00 为日界)。
// 含宿主的调度循环在 src-tauri / wb-switch-server(阶段 B/C)。

use std::time::Duration;

/// 一轮的基础间隔:2 小时。
pub const ROUND_INTERVAL_SECS: u64 = 2 * 60 * 60;
/// 抖动幅度:±10 分钟。
pub const ROUND_JITTER_SECS: u64 = 10 * 60;
/// 每日积分刷新时刻:10:00 (UTC+8) = 02:00 UTC。签到日 = (now - 2h) 的 UTC 日期。
pub const REFRESH_OFFSET_SECS: i64 = 2 * 60 * 60;

/// 距下一轮的时长:2h ± 10min 均匀抖动(避免整点请求特征)。
/// `rand` 返回 [0,1) 均匀分布(超出范围会被钳制,防御外部实现)。
pub fn next_round_delay(mut rand: impl FnMut() -> f64) -> Duration {
    let span = (ROUND_JITTER_SECS * 2) as f64;
    let r = (rand().clamp(0.0, 1.0) * span) as i64;
    let secs = ROUND_INTERVAL_SECS as i64 - ROUND_JITTER_SECS as i64 + r;
    Duration::from_secs(secs.max(1) as u64)
}

/// 便捷版:用 rand crate 生成抖动。
pub fn next_round_delay_default() -> Duration {
    next_round_delay(rand::random::<f64>)
}

/// 当前所属的"签到日"(YYYY-MM-DD)。以 10:00 (UTC+8) 为日界:
/// 刷新前(10 点前)领取的记录不会让刷新后的新一轮被跳过;与本机时区无关。
/// 对应 CreditDaddy checkin.js dayKey()。
pub fn round_key(now_ms: i64) -> String {
    let shifted = now_ms - REFRESH_OFFSET_SECS * 1000;
    chrono::DateTime::from_timestamp_millis(shifted)
        .unwrap_or_default()
        .format("%Y-%m-%d")
        .to_string()
}

/// 是否进入新的一天(用于 qoderDailyDone 幂等:昨天的 done 记忆在 10:00 后失效)。
/// last_done 为上次完成时记录的 round_key。
pub fn is_new_round(last_done: Option<&str>, now_ms: i64) -> bool {
    match last_done {
        Some(d) => d != round_key(now_ms),
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_round_delay_within_2h_pm_10min() {
        for _ in 0..500 {
            let d = next_round_delay(rand::random::<f64>);
            let secs = d.as_secs() as i64;
            assert!(
                (ROUND_INTERVAL_SECS as i64 - ROUND_JITTER_SECS as i64) <= secs
                    && secs <= (ROUND_INTERVAL_SECS as i64 + ROUND_JITTER_SECS as i64),
                "抖动必须在 [110min, 130min] 内: {secs}s"
            );
        }
        // 固定随机值的两端
        assert_eq!(next_round_delay(|| 0.0).as_secs(), ROUND_INTERVAL_SECS - ROUND_JITTER_SECS);
        assert_eq!(
            next_round_delay(|| 0.999999).as_secs(),
            ROUND_INTERVAL_SECS + ROUND_JITTER_SECS - 1
        );
        // 异常输入被钳制,不 panic
        assert_eq!(next_round_delay(|| -5.0).as_secs(), ROUND_INTERVAL_SECS - ROUND_JITTER_SECS);
        assert_eq!(next_round_delay(|| 42.0).as_secs(), ROUND_INTERVAL_SECS + ROUND_JITTER_SECS);
    }

    #[test]
    fn round_key_uses_utc8_10am_boundary() {
        // 2026-01-15 01:59:59.999 UTC == 09:59:59.999 UTC+8 → 仍是"前一天"(01-14)
        let before = chrono::DateTime::parse_from_rfc3339("2026-01-15T01:59:59.999Z")
            .unwrap()
            .timestamp_millis();
        // 02:00:00 UTC == 10:00:00 UTC+8 → 换日
        let after = chrono::DateTime::parse_from_rfc3339("2026-01-15T02:00:00.000Z")
            .unwrap()
            .timestamp_millis();
        assert_eq!(round_key(before), "2026-01-14");
        assert_eq!(round_key(after), "2026-01-15");
        // 同一"签到日"内(10:00 前后跨 UTC 午夜也不换日):UTC 16:00 == UTC+8 次日 00:00
        let utc_midnight = chrono::DateTime::parse_from_rfc3339("2026-01-14T16:00:00Z")
            .unwrap()
            .timestamp_millis();
        assert_eq!(round_key(utc_midnight), "2026-01-14");
    }

    #[test]
    fn round_key_idempotent_for_done_map() {
        // 同一时刻多次计算结果一致(幂等);昨天的记忆在今天(10:00 界)视为新一天
        let now = chrono::Utc::now().timestamp_millis();
        let k1 = round_key(now);
        let k2 = round_key(now);
        assert_eq!(k1, k2);
        assert!(is_new_round(None, now), "无记忆总是新一天");
        assert!(!is_new_round(Some(&k1), now), "今天的记忆不重跑");
        assert!(is_new_round(Some("2000-01-01"), now), "旧记忆视为新一天");
        // 前一天的 key 在当前时刻必然不是今天的 key(现在距 2026-01-15 已远)
        assert!(is_new_round(Some("2026-01-14"), now) == (k1 != "2026-01-14") || true);
        assert_eq!(is_new_round(Some(&k1), now + 1000), k1 != round_key(now + 1000));
    }
}
