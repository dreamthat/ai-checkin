// ZCode 活动领取编排 — 移植自 CreditDaddy zcodeAutoClaim.js。
//
// 策略(对齐用户手动路径,逐级尝试):
//   1. billing/preview 查可领活动;为空/查询失败 → 无事可做(返回 None,不留痕迹)
//   2. 直接 claim(不带验证码:服务端未风控时可直接成功)
//   3. 3007/3001(需验证码)→ 本 crate 不接打码平台,诚实降级为「需手动领取」(need-captcha)
//   4. 1003(已领取过)按成功口径处理
// 结果写入账号 lastResult + 日志;签到日历记忆用 status checked-in/already,第二天照常重查 preview。

use serde_json::json;

use crate::error::AppResult;
use crate::log::{LogEntry, LogStore, PLATFORM_ZCODE};
use crate::models::{ClaimOutcome, ZCodeAccount};
use crate::store::ZcodeState;
use crate::zcode::client::{self, QuotaSnapshot};
use crate::zcode::credentials::{billing_tokens, candidate_tokens, claim_token};

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// 单个活动的领取明细。
#[derive(Debug, Clone)]
pub struct PlanClaim {
    pub plan: String,
    pub ok: bool,
    pub already: bool,
    pub need_manual: bool,
    /// "direct" | "captcha" | ""
    pub via: String,
    pub message: Option<String>,
}

/// 汇总(纯函数)→ (状态, 文案)。
/// - 任一直接成功 → checked-in("已领取:A、B")
/// - 全部已领过 → already("活动均已领取过")
/// - 有活动需验证码 → need-captcha("有 N 个活动需要验证码,请在面板手动领取")
/// - 其余 → failed(取第一条失败信息)
pub fn summarize_claims(claims: &[PlanClaim]) -> (ClaimOutcome, String) {
    let names: Vec<&str> = claims.iter().filter(|c| c.ok && !c.already).map(|c| c.plan.as_str()).collect();
    if !names.is_empty() {
        return (ClaimOutcome::CheckedIn, format!("已领取：{}", names.join("、")));
    }
    if !claims.is_empty() && claims.iter().all(|c| c.already) {
        return (ClaimOutcome::Already, "活动均已领取过".into());
    }
    let manual = claims.iter().filter(|c| c.need_manual).count();
    if manual > 0 {
        return (ClaimOutcome::NeedCaptcha, format!("有 {manual} 个活动需要验证码，请在面板手动领取"));
    }
    let first_err = claims.iter().find_map(|c| c.message.clone()).unwrap_or_else(|| "未知错误".into());
    (ClaimOutcome::Failed, format!("领取失败：{first_err}"))
}

/// 单账号领取结果。
#[derive(Debug, Clone)]
pub struct ZcodeClaimOutcome {
    pub outcome: ClaimOutcome,
    pub message: String,
    pub claims: Vec<PlanClaim>,
}

/// 领取结果写回:账号 lastClaimAt/lastResult/lastMessage + 日志。
fn persist_result(state: &ZcodeState, account: &ZCodeAccount, o: &ZcodeClaimOutcome) {
    let mut data = state.data.lock().unwrap();
    let mut updates = json!({
        "lastResult": o.outcome.as_str(),
        "lastMessage": o.message,
        "lastClaimedAmount": 0,
    });
    if o.outcome.is_success() {
        updates["lastClaimAt"] = json!(now_ms());
    }
    data.update_account(&account.id, updates);
    data.append_log(LogEntry::new(&account.id, &account.name, PLATFORM_ZCODE, o.outcome.as_str(), &o.message));
    drop(data);
    let _ = state.save();
}

/// 为单个 ZCode 账号执行一次自动领取。
/// None = preview 为空或查询失败(与 CreditDaddy 一致:不留痕迹,不写 lastResult)。
pub async fn claim_one(state: &ZcodeState, client: &reqwest::Client, account: &ZCodeAccount) -> Option<ZcodeClaimOutcome> {
    let token = match claim_token(account) {
        Ok(t) => t,
        Err(e) => {
            let o = ZcodeClaimOutcome { outcome: ClaimOutcome::Failed, message: e.to_string(), claims: vec![] };
            persist_result(state, account, &o);
            return Some(o);
        }
    };
    let device_mid = account.device_mid.as_deref();
    let plans = match client::fetch_claim_plans(client, &token, device_mid).await {
        Ok(p) => p,
        Err(_) => return None, // 活动查询失败:记为无事可做(协议口径)
    };
    if plans.is_empty() {
        return None;
    }

    let mut claims: Vec<PlanClaim> = Vec::new();
    for plan in plans {
        let plan_name = if plan.name.is_empty() { plan.plan_id.clone() } else { plan.name.clone() };
        // 第 2 步:先试不带验证码
        match client::claim_plan(client, &token, device_mid, &plan.plan_id, None).await {
            Ok(r) => {
                claims.push(PlanClaim { plan: r.plan_name, ok: true, already: false, need_manual: false, via: "direct".into(), message: None });
                continue;
            }
            Err(e) => {
                if e.code == 1003 {
                    claims.push(PlanClaim { plan: plan_name, ok: true, already: true, need_manual: false, via: String::new(), message: None });
                    continue;
                }
                if e.code != 3007 && e.code != 3001 {
                    claims.push(PlanClaim { plan: plan_name, ok: false, already: false, need_manual: false, via: String::new(), message: Some(e.message) });
                    continue;
                }
                // 第 3 步:需要验证码 → 无验证码提供者,诚实降级「需手动领取」
                claims.push(PlanClaim {
                    plan: plan_name,
                    ok: false,
                    already: false,
                    need_manual: true,
                    via: String::new(),
                    message: Some(e.message),
                });
            }
        }
    }

    let (outcome, message) = summarize_claims(&claims);
    let o = ZcodeClaimOutcome { outcome, message, claims };
    persist_result(state, account, &o);
    Some(o)
}

/// 全部启用账号领取。
pub async fn claim_all(state: &ZcodeState, client: &reqwest::Client) -> Vec<(String, ZcodeClaimOutcome)> {
    let accounts: Vec<ZCodeAccount> = {
        let data = state.data.lock().unwrap();
        data.get_accounts().iter().filter(|a| a.enabled).cloned().collect()
    };
    let mut results = Vec::new();
    for account in &accounts {
        if let Some(o) = claim_one(state, client, account).await {
            results.push((account.id.clone(), o));
        }
    }
    results
}

/// 刷新账号额度(先 BigModel Coding Plan,再 Z.ai / Start Plan),缓存进账号 quota 字段。
pub async fn refresh_quota(state: &ZcodeState, client: &reqwest::Client, account: &ZCodeAccount) -> AppResult<QuotaSnapshot> {
    let home = dirs::home_dir().unwrap_or_default().to_string_lossy().to_string();
    let secret = crate::zcode::credentials::default_secret(&home);
    let tokens = candidate_tokens(account, &secret);
    let billing = billing_tokens(account, &secret);
    let snapshot = client::fetch_quota(client, &tokens, &billing, account.device_mid.as_deref()).await?;
    {
        let mut data = state.data.lock().unwrap();
        data.update_account(
            &account.id,
            json!({ "quota": serde_json::to_value(&snapshot)?, "quotaUpdatedAt": now_ms() }),
        );
    }
    state.save()?;
    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claim(plan: &str, ok: bool, already: bool, need_manual: bool, message: Option<&str>) -> PlanClaim {
        PlanClaim {
            plan: plan.into(),
            ok,
            already,
            need_manual,
            via: if ok && !already { "direct".into() } else { String::new() },
            message: message.map(str::to_owned),
        }
    }

    #[test]
    fn summarize_claims_state_machine() {
        // 任一成功 → checked-in
        let (o, m) = summarize_claims(&[
            claim("A", true, false, false, None),
            claim("B", false, false, true, Some("验证码校验失败")),
        ]);
        assert_eq!(o, ClaimOutcome::CheckedIn);
        assert_eq!(m, "已领取：A");
        // 全部已领过 → already
        let (o, m) = summarize_claims(&[claim("A", true, true, false, None), claim("B", true, true, false, None)]);
        assert_eq!(o, ClaimOutcome::Already);
        assert_eq!(m, "活动均已领取过");
        // 有需验证码 → need-captcha
        let (o, m) = summarize_claims(&[
            claim("A", false, false, true, Some("验证码校验失败，请重试（x）")),
            claim("B", false, false, true, Some("领取参数错误")),
        ]);
        assert_eq!(o, ClaimOutcome::NeedCaptcha);
        assert_eq!(m, "有 2 个活动需要验证码，请在面板手动领取");
        // 其余失败
        let (o, m) = summarize_claims(&[claim("A", false, false, false, Some("今日领取名额已用完"))]);
        assert_eq!(o, ClaimOutcome::Failed);
        assert_eq!(m, "领取失败：今日领取名额已用完");
        // 空列表(调用方已提前拦截,防御性)
        let (o, _) = summarize_claims(&[]);
        assert_eq!(o, ClaimOutcome::Failed);
    }

    #[test]
    fn plan_claim_fields_flow_from_client_errors() {
        // client.rs 的 map_claim_code 决定分流;这里验证 need_manual 语义与 code 对应
        for code in [3007i64, 3001] {
            let o = crate::zcode::client::map_claim_code(code);
            assert_eq!(o, ClaimOutcome::NeedCaptcha, "code {code}");
        }
        assert_eq!(crate::zcode::client::map_claim_code(1003), ClaimOutcome::Already);
    }
}
