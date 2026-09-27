//! 灵犀(金山)多用户签到模块。协议极简:POST 用户从浏览器 F12 抓包的 checkinUrl,
//! headers 仅固定 UA + Cookie(每账号各自的 URL 与 Cookie,无本地凭据检测),
//! 对响应文本(非结构化 JSON)做三级判定:成功 / 已签到 / 失败 / 未知。
//! 协议事实来源:Lingxi-Check-in 源项目(lib/Lingxi.js)。

pub mod checkin;
pub mod local_import;

use serde::{Deserialize, Serialize};

use crate::models::{CreditSettings, PartialSettings};
use crate::schedule;

/// 灵犀官方每日签到接口(实测验证:POST 领取当日智点,响应含 task_key/reward_amount)。
/// 添加账号时 checkinUrl 留空即用此默认值,仍可在账号卡片「编辑」中改为抓包地址。
pub const DEFAULT_CHECKIN_URL: &str = "https://lingxi.kdocs.cn/api/public/v1/tasks/daily_check_in/claim";

/// 灵犀账号(字段对齐前端卡片展示;checkinUrl / cookie 由用户抓包手动获取)。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LingxiAccount {
    pub id: String,
    pub name: String,
    /// 签到接口完整 URL(浏览器 F12 → Network → 手动签到 → Copy as cURL 提取)。
    #[serde(default)]
    pub checkin_url: String,
    /// 该账号的登录 Cookie(与 checkinUrl 同一次抓包提取)。
    #[serde(default)]
    pub cookie: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub created_at: i64,
    /// 最近一次实际发起签到的时间(Unix 毫秒;skipped 跳过不写)。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub last_checkin_at: Option<i64>,
    /// 最近一次结果(Outcome 字符串:success/already/unknown/failed/skipped)。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub last_result: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub last_message: Option<String>,
    /// 当日成功签到日期(YYYY-MM-DD,Asia/Shanghai)——每日幂等跳过的依据。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub last_success_date: Option<String>,
}

fn yes() -> bool {
    true
}

/// 灵犀签到结果口径(协议字符串与 Lingxi-Check-in 状态语义一致):
/// success 签到成功 / already 已签到(已签过) / unknown 响应无法识别 / failed 失败 / skipped 当日已成功跳过。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Outcome {
    #[serde(rename = "success")]
    Success,
    #[serde(rename = "already")]
    Already,
    #[serde(rename = "unknown")]
    Unknown,
    #[serde(rename = "failed")]
    Failed,
    #[serde(rename = "skipped")]
    Skipped,
}

impl Outcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Already => "already",
            Self::Unknown => "unknown",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "success" => Self::Success,
            "already" => Self::Already,
            "unknown" => Self::Unknown,
            "failed" => Self::Failed,
            "skipped" => Self::Skipped,
            _ => return None,
        })
    }

    /// 视为"成功口径"(写入 lastSuccessDate 每日幂等记忆):签到成功或已签到。
    pub fn is_success(&self) -> bool {
        matches!(self, Self::Success | Self::Already)
    }
}

impl std::fmt::Display for Outcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 响应文本三级判定(纯函数,对齐 Lingxi-Check-in 的 res 小写 + 去空格口径):
/// - Failed:含「未登录」「失败」,或 compact 含 `"error"` 且不含 `"error":0`
/// - Already:含「已签到」「已签过」(先于「成功」判定,幂等语义优先)
/// - Success:含「成功」,或 compact 含 `"success":true` / `"error":0`
/// - Unknown:都不匹配(不写成功状态,下个时间点重试)
pub fn classify_response(text: &str) -> Outcome {
    let lower = text.to_lowercase();
    let res_compact = lower.replace(' ', "");
    if lower.contains("未登录") || lower.contains("失败") {
        return Outcome::Failed;
    }
    if res_compact.contains("\"error\"") && !res_compact.contains("\"error\":0") {
        return Outcome::Failed;
    }
    if lower.contains("已签到") || lower.contains("已签过") {
        return Outcome::Already;
    }
    // 官方任务中心 claim 接口(lingxi.kdocs.cn/api/public/v1/tasks/daily_check_in/claim)
    // 的成功响应。两层形态都覆盖:
    //   ① 网页实测(2026-09):{"data":{...,"result":"ok"},"vCode":0}
    //   ② 业务数据体:{task_key:"daily_check_in", reward_amount, trade_no:"lx_task_...",
    //      total_claimed_credits, extra:{check_in_days:[...], consecutive_days, ...}}
    // task_key=日常签到 与 total_claimed_credits(累计到账智点)只在领取成功响应中出现。
    if lower.contains("成功")
        || res_compact.contains("\"success\":true")
        || res_compact.contains("\"error\":0")
        || res_compact.contains("\"result\":\"ok\"")
        || res_compact.contains("\"vcode\":0")
        || res_compact.contains("\"task_key\":\"daily_check_in\"")
        || res_compact.contains("\"total_claimed_credits\"")
    {
        return Outcome::Success;
    }
    Outcome::Unknown
}

/// 灵犀设置:每日多时间点签到(Asia/Shanghai)+ 成功/失败通知开关。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LingxiSettings {
    /// 每日签到时间点(HH:MM 列表;保存时过滤非法格式,全非法则不调度)。
    pub checkin_times: Vec<String>,
    pub notify_on_success: bool,
    pub notify_on_failed: bool,
}

impl Default for LingxiSettings {
    fn default() -> Self {
        Self {
            checkin_times: ["08:30", "16:30"].iter().map(|s| s.to_string()).collect(),
            notify_on_success: true,
            notify_on_failed: true,
        }
    }
}

/// 灵犀设置的部分更新(与 Qoder/ZCode 的 PartialSettings 分离:调度模型是每日定点而非轮间隔)。
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PartialLingxiSettings {
    #[serde(default)]
    pub checkin_times: Option<Vec<String>>,
    #[serde(default)]
    pub notify_on_success: Option<bool>,
    #[serde(default)]
    pub notify_on_failed: Option<bool>,
}

impl LingxiSettings {
    /// 部分合并;checkinTimes 过滤非法格式(规范化 HH:MM、去重)。
    pub fn merge_partial(&mut self, p: &PartialLingxiSettings) {
        if let Some(times) = &p.checkin_times {
            self.checkin_times = schedule::normalize_hhmm_list(times.iter().map(String::as_str));
        }
        if let Some(v) = p.notify_on_success {
            self.notify_on_success = v;
        }
        if let Some(v) = p.notify_on_failed {
            self.notify_on_failed = v;
        }
    }
}

/// CreditSettings 是 CreditState 泛型存储的约束;灵犀设置不经 PartialSettings 通道修改
/// (走 PartialLingxiSettings 专用通道,此处保持空实现)。
impl CreditSettings for LingxiSettings {
    fn merge_partial(&mut self, _p: &PartialSettings) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_response_three_levels() {
        // —— Failed:未登录 / 失败 ——
        assert_eq!(classify_response("当前未登录，请重新登录"), Outcome::Failed);
        assert_eq!(classify_response("签到失败"), Outcome::Failed);
        assert_eq!(classify_response(r#"{"error":1,"msg":"bad cookie"}"#), Outcome::Failed);
        // 去空格后命中:`"error" : 1` → `"error":1`
        assert_eq!(classify_response(r#"{ "error" : 1 }"#), Outcome::Failed);
        assert_eq!(classify_response(r#"{"error":"invalid"}"#), Outcome::Failed);
        // —— Already:已签到 / 已签过(先于「成功」判定) ——
        assert_eq!(classify_response("今日已签到"), Outcome::Already);
        assert_eq!(classify_response("您已签过到了"), Outcome::Already);
        assert_eq!(classify_response("签到成功，今日已签到"), Outcome::Already);
        // —— Success:成功 / success:true / error:0 ——
        assert_eq!(classify_response("签到成功"), Outcome::Success);
        assert_eq!(classify_response(r#"{"success":true}"#), Outcome::Success);
        assert_eq!(classify_response(r#"{ "success" : true }"#), Outcome::Success);
        assert_eq!(classify_response(r#"{"error":0,"msg":"ok"}"#), Outcome::Success);
        // 大小写不敏感(HTML/JSON 混合响应)
        assert_eq!(classify_response(r#"{"Success":TRUE}"#), Outcome::Success);
        // —— Success:官方 claim 接口实测形态(2026-09 截图,lx_task_8045248 签到到账 100) ——
        let claim_ok = r#"{"data":{"task_key":"daily_check_in","reward_amount":100,"trade_no":"lx_task_8045248_daily_check_in_1902252","total_claimed_credits":100,"extras":{"check_in_days":[{"day":1,"date":"2026-09-27","reward":100,"status":"claimed"}]}},"vCode":0}"#;
        assert_eq!(classify_response(claim_ok), Outcome::Success);
        assert_eq!(classify_response(r#"{"result":"ok"}"#), Outcome::Success);
        // 官方文档化业务数据体形态(无 result/vCode 包裹):task_key + total_claimed_credits
        let claim_body = r#"{"task_key":"daily_check_in","reward_amount":100,"trade_no":"lx_task_8045248_daily_check_in_1902252","total_claimed_credits":100,"extra":{"check_in_days":[{"day":1,"date":"2026-09-27","reward":100,"status":"claimed"},{"day":7,"date":"2026-10-03","reward":200,"status":"locked"}],"consecutive_days":1,"today_day":1,"today_reward":100}}"#;
        assert_eq!(classify_response(claim_body), Outcome::Success);
        // vCode 非 0 不得命中 vcode:0 特征(无其他特征时归未知,交人工确认)
        assert_eq!(classify_response(r#"{"vCode":1003,"msg":"操作太过频繁"}"#), Outcome::Unknown);
        // —— Unknown:都不匹配 → 不写成功状态 ——
        assert_eq!(classify_response("hello world"), Outcome::Unknown);
        assert_eq!(classify_response(""), Outcome::Unknown);
        assert_eq!(classify_response(r#"{"code":200}"#), Outcome::Unknown);
        // 「成功」存在但带 error 非 0 → 失败优先
        assert_eq!(classify_response(r#"{"error":2,"msg":"操作成功"}"#), Outcome::Failed);
    }

    #[test]
    fn outcome_serde_and_success_semantics() {
        for o in [
            Outcome::Success,
            Outcome::Already,
            Outcome::Unknown,
            Outcome::Failed,
            Outcome::Skipped,
        ] {
            let s = serde_json::to_string(&o).unwrap();
            assert_eq!(s, format!("\"{}\"", o.as_str()));
            let back: Outcome = serde_json::from_str(&s).unwrap();
            assert_eq!(back, o);
            assert_eq!(Outcome::parse(o.as_str()), Some(o));
        }
        assert_eq!(Outcome::parse("other"), None);
        assert!(Outcome::Success.is_success());
        assert!(Outcome::Already.is_success());
        assert!(!Outcome::Unknown.is_success());
        assert!(!Outcome::Failed.is_success());
        assert!(!Outcome::Skipped.is_success());
    }

    #[test]
    fn lingxi_account_serde_roundtrip_camel_case() {
        let a = LingxiAccount {
            id: "l1".into(),
            name: "灵犀账号".into(),
            checkin_url: "https://lingxi.example.com/api/checkin".into(),
            cookie: "SESSION=abc".into(),
            enabled: true,
            created_at: 123,
            last_checkin_at: Some(456),
            last_result: Some("success".into()),
            last_message: Some("签到成功".into()),
            last_success_date: Some("2026-09-26".into()),
        };
        let v = serde_json::to_value(&a).unwrap();
        assert_eq!(v["checkinUrl"], "https://lingxi.example.com/api/checkin");
        assert_eq!(v["lastCheckinAt"], 456);
        assert_eq!(v["lastResult"], "success");
        assert_eq!(v["lastSuccessDate"], "2026-09-26");
        let back: LingxiAccount = serde_json::from_value(v).unwrap();
        assert_eq!(back.last_success_date.as_deref(), Some("2026-09-26"));
        // 反序列化容忍缺字段(旧数据兼容)
        let minimal: LingxiAccount = serde_json::from_value(serde_json::json!({"id": "x", "name": "n"})).unwrap();
        assert!(minimal.enabled);
        assert!(minimal.checkin_url.is_empty());
    }

    #[test]
    fn lingxi_settings_merge_partial_filters_invalid_times() {
        let mut s = LingxiSettings::default();
        assert_eq!(s.checkin_times, vec!["08:30", "16:30"]);
        s.merge_partial(&PartialLingxiSettings {
            checkin_times: Some(vec!["9:05".into(), "25:00".into(), "08:30 ".into(), "abc".into(), "23:59".into(), "9:05".into()]),
            notify_on_success: Some(false),
            notify_on_failed: None,
        });
        assert_eq!(s.checkin_times, vec!["09:05", "08:30", "23:59"], "非法剔除、规范化、保序去重");
        assert!(!s.notify_on_success);
        assert!(s.notify_on_failed);
        // 全非法 → 空列表(不调度)
        s.merge_partial(&PartialLingxiSettings { checkin_times: Some(vec!["x".into()]), ..Default::default() });
        assert!(s.checkin_times.is_empty());
    }
}
