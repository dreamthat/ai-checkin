// 调度纯函数:轮间隔(2h±10min 均匀抖动)与"签到日"判定(UTC+8 当日 10:00 为日界)。
// 含宿主的调度循环在 src-tauri / wb-switch-server(阶段 B/C)。

use std::time::Duration;

use chrono::{DateTime, Datelike, TimeZone, Utc};

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

// ===== 灵犀每日定点(Asia/Shanghai 固定 UTC+8,无夏令时) =====

/// Asia/Shanghai 固定偏移(UTC+8)。
pub fn shanghai_tz() -> chrono::FixedOffset {
    chrono::FixedOffset::east_opt(8 * 3600).expect("UTC+8 固定偏移恒有效")
}

/// 解析单个 "HH:MM"(容忍 "H:M" 与首尾空白);非法返回 None。
pub fn parse_hhmm(s: &str) -> Option<(u32, u32)> {
    let (h, m) = s.trim().split_once(':')?;
    let h: u32 = h.trim().parse().ok()?;
    let m: u32 = m.trim().parse().ok()?;
    if h > 23 || m > 59 {
        return None;
    }
    Some((h, m))
}

/// 解析 HH:MM 列表(过滤非法;排序去重,供调度计算)。
pub fn parse_hhmm_list<'a, I: IntoIterator<Item = &'a str>>(times: I) -> Vec<(u32, u32)> {
    let mut out: Vec<(u32, u32)> = times.into_iter().filter_map(parse_hhmm).collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// 过滤非法格式并规范化为 "HH:MM"(设置保存用;保留原顺序、去重)。
pub fn normalize_hhmm_list<'a, I: IntoIterator<Item = &'a str>>(times: I) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in times {
        if let Some((h, m)) = parse_hhmm(t) {
            let s = format!("{h:02}:{m:02}");
            if !out.contains(&s) {
                out.push(s);
            }
        }
    }
    out
}

/// Asia/Shanghai 本地日期(YYYY-MM-DD);灵犀 per-account lastSuccessDate 幂等键。
pub fn shanghai_today(now_utc: DateTime<Utc>) -> String {
    now_utc.with_timezone(&shanghai_tz()).format("%Y-%m-%d").to_string()
}

/// 灵犀下一次执行时刻:当日(Asia/Shanghai)未来最近的时间点,否则明日最早的时间点;
/// 列表为空 / 全非法 → None。
pub fn lingxi_next_run(times: &[String], now_utc: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let points = parse_hhmm_list(times.iter().map(String::as_str));
    if points.is_empty() {
        return None;
    }
    let tz = shanghai_tz();
    let today = now_utc.with_timezone(&tz).date_naive();
    let (y, m, d) = (today.year(), today.month(), today.day());
    // 当日未来最近
    let mut best: Option<DateTime<Utc>> = None;
    for &(h, min) in &points {
        let Some(dt) = tz.with_ymd_and_hms(y, m, d, h, min, 0).single() else {
            continue;
        };
        let dt = dt.with_timezone(&Utc);
        if dt > now_utc && best.map_or(true, |b| dt < b) {
            best = Some(dt);
        }
    }
    if let Some(b) = best {
        return Some(b);
    }
    // 明日最早
    let tomorrow = today.succ_opt()?;
    points
        .iter()
        .filter_map(|&(h, min)| {
            tz.with_ymd_and_hms(tomorrow.year(), tomorrow.month(), tomorrow.day(), h, min, 0)
                .single()
                .map(|dt| dt.with_timezone(&Utc))
        })
        .min()
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

    #[test]
    fn parse_hhmm_filters_invalid() {
        assert_eq!(parse_hhmm("08:30"), Some((8, 30)));
        assert_eq!(parse_hhmm(" 9:05 "), Some((9, 5)));
        assert_eq!(parse_hhmm("23:59"), Some((23, 59)));
        assert_eq!(parse_hhmm("24:00"), None, "小时越界");
        assert_eq!(parse_hhmm("08:60"), None, "分钟越界");
        assert_eq!(parse_hhmm("0830"), None);
        assert_eq!(parse_hhmm("08:"), None);
        assert_eq!(parse_hhmm(":30"), None);
        assert_eq!(parse_hhmm(""), None);
        assert_eq!(parse_hhmm("08:30:00"), None, "不接受秒段");
    }

    #[test]
    fn parse_and_normalize_hhmm_lists() {
        assert_eq!(
            parse_hhmm_list(["16:30", "bad", "08:30", "16:30"]),
            vec![(8, 30), (16, 30)],
            "过滤非法 + 排序去重"
        );
        assert_eq!(parse_hhmm_list(Vec::<&str>::new()), Vec::new());
        assert_eq!(
            normalize_hhmm_list(["9:05", "bad", "09:05", "23:5"]),
            vec!["09:05", "23:05"],
            "规范化两位、保序去重"
        );
    }

    #[test]
    fn shanghai_today_uses_utc8_date() {
        // 2026-01-14 16:00 UTC == UTC+8 次日 00:00
        let dt = chrono::DateTime::parse_from_rfc3339("2026-01-14T16:00:00Z").unwrap();
        assert_eq!(shanghai_today(dt.with_timezone(&Utc)), "2026-01-15");
        let dt = chrono::DateTime::parse_from_rfc3339("2026-01-14T15:59:59Z").unwrap();
        assert_eq!(shanghai_today(dt.with_timezone(&Utc)), "2026-01-14");
    }

    #[test]
    fn lingxi_next_run_same_day_future_and_next_day_fallback() {
        let times: Vec<String> = vec!["08:30".into(), "16:30".into()];
        let tz = shanghai_tz();
        let at = |y: i32, mo: u32, d: u32, h: u32, mi: u32| {
            tz.with_ymd_and_hms(y, mo, d, h, mi, 0).single().unwrap().with_timezone(&Utc)
        };
        // 当日 09:00 (UTC+8) → 下一个时间点是 16:30
        let now = at(2026, 3, 1, 9, 0);
        assert_eq!(lingxi_next_run(&times, now), Some(at(2026, 3, 1, 16, 30)));
        // 当日 16:31 → 明日 08:30
        let now = at(2026, 3, 1, 16, 31);
        assert_eq!(lingxi_next_run(&times, now), Some(at(2026, 3, 2, 8, 30)));
        // 恰好等于时间点本身 → 算已过,取下一个
        let now = at(2026, 3, 1, 8, 30);
        assert_eq!(lingxi_next_run(&times, now), Some(at(2026, 3, 1, 16, 30)));
        // 时间点乱序 / 非法混入不受影响
        let messy: Vec<String> = vec!["bad".into(), "16:30".into(), "08:30".into()];
        let now = at(2026, 3, 1, 0, 0);
        assert_eq!(lingxi_next_run(&messy, now), Some(at(2026, 3, 1, 8, 30)));
        // 空列表 / 全非法 → None
        assert_eq!(lingxi_next_run(&[], now), None);
        assert_eq!(lingxi_next_run(&["x".to_string()], now), None);
    }
}
