// Qoder OpenAPI 协议客户端 — 移植自 CreditDaddy constants.js + qoderClient.js。
//
//   GET  /sash/api/v1/me/campaigns?clientType=10   → { uid, campaigns:[…] }
//   POST /sash/api/v1/me/campaigns/{id}/claim
//   GET  /api/v1/userinfo / GET /api/v2/quota/usage
// 国际版必须携带设备风控身份(Cosy-MachineToken/Code/Type)才会下发「每天领 100 Credits」。

use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde_json::Value;

use crate::error::{AppError, AppResult};
use crate::models::ClaimOutcome;

use super::identity::RiskIdentity;

pub const OPENAPI_BASE: &str = "https://openapi.qoder.sh";
pub const CN_OPENAPI_BASE: &str = "https://openapi.qoder.com.cn";
pub const USERINFO_PATH: &str = "/api/v1/userinfo";
pub const QUOTA_USAGE_PATH: &str = "/api/v2/quota/usage";
pub const CAMPAIGNS_PATH: &str = "/sash/api/v1/me/campaigns?clientType=10";
/// 请求超时(任务口径 30s)
pub const TIMEOUT_SECS: u64 = 30;

/// 区域 → base URL。"cn" → 国内版,其余 → 国际版。
pub fn api_base(region: &str) -> &'static str {
    if region == "cn" {
        CN_OPENAPI_BASE
    } else {
        OPENAPI_BASE
    }
}

pub fn campaign_claim_path(campaign_id: &str) -> String {
    format!("/sash/api/v1/me/campaigns/{}/claim", encodeURIComponent(campaign_id))
}

/// JS encodeURIComponent 的等价实现(路径段安全)。
#[allow(non_snake_case)]
fn encodeURIComponent(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'!' | b'~' | b'*'
            | b'\'' | b'(' | b')' => out.push(b as char),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// 设备身份(客户端身份 + 风控身份),由 identity.rs 组装;None 时退回基础请求头。
#[derive(Debug, Clone, Default)]
pub struct DeviceIdentity {
    pub machine_id: String,
    /// 与客户端一致的 Cosy-MachineOS 格式:x86_64_win32 / aarch64_darwin …
    pub machine_os: String,
    pub machine_hostname: Option<String>,
    /// Cosy-Version(探测本机客户端版本,缺省 0.2.5)
    pub client_version: String,
    pub risk: Option<RiskIdentity>,
}

fn ins(h: &mut HeaderMap, k: &'static str, v: String) {
    if let Ok(v) = HeaderValue::from_str(&v) {
        h.insert(HeaderName::from_static(k), v);
    }
}

/// 请求头。identity 为 None → 基础头(constants.js buildQoderHeaders);
/// Some → 叠加客户端身份与设备风控身份(qoderClient.js buildCampaignHeaders)。
/// region 目前不影响请求头本身(区域由 base_url 区分),保留参数与调用约定一致。
pub fn build_headers(token: &str, identity: Option<&DeviceIdentity>, region: &str) -> HeaderMap {
    let _ = region;
    let mut h = HeaderMap::new();
    ins(&mut h, "authorization", format!("Bearer {token}"));
    ins(&mut h, "cosy-clienttype", "10".into());
    ins(
        &mut h,
        "cosy-version",
        identity
            .and_then(|i| (!i.client_version.is_empty()).then(|| i.client_version.clone()))
            .unwrap_or_else(|| "0.3.3".into()),
    );
    ins(
        &mut h,
        "cosy-machineos",
        identity
            .map(|i| i.machine_os.clone())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "windows".into()),
    );
    ins(&mut h, "user-agent", "Qoder".into());
    ins(&mut h, "accept", "application/json".into());
    if let Some(i) = identity {
        if !i.machine_id.is_empty() {
            ins(&mut h, "cosy-machineid", i.machine_id.clone());
        }
        if let Some(host) = i.machine_hostname.as_deref().filter(|s| !s.is_empty()) {
            ins(&mut h, "cosy-machinehostname", host.to_string());
        }
        if let Some(r) = &i.risk {
            ins(&mut h, "cosy-machinetoken", r.machine_token.clone());
            ins(&mut h, "cosy-machinecode", r.machine_code.clone());
            ins(&mut h, "cosy-machinetype", r.machine_type.clone());
        }
    }
    h
}

async fn get_json(client: &reqwest::Client, url: &str, headers: &HeaderMap) -> AppResult<(u16, Value)> {
    let resp = client
        .get(url)
        .headers(headers.clone())
        .timeout(Duration::from_secs(TIMEOUT_SECS))
        .send()
        .await
        .map_err(|e| AppError::Network(format!("请求失败: {e}")))?;
    let status = resp.status().as_u16();
    let data: Value = resp.json().await.unwrap_or(Value::Null);
    Ok((status, data))
}

/// 活动列表响应(uid 供风控身份与账号回写)。
#[derive(Debug, Clone, Default)]
pub struct CampaignsResp {
    pub uid: Option<String>,
    pub campaigns: Vec<Value>,
}

/// 拉取活动列表。401/403 → 鉴权失败错误(调用方按需提示 token 过期)。
pub async fn list_campaigns(
    client: &reqwest::Client,
    base: &str,
    headers: &HeaderMap,
) -> AppResult<CampaignsResp> {
    let (status, payload) = get_json(client, &format!("{}{}", base.trim_end_matches('/'), CAMPAIGNS_PATH), headers).await?;
    if status == 401 || status == 403 {
        return Err(AppError::Api(format!("鉴权失败 (HTTP {status})，token 可能已过期")));
    }
    if status >= 400 || payload.is_null() {
        return Err(AppError::Api(format!("活动列表 HTTP {status}")));
    }
    Ok(CampaignsResp {
        uid: payload.get("uid").and_then(|v| v.as_str()).map(str::to_owned),
        campaigns: payload
            .get("campaigns")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default(),
    })
}

/// 积分类活动:actionType == CLAIM_BENEFIT(对应 creditCampaign)。
pub fn is_credit_campaign(c: &Value) -> bool {
    c.get("actionType").and_then(|v| v.as_str()) == Some("CLAIM_BENEFIT")
}

/// 当前可领取:积分类 + claimStatus == CLAIMABLE。
pub fn is_claimable(c: &Value) -> bool {
    is_credit_campaign(c) && c.get("claimStatus").and_then(|v| v.as_str()) == Some("CLAIMABLE")
}

/// 过滤出可领取活动(保留引用,claim 时取 campaignId/benefit.amount)。
pub fn filter_claimable(campaigns: &[Value]) -> Vec<&Value> {
    campaigns.iter().filter(|c| is_claimable(c)).collect()
}

/// 领取单个活动,返回到账 Credits 数额(benefit.amount,响应优先、回退列表值)。
pub async fn claim_campaign(
    client: &reqwest::Client,
    base: &str,
    headers: &HeaderMap,
    campaign_id: &str,
    fallback_amount: i64,
) -> AppResult<i64> {
    let url = format!("{}{}", base.trim_end_matches('/'), campaign_claim_path(campaign_id));
    let resp = client
        .post(&url)
        .headers(headers.clone())
        .timeout(Duration::from_secs(TIMEOUT_SECS))
        .send()
        .await
        .map_err(|e| AppError::Network(format!("请求失败: {e}")))?;
    let status = resp.status().as_u16();
    if !resp.status().is_success() {
        let text = resp.text().await.unwrap_or_default();
        let head: String = text.chars().take(120).collect();
        return Err(AppError::Api(format!("HTTP {status} {head}")));
    }
    let rj: Value = resp.json().await.unwrap_or(Value::Null);
    let amount = rj
        .pointer("/benefit/amount")
        .and_then(|v| v.as_i64())
        .or_else(|| rj.pointer("/benefit/amount").and_then(|v| v.as_f64().map(|f| f as i64)))
        .unwrap_or(fallback_amount);
    Ok(amount)
}

/// 查询配额/积分用量(原始 JSON 透传,由前端/调用方解释)。401/403 → 鉴权失败。
pub async fn fetch_quota(client: &reqwest::Client, base: &str, token: &str) -> AppResult<Value> {
    let headers = build_headers(token, None, "");
    let (status, data) = get_json(client, &format!("{}{}", base.trim_end_matches('/'), QUOTA_USAGE_PATH), &headers).await?;
    if status == 401 || status == 403 {
        return Err(AppError::Api(format!("鉴权失败 (HTTP {status})，token 可能已过期")));
    }
    if status >= 400 || data.is_null() {
        return Err(AppError::Api(format!("quota HTTP {status}")));
    }
    Ok(data)
}

/// 拉取账号信息(昵称/邮箱/uid),用于给账号起显示名。返回原始 JSON。
pub async fn fetch_userinfo(client: &reqwest::Client, base: &str, token: &str) -> AppResult<Value> {
    let headers = build_headers(token, None, "");
    let (status, data) = get_json(client, &format!("{}{}", base.trim_end_matches('/'), USERINFO_PATH), &headers).await?;
    if status == 401 || status == 403 {
        return Err(AppError::Api(format!("鉴权失败 (HTTP {status})，token 可能已过期")));
    }
    if status >= 400 || data.is_null() {
        return Err(AppError::Api(format!("userinfo HTTP {status}")));
    }
    Ok(data)
}

/// 单账号领取汇总(纯函数,便于单测)。
/// - 无可领:有积分类活动 → already("今日已领");否则 no-activity(带提示)。
/// - 有可领:任一成功 → checked-in;否则 failed。
pub fn summarize_claim_round(
    has_credit_campaign: bool,
    claimable_count: usize,
    claimed_amount: i64,
    errors: &[String],
    no_activity_hint: &str,
) -> (ClaimOutcome, String) {
    if claimable_count == 0 {
        if !has_credit_campaign {
            return (ClaimOutcome::NoActivity, no_activity_hint.to_string());
        }
        return (ClaimOutcome::Already, "今日已领".into());
    }
    if claimed_amount > 0 {
        return (ClaimOutcome::CheckedIn, format!("领取成功 +{claimed_amount} Credits"));
    }
    (
        ClaimOutcome::Failed,
        format!("领取失败：{}", if errors.is_empty() { "未知原因".to_string() } else { errors.join("；") }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn api_base_by_region() {
        assert_eq!(api_base("cn"), CN_OPENAPI_BASE);
        assert_eq!(api_base("intl"), OPENAPI_BASE);
        assert_eq!(api_base(""), OPENAPI_BASE);
    }

    #[test]
    fn build_headers_basic_and_full() {
        // 基础头(constants.js buildQoderHeaders)
        let h = build_headers("tok", None, "cn");
        assert_eq!(h.get("authorization").unwrap(), "Bearer tok");
        assert_eq!(h.get("cosy-clienttype").unwrap(), "10");
        assert_eq!(h.get("cosy-version").unwrap(), "0.3.3");
        assert_eq!(h.get("cosy-machineos").unwrap(), "windows");
        assert_eq!(h.get("user-agent").unwrap(), "Qoder");
        assert_eq!(h.get("accept").unwrap(), "application/json");
        assert!(h.get("cosy-machineid").is_none());

        // 完整头(客户端身份 + 风控身份)
        let id = DeviceIdentity {
            machine_id: "00000000-1111-4222-8333-444444444444".into(),
            machine_os: "x86_64_win32".into(),
            machine_hostname: Some("MY-PC".into()),
            client_version: "0.2.5".into(),
            risk: Some(RiskIdentity {
                machine_token: "t".into(),
                machine_code: "c".into(),
                machine_type: "y".into(),
            }),
        };
        let h = build_headers("tok", Some(&id), "intl");
        assert_eq!(h.get("cosy-version").unwrap(), "0.2.5");
        assert_eq!(h.get("cosy-machineos").unwrap(), "x86_64_win32");
        assert_eq!(h.get("cosy-machineid").unwrap(), "00000000-1111-4222-8333-444444444444");
        assert_eq!(h.get("cosy-machinehostname").unwrap(), "MY-PC");
        assert_eq!(h.get("cosy-machinetoken").unwrap(), "t");
        assert_eq!(h.get("cosy-machinecode").unwrap(), "c");
        assert_eq!(h.get("cosy-machinetype").unwrap(), "y");
    }

    #[test]
    fn campaign_filter_claimable_only() {
        let campaigns = vec![
            json!({"campaignId": "c1", "actionType": "CLAIM_BENEFIT", "claimStatus": "CLAIMABLE", "benefit": {"amount": 100}}),
            json!({"campaignId": "c2", "actionType": "CLAIM_BENEFIT", "claimStatus": "CLAIMED"}),
            json!({"campaignId": "c3", "actionType": "SIGN_IN", "claimStatus": "CLAIMABLE"}),
            json!({"campaignId": "c4", "benefit": {"amount": 5}}),
            json!({"campaignId": "c5", "actionType": "CLAIM_BENEFIT", "claimStatus": "CLAIMABLE", "benefit": {"amount": 7}}),
        ];
        let claimable = filter_claimable(&campaigns);
        assert_eq!(claimable.len(), 2, "只保留 CLAIM_BENEFIT + CLAIMABLE");
        assert_eq!(claimable[0]["campaignId"], "c1");
        assert_eq!(claimable[1]["campaignId"], "c5");
        assert!(is_credit_campaign(&campaigns[1]));
        assert!(!is_credit_campaign(&campaigns[3]));
    }

    #[test]
    fn summarize_claim_round_state_machine() {
        // 无积分类活动 → no-activity(带提示)
        let (o, m) = summarize_claim_round(false, 0, 0, &[], "当前无可领取的活动");
        assert_eq!(o, ClaimOutcome::NoActivity);
        assert_eq!(m, "当前无可领取的活动");
        // 有积分类活动但都领过 → already
        let (o, m) = summarize_claim_round(true, 0, 0, &[], "提示");
        assert_eq!(o, ClaimOutcome::Already);
        assert_eq!(m, "今日已领");
        // 领取成功
        let (o, m) = summarize_claim_round(true, 2, 107, &[], "提示");
        assert_eq!(o, ClaimOutcome::CheckedIn);
        assert_eq!(m, "领取成功 +107 Credits");
        // 全部失败
        let (o, m) = summarize_claim_round(true, 2, 0, &["HTTP 500 boom".into()], "提示");
        assert_eq!(o, ClaimOutcome::Failed);
        assert_eq!(m, "领取失败：HTTP 500 boom");
        // 失败但无错误信息
        let (o, m) = summarize_claim_round(true, 1, 0, &[], "提示");
        assert_eq!(o, ClaimOutcome::Failed);
        assert_eq!(m, "领取失败：未知原因");
    }

    #[test]
    fn campaign_claim_path_encodes_id() {
        assert_eq!(
            campaign_claim_path("abc/123"),
            "/sash/api/v1/me/campaigns/abc%2F123/claim"
        );
        assert_eq!(
            campaign_claim_path("plain-id"),
            "/sash/api/v1/me/campaigns/plain-id/claim"
        );
    }
}
