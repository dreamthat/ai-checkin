// 数据模型 — 移植自 CreditDaddy(store.js accounts 结构 / zcodeLocal.js / checkin.js)。
// serde camelCase 与前端字段对齐;可选字段全部带 default,保证旧数据兼容。

use serde::{Deserialize, Serialize};

/// 领取/签到结果(协议口径,与 CreditDaddy checkinOne/zcodeAutoClaim 一致):
/// checked-in 成功领取 / already 今日已领 / no-activity 无可领活动 / failed 失败 / need-captcha 需验证码。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClaimOutcome {
    #[serde(rename = "checked-in")]
    CheckedIn,
    #[serde(rename = "already")]
    Already,
    #[serde(rename = "no-activity")]
    NoActivity,
    #[serde(rename = "failed")]
    Failed,
    #[serde(rename = "need-captcha")]
    NeedCaptcha,
}

impl ClaimOutcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::CheckedIn => "checked-in",
            Self::Already => "already",
            Self::NoActivity => "no-activity",
            Self::Failed => "failed",
            Self::NeedCaptcha => "need-captcha",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "checked-in" => Self::CheckedIn,
            "already" => Self::Already,
            "no-activity" => Self::NoActivity,
            "failed" => Self::Failed,
            "need-captcha" => Self::NeedCaptcha,
            _ => return None,
        })
    }

    /// 视为"成功口径"(写入每日完成记忆):成功领取或已领过。
    pub fn is_success(&self) -> bool {
        matches!(self, Self::CheckedIn | Self::Already)
    }
}

impl std::fmt::Display for ClaimOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Qoder 账号(字段参照 CreditDaddy store.js normalizeAccountInput + checkin.js lastResult 回写)。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct QoderAccount {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub token: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub refresh_token: Option<String>,
    /// Qoder 用户 ID(uid,首次领取后从 campaigns 响应回写)
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub user_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub email: Option<String>,
    /// 区域:"intl"(国际版) | "cn"(国内版)
    #[serde(default = "default_region")]
    pub region: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub created_at: i64,
    /// 凭据过期时间(ISO 字符串,来自本地客户端导入)
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub expires_at: Option<String>,
    /// 来源:"manual" 手动录入 | "local-app" 本机客户端导入 | "device" 浏览器授权
    #[serde(default = "default_source")]
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub last_claim_at: Option<i64>,
    /// 最近一次结果(ClaimOutcome 字符串)
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub last_result: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub last_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub last_claimed_amount: Option<i64>,
    /// 额度查询缓存(/api/v2/quota/usage 原始 JSON)
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub quota: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub quota_updated_at: Option<i64>,
    /// 冷却(领取失败后的暂停,Unix 毫秒;None=不在冷却)
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cooldown_until: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cooldown_reason: Option<String>,
}

/// ZCode 账号(字段参照 CreditDaddy zcodeLocal.js liveToAccount)。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ZCodeAccount {
    pub id: String,
    pub name: String,
    /// 手动录入时为直接 token;本机导入时为可读标记 "zcode-creds:<uid>"(真实凭据在 credentials 快照)
    #[serde(default)]
    pub token: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub refresh_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub user_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub email: Option<String>,
    /// 区域(预留,ZCode 无国内/国际之分,固定 "intl")
    #[serde(default = "default_region")]
    pub region: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default = "default_source")]
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub last_claim_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub last_result: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub last_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub last_claimed_amount: Option<i64>,
    /// 额度查询缓存(QuotaSnapshot JSON)
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub quota: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub quota_updated_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cooldown_until: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cooldown_reason: Option<String>,
    /// 本机导入的 credentials.json 快照(各字段保持 enc:v1 密文)
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub credentials: Option<serde_json::Value>,
    /// 本机导入的 config.json 快照(部分账号在此存明文 API Key)
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub config: Option<serde_json::Value>,
    /// 虚拟设备 ID(导入时取本机 telemetry-state.json 的 deviceMid)
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub device_mid: Option<String>,
    /// 去重哈希(canonicalHash,同一登录识别)
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub canonical_hash: Option<String>,
}

fn default_region() -> String {
    "intl".into()
}
fn default_source() -> String {
    "manual".into()
}
fn yes() -> bool {
    true
}

/// Qoder 设置。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QoderSettings {
    /// 自动领取开关(定时轮)
    pub auto_claim_enabled: bool,
    /// 轮询间隔(分钟),最短 30
    pub interval_min: u32,
    /// 新账号默认区域:"intl" | "cn"
    pub region: String,
}

impl Default for QoderSettings {
    fn default() -> Self {
        Self {
            auto_claim_enabled: false,
            interval_min: 120,
            region: "intl".into(),
        }
    }
}

/// ZCode 设置。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ZcodeSettings {
    pub auto_claim_enabled: bool,
    pub interval_min: u32,
}

impl Default for ZcodeSettings {
    fn default() -> Self {
        Self {
            auto_claim_enabled: false,
            interval_min: 120,
        }
    }
}

/// 设置的部分更新(前端 saveSettings 传 partial;两个平台共用,Qoder 额外消费 region)。
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PartialSettings {
    #[serde(default)]
    pub auto_claim_enabled: Option<bool>,
    #[serde(default)]
    pub interval_min: Option<u32>,
    #[serde(default)]
    pub region: Option<String>,
}

/// 设置可被 PartialSettings 合并(Qoder 消费 region,ZCode 忽略)。
pub trait CreditSettings:
    Default + Clone + Serialize + serde::de::DeserializeOwned + Send + Sync
{
    fn merge_partial(&mut self, p: &PartialSettings);
}

const MIN_INTERVAL_MIN: u32 = 30;

impl CreditSettings for QoderSettings {
    fn merge_partial(&mut self, p: &PartialSettings) {
        if let Some(v) = p.auto_claim_enabled {
            self.auto_claim_enabled = v;
        }
        if let Some(v) = p.interval_min {
            self.interval_min = v.max(MIN_INTERVAL_MIN);
        }
        if let Some(v) = p.region.clone() {
            if v == "intl" || v == "cn" {
                self.region = v;
            }
        }
    }
}

impl CreditSettings for ZcodeSettings {
    fn merge_partial(&mut self, p: &PartialSettings) {
        if let Some(v) = p.auto_claim_enabled {
            self.auto_claim_enabled = v;
        }
        if let Some(v) = p.interval_min {
            self.interval_min = v.max(MIN_INTERVAL_MIN);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claim_outcome_serde_roundtrip() {
        for o in [
            ClaimOutcome::CheckedIn,
            ClaimOutcome::Already,
            ClaimOutcome::NoActivity,
            ClaimOutcome::Failed,
            ClaimOutcome::NeedCaptcha,
        ] {
            let s = serde_json::to_string(&o).unwrap();
            assert_eq!(s, format!("\"{}\"", o.as_str()), "协议字符串必须与 CreditDaddy 一致");
            let back: ClaimOutcome = serde_json::from_str(&s).unwrap();
            assert_eq!(back, o);
            assert_eq!(ClaimOutcome::parse(o.as_str()), Some(o));
        }
        assert_eq!(ClaimOutcome::parse("other"), None);
        assert!(ClaimOutcome::CheckedIn.is_success());
        assert!(ClaimOutcome::Already.is_success());
        assert!(!ClaimOutcome::NoActivity.is_success());
        assert!(!ClaimOutcome::NeedCaptcha.is_success());
    }

    #[test]
    fn qoder_account_serde_roundtrip_camel_case() {
        let a = QoderAccount {
            id: "a1".into(),
            name: "测试".into(),
            token: "dt-abc1234567890abcdef".into(),
            user_id: Some("u1".into()),
            email: Some("a@b.c".into()),
            region: "cn".into(),
            enabled: true,
            last_claim_at: Some(123),
            last_result: Some("checked-in".into()),
            last_message: Some("领取成功 +100 Credits".into()),
            last_claimed_amount: Some(100),
            quota: Some(serde_json::json!({"x": 1})),
            cooldown_until: Some(456),
            cooldown_reason: Some("限频".into()),
            ..Default::default()
        };
        let v = serde_json::to_value(&a).unwrap();
        // camelCase 键名断言
        assert!(v.get("refreshToken").is_some() || v.get("userId").is_some());
        assert_eq!(v["userId"], "u1");
        assert_eq!(v["lastClaimAt"], 123);
        assert_eq!(v["lastResult"], "checked-in");
        assert_eq!(v["lastClaimedAmount"], 100);
        assert_eq!(v["cooldownUntil"], 456);
        // 往返
        let back: QoderAccount = serde_json::from_value(v).unwrap();
        assert_eq!(back.user_id.as_deref(), Some("u1"));
        assert_eq!(back.last_result.as_deref(), Some("checked-in"));
        // 反序列化容忍缺字段(旧数据兼容)
        let minimal: QoderAccount = serde_json::from_value(serde_json::json!({"id": "x", "name": "n"})).unwrap();
        assert_eq!(minimal.region, "intl");
        assert!(minimal.enabled);
    }

    #[test]
    fn settings_merge_partial() {
        let mut qs = QoderSettings::default();
        qs.merge_partial(&PartialSettings {
            auto_claim_enabled: Some(true),
            interval_min: Some(10), // 低于下限 30,被钳制
            region: Some("cn".into()),
        });
        assert!(qs.auto_claim_enabled);
        assert_eq!(qs.interval_min, 30);
        assert_eq!(qs.region, "cn");

        let mut zs = ZcodeSettings::default();
        zs.merge_partial(&PartialSettings {
            auto_claim_enabled: Some(true),
            interval_min: Some(90),
            region: Some("cn".into()), // ZCode 忽略 region
        });
        assert!(zs.auto_claim_enabled);
        assert_eq!(zs.interval_min, 90);
        let v = serde_json::to_value(&zs).unwrap();
        assert!(v.get("region").is_none());
    }
}
