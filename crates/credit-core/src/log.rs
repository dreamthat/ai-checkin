// 签到/领取日志(仿 trae-core CheckinLog)。append/list/clear 以 trait 形式实现在 CreditStoreData 上。

use serde::{Deserialize, Serialize};

use crate::store::CreditStoreData;

pub const PLATFORM_QODER: &str = "qoder";
pub const PLATFORM_ZCODE: &str = "zcode";
pub const PLATFORM_LINGXI: &str = "lingxi";
/// 日志上限(超出丢弃最旧)
pub const MAX_LOGS: usize = 500;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    pub time: i64,
    pub account_id: String,
    pub account_name: String,
    /// 平台:"qoder" | "zcode" | "lingxi"
    pub platform: String,
    /// 结果(ClaimOutcome 字符串)
    pub result: String,
    pub message: String,
}

impl LogEntry {
    pub fn new(account_id: &str, account_name: &str, platform: &str, result: &str, message: &str) -> Self {
        Self {
            time: chrono::Utc::now().timestamp_millis(),
            account_id: account_id.into(),
            account_name: account_name.into(),
            platform: platform.into(),
            result: result.into(),
            message: message.into(),
        }
    }
}

/// 日志存取能力(由 CreditStoreData 实现)。
pub trait LogStore {
    fn append_log(&mut self, entry: LogEntry);
    fn list_logs(&self, limit: usize) -> Vec<LogEntry>;
    fn clear_logs(&mut self);
}

impl<A, S> LogStore for CreditStoreData<A, S> {
    /// 追加一条日志,超过 MAX_LOGS 丢弃最旧。
    fn append_log(&mut self, entry: LogEntry) {
        self.logs.push(entry);
        if self.logs.len() > MAX_LOGS {
            let drop_n = self.logs.len() - MAX_LOGS;
            self.logs.drain(0..drop_n);
        }
    }

    /// 最近 limit 条,最新在前(对应原 slice(-limit).reverse())。
    fn list_logs(&self, limit: usize) -> Vec<LogEntry> {
        let n = self.logs.len();
        let start = n.saturating_sub(limit);
        self.logs[start..].iter().rev().cloned().collect()
    }

    fn clear_logs(&mut self) {
        self.logs.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> CreditStoreData<crate::models::QoderAccount, crate::models::QoderSettings> {
        CreditStoreData::default()
    }

    #[test]
    fn log_append_list_clear() {
        let mut st = store();
        for i in 0..7 {
            st.append_log(LogEntry::new(&format!("a{i}"), "名", PLATFORM_QODER, "checked-in", "ok"));
        }
        let listed = st.list_logs(3);
        assert_eq!(listed.len(), 3);
        assert_eq!(listed[0].account_id, "a6", "最新在前");
        assert_eq!(listed[2].account_id, "a4");
        assert_eq!(listed[0].platform, "qoder");
        assert_eq!(listed[0].result, "checked-in");
        st.clear_logs();
        assert!(st.list_logs(10).is_empty());
    }

    #[test]
    fn log_capacity_trims_oldest() {
        let mut st = store();
        for i in 0..(MAX_LOGS + 10) {
            st.append_log(LogEntry::new(&format!("a{i}"), "名", PLATFORM_ZCODE, "failed", "x"));
        }
        assert_eq!(st.logs.len(), MAX_LOGS);
        assert_eq!(st.logs[0].account_id, "a10", "最旧的 10 条被丢弃");
        let listed = st.list_logs(usize::MAX);
        assert_eq!(listed.len(), MAX_LOGS);
        assert_eq!(listed[0].account_id, format!("a{}", MAX_LOGS + 9));
    }
}
