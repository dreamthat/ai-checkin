// Qoder 领取编排 — 移植自 CreditDaddy qoderClient.checkinOne + checkin.js 状态记忆。
// 流程:token → 基础请求拿 uid → 客户端身份 + 设备风控身份 → 重拉活动列表
//   → 过滤 CLAIMABLE + CLAIM_BENEFIT → 逐个 claim → 汇总 Outcome → 写日志/状态。

use serde_json::json;

use crate::log::{LogEntry, LogStore, PLATFORM_QODER};
use crate::models::{ClaimOutcome, QoderAccount};
use crate::schedule;
use crate::store::QoderState;

use super::client::{self, DeviceIdentity};
use super::identity;

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// 单账号领取结果。
#[derive(Debug, Clone)]
pub struct QoderCheckinOutcome {
    pub outcome: ClaimOutcome,
    pub message: String,
    /// 本次领取到的 Credits 总额
    pub claimed_amount: i64,
    /// Qoder 用户 ID(供调用方回写账号)
    pub uid: Option<String>,
    /// 本次是否用上了设备风控身份(国际版每日活动的关键)
    pub risk: bool,
}

impl QoderCheckinOutcome {
    fn fail(message: &str) -> Self {
        Self { outcome: ClaimOutcome::Failed, message: message.into(), claimed_amount: 0, uid: None, risk: false }
    }
}

/// 无可领活动时的提示(纯函数):国内版固定文案;国际版区分"已限领"与"缺风控身份"。
pub fn no_activity_hint(region: &str, risk: bool, risk_error: Option<&str>) -> String {
    if region != "intl" {
        return "当前无可领取的活动".into();
    }
    if risk {
        return "当前无可领取的活动（国际版每台设备每天限领一次，可能已被本机其他账号领取）".into();
    }
    format!(
        "当前无可领取的活动（{}；国际版需要本机安装 Qoder 客户端）",
        risk_error.unwrap_or(&identity::risk_unavailable_reason())
    )
}

/// 核心领取流程(不写状态,由 checkin_one/checkin_all 决定持久化时机)。
async fn checkin_core(state: &QoderState, client: &reqwest::Client, account: &QoderAccount) -> QoderCheckinOutcome {
    let region = if account.region == "cn" { "cn" } else { "intl" };
    let base = client::api_base(region);
    let token = account.token.trim().to_string();
    if token.is_empty() {
        return QoderCheckinOutcome::fail("token 为空");
    }

    // 1) 基础请求拿 uid(风控身份按 uid 生成);uid 已知时跳过
    let mut uid = account.user_id.clone().filter(|s| !s.is_empty());
    let mut first: Option<client::CampaignsResp> = None;
    if uid.is_none() {
        match client::list_campaigns(client, base, &client::build_headers(&token, None, region)).await {
            Ok(resp) => {
                uid = resp.uid.clone();
                first = Some(resp);
            }
            Err(e) => return QoderCheckinOutcome::fail(&e.to_string()),
        }
    }

    // 2) 客户端身份 + 设备风控身份(国际版只有带风控身份的请求才会下发每日积分活动)
    let (risk, risk_error) = match identity::get_risk_identity_detailed(region, uid.as_deref()).await {
        Ok(Some(r)) => (Some(r), None),
        Ok(None) => (None, Some(identity::risk_unavailable_reason())),
        Err(e) => (None, Some(e)),
    };
    let identity = DeviceIdentity {
        machine_id: identity::machine_id_for(&state.base_dir, region),
        machine_os: identity::machine_os(),
        machine_hostname: identity::machine_hostname(),
        client_version: identity::client_version(region),
        risk: risk.clone(),
    };
    let headers = client::build_headers(&token, Some(&identity), region);

    // 3) 带身份重新拉取(有风控身份或尚未拉过时)
    let resp = if risk.is_some() || first.is_none() {
        match client::list_campaigns(client, base, &headers).await {
            Ok(r) => r,
            Err(e) => return QoderCheckinOutcome::fail(&e.to_string()),
        }
    } else {
        first.unwrap()
    };
    if resp.uid.is_some() {
        uid = resp.uid.clone();
    }

    // 4) 过滤 CLAIMABLE + CLAIM_BENEFIT → 逐个领取
    let claimable = client::filter_claimable(&resp.campaigns);
    let has_credit = resp.campaigns.iter().any(|c| client::is_credit_campaign(c));
    let claimable_count = claimable.len();
    let hint = no_activity_hint(region, risk.is_some(), risk_error.as_deref());
    let mut total = 0i64;
    let mut errors: Vec<String> = Vec::new();
    for c in &claimable {
        let id = c.get("campaignId").and_then(|v| v.as_str()).unwrap_or_default().to_string();
        let fallback = c.pointer("/benefit/amount").and_then(|v| v.as_i64()).unwrap_or(0);
        match client::claim_campaign(client, base, &headers, &id, fallback).await {
            Ok(amount) => total += amount,
            Err(e) => errors.push(e.to_string()),
        }
    }
    let (outcome, message) = client::summarize_claim_round(has_credit, claimable_count, total, &errors, &hint);
    QoderCheckinOutcome { outcome, message, claimed_amount: total, uid, risk: risk.is_some() }
}

/// 领取结果写回:账号 lastClaimAt/lastResult/lastMessage/uid + 日志。
fn persist_result(state: &QoderState, account: &QoderAccount, o: &QoderCheckinOutcome) {
    let mut data = state.data.lock().unwrap();
    let mut updates = json!({
        "lastClaimAt": now_ms(),
        "lastResult": o.outcome.as_str(),
        "lastMessage": o.message,
        "lastClaimedAmount": o.claimed_amount,
    });
    if let Some(uid) = &o.uid {
        updates["userId"] = json!(uid);
    }
    data.update_account(&account.id, updates);
    data.append_log(LogEntry::new(&account.id, &account.name, PLATFORM_QODER, o.outcome.as_str(), &o.message));
    drop(data);
    let _ = state.save();
}

/// 单账号签到(领取所有可领活动):引擎 + 状态回写。
pub async fn checkin_one(state: &QoderState, client: &reqwest::Client, account: &QoderAccount) -> QoderCheckinOutcome {
    let outcome = checkin_core(state, client, account).await;
    persist_result(state, account, &outcome);
    outcome
}

/// 本机今日国际版领取记录(day, accountId, name)——内存态,跨重启丢一个提醒周期,可接受。
struct DeviceClaim {
    day: String,
    account_id: String,
    name: String,
}

/// 全部启用账号签到。含"每台设备每天限领一次"记忆(国际版):本机已有账号领取成功后,
/// 其余国际版账号的 no-activity 归并为 already 并提示限领来源(对应 CreditDaddy deviceClaim)。
pub async fn checkin_all(state: &QoderState, client: &reqwest::Client) -> Vec<(String, QoderCheckinOutcome)> {
    let accounts: Vec<QoderAccount> = {
        let data = state.data.lock().unwrap();
        data.get_accounts().iter().filter(|a| a.enabled).cloned().collect()
    };
    let today = schedule::round_key(now_ms());
    let mut device_claim: Option<DeviceClaim> = None;
    let mut results = Vec::new();
    for account in &accounts {
        let mut o = checkin_core(state, client, account).await;
        if account.region == "intl" && o.risk {
            if o.outcome == ClaimOutcome::CheckedIn {
                device_claim = Some(DeviceClaim { day: today.clone(), account_id: account.id.clone(), name: account.name.clone() });
            } else if o.outcome == ClaimOutcome::NoActivity {
                if let Some(dc) = &device_claim {
                    if dc.day == today && dc.account_id != account.id {
                        o.outcome = ClaimOutcome::Already;
                        o.message = format!("本机今日国际版额度已由「{}」领取（Qoder 每台设备每天限领一次）", dc.name);
                    }
                }
            }
        }
        persist_result(state, account, &o);
        results.push((account.id.clone(), o));
    }
    results
}

/// 刷新账号额度(/api/v2/quota/usage),缓存进账号 quota 字段。返回原始 JSON。
pub async fn refresh_quota(state: &QoderState, client: &reqwest::Client, account: &QoderAccount) -> crate::error::AppResult<serde_json::Value> {
    let region = if account.region == "cn" { "cn" } else { "intl" };
    let data = client::fetch_quota(client, client::api_base(region), account.token.trim()).await?;
    {
        let mut d = state.data.lock().unwrap();
        d.update_account(&account.id, json!({ "quota": data, "quotaUpdatedAt": now_ms() }));
    }
    state.save()?;
    Ok(data)
}

/// 该账号在本"签到日"(UTC+8 10:00 界)是否已完成过(checked-in/already)。
/// 用于调度层的 qoderDailyDone 幂等:no-activity / failed 不算完成,下一轮还会重试。
pub fn done_this_round(account: &QoderAccount, now_ms: i64) -> bool {
    let Some(result) = account.last_result.as_deref().and_then(ClaimOutcome::parse) else {
        return false;
    };
    result.is_success()
        && account.last_claim_at.map(|at| schedule::round_key(at) == schedule::round_key(now_ms)).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_activity_hint_variants() {
        assert_eq!(no_activity_hint("cn", false, None), "当前无可领取的活动");
        assert_eq!(
            no_activity_hint("intl", true, None),
            "当前无可领取的活动（国际版每台设备每天限领一次，可能已被本机其他账号领取）"
        );
        let missing = no_activity_hint("intl", false, None);
        assert!(missing.contains("本机未安装 Qoder 客户端") || missing.contains("设备身份组件"), "{missing}");
        assert!(missing.contains("国际版需要本机安装 Qoder 客户端"));
        let custom = no_activity_hint("intl", false, Some("runtime-info 超时"));
        assert!(custom.contains("runtime-info 超时"));
    }

    #[test]
    fn done_this_round_respects_utc8_10am_boundary() {
        // 构造"今天 11:00 UTC+8"的时间戳:取任意一天 03:00 UTC
        let today_11am_cn = chrono::DateTime::parse_from_rfc3339("2026-03-01T03:00:00Z")
            .unwrap()
            .timestamp_millis();
        let yesterday_11am_cn = chrono::DateTime::parse_from_rfc3339("2026-02-28T03:00:00Z")
            .unwrap()
            .timestamp_millis();
        let done = QoderAccount {
            last_result: Some("checked-in".into()),
            last_claim_at: Some(today_11am_cn),
            ..Default::default()
        };
        assert!(done_this_round(&done, today_11am_cn));
        assert!(done_this_round(&done, today_11am_cn + 3_600_000), "同一天内(UTC+8 10:00 界)仍算完成");
        assert!(!done_this_round(&done, yesterday_11am_cn), "昨天的完成不算今天");
        // no-activity 不算完成(10:00 刷新后还会出现可领活动,不能记忆)
        let no_act = QoderAccount { last_result: Some("no-activity".into()), last_claim_at: Some(today_11am_cn), ..Default::default() };
        assert!(!done_this_round(&no_act, today_11am_cn));
        // failed 不算
        let failed = QoderAccount { last_result: Some("failed".into()), last_claim_at: Some(today_11am_cn), ..Default::default() };
        assert!(!done_this_round(&failed, today_11am_cn));
        // 无结果不算
        assert!(!done_this_round(&QoderAccount::default(), today_11am_cn));
    }
}
