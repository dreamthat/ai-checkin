// 定时签到调度纯函数(自 trae-mate scheduler.rs 拆出)。
// 含 AppHandle 的调度循环在宿主层(src-tauri/src/trae_scheduler.rs / server main.rs)。

use chrono_tz::Asia::Shanghai;

use crate::models::AppSettings;

/// 解析 "HH:mm"(容忍空格/缺零),非法返回 None。
pub fn parse_hhmm(s: &str) -> Option<(u32, u32)> {
    let mut parts = s.trim().split(':');
    let h: u32 = parts.next()?.trim().parse().ok()?;
    let m: u32 = parts.next()?.trim().parse().ok()?;
    if h > 23 || m > 59 {
        return None;
    }
    Some((h, m))
}

/// 计算下次执行时间(Asia/Shanghai 时区,每日定点),已过今天时间点则取明天。
pub fn next_run_instant(settings: &AppSettings) -> Option<chrono::DateTime<chrono::Utc>> {
    let (h, m) = parse_hhmm(&settings.checkin_time)?;
    let now = chrono::Utc::now().with_timezone(&Shanghai);
    let today = now.date_naive().and_hms_opt(h, m, 0)?;
    let today = today.and_local_timezone(Shanghai).single()?;
    let next = if today > now {
        today
    } else {
        today + chrono::Duration::days(1)
    };
    Some(next.with_timezone(&chrono::Utc))
}

/// 距下次执行的时长,未配置/非法时间返回 None。
pub fn next_run_duration(settings: &AppSettings) -> Option<std::time::Duration> {
    let next = next_run_instant(settings)?;
    let now = chrono::Utc::now();
    (next - now).to_std().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(time: &str) -> AppSettings {
        AppSettings {
            checkin_time: time.into(),
            ..Default::default()
        }
    }

    #[test]
    fn parse_hhmm_variants() {
        assert_eq!(parse_hhmm("08:00"), Some((8, 0)));
        assert_eq!(parse_hhmm("8:5"), Some((8, 5)));
        assert_eq!(parse_hhmm(" 23:59 "), Some((23, 59)));
        assert_eq!(parse_hhmm("24:00"), None);
        assert_eq!(parse_hhmm("08:60"), None);
        assert_eq!(parse_hhmm("abc"), None);
        assert_eq!(parse_hhmm(""), None);
    }

    #[test]
    fn next_run_is_future_and_daily() {
        let s = settings("08:00");
        let next = next_run_instant(&s).expect("合法时间应能算出下次执行");
        let now = chrono::Utc::now();
        assert!(next > now, "下次执行必须在未来");
        // 与上海时区当前时刻差不超过 24h
        let sh_now = now.with_timezone(&Shanghai);
        let sh_next = next.with_timezone(&Shanghai);
        let diff = sh_next - sh_now;
        assert!(diff.num_hours() <= 24, "每日定点调度间隔不超过 24h: {diff}");
    }

    #[test]
    fn next_run_invalid_time_is_none() {
        assert!(next_run_instant(&settings("bad")).is_none());
        assert!(next_run_instant(&settings("25:00")).is_none());
    }
}
