// ZCode API 客户端 — 移植自 CreditDaddy zcodeClient.js(原 zcode-switch quota.rs / claim.rs,MIT)。
//
//   GET https://open.bigmodel.cn/api/monitor/usage/quota/limit   Coding Plan 窗口额度
//   GET https://open.bigmodel.cn/api/biz/subscription/list      当前订阅
//   GET https://zcode.z.ai/api/v1/zcode-plan/billing/balance    Start Plan / Z.ai 渠道余额
//   GET https://zcode.z.ai/api/v1/zcode-plan/billing/preview    可领取的活动套餐列表
//   POST https://zcode.z.ai/api/v1/zcode-plan/billing/claim     领取(body {plan_id},可带验证码头)
// 业务码:1001/1002/1004/1005/401 → 失败;1003 → 已领取;3007/3001 → 需要验证码。

use std::time::Duration;

use chrono::SecondsFormat;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde_json::{json, Value};

use crate::error::{AppError, AppResult};
use crate::models::ClaimOutcome;

pub const ZAI_BASE: &str = "https://zcode.z.ai";
pub const BIGMODEL_BASE: &str = "https://open.bigmodel.cn";
pub const BILLING_BALANCE_PATH: &str = "/api/v1/zcode-plan/billing/balance";
pub const BILLING_PREVIEW_PATH: &str = "/api/v1/zcode-plan/billing/preview";
pub const BILLING_CLAIM_PATH: &str = "/api/v1/zcode-plan/billing/claim";
pub const QUOTA_LIMIT_PATH: &str = "/api/monitor/usage/quota/limit";
pub const SUBSCRIPTION_LIST_PATH: &str = "/api/biz/subscription/list";
/// 客户端版本探测失败时的兜底(与 zcodeClient.js 一致)
pub const APP_VERSION_FALLBACK: &str = "3.11.2";
const GET_TIMEOUT: Duration = Duration::from_secs(15);
const CLAIM_TIMEOUT: Duration = Duration::from_secs(25);

// ── 客户端版本(billing 接口对版本有校验,过低直接 3001) ──

/// ZCode 客户端版本:优先 Windows 注册表探测已安装版本,否则兜底 3.11.2(进程内缓存)。
pub fn zcode_app_version() -> String {
    static CACHE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    CACHE
        .get_or_init(|| {
            #[cfg(windows)]
            {
                registry_version().unwrap_or_else(|| APP_VERSION_FALLBACK.to_string())
            }
            #[cfg(not(windows))]
            {
                APP_VERSION_FALLBACK.to_string()
            }
        })
        .clone()
}

#[cfg(windows)]
fn registry_version() -> Option<String> {
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    use winreg::RegKey;
    const UNINSTALL: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall";
    for (hive, sub) in [
        (HKEY_LOCAL_MACHINE, UNINSTALL),
        (HKEY_LOCAL_MACHINE, r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall"),
        (HKEY_CURRENT_USER, UNINSTALL),
    ] {
        let root = RegKey::predef(hive).open_subkey(sub).ok()?;
        for key in root.enum_keys().flatten() {
            let Ok(k) = root.open_subkey(&key) else { continue };
            let name: String = k.get_value("DisplayName").unwrap_or_default();
            if !name.starts_with("ZCode") {
                continue;
            }
            let ver: String = k.get_value("DisplayVersion").unwrap_or_default();
            if !ver.is_empty() {
                return Some(ver);
            }
        }
    }
    None
}

/// X-Platform:win32-x64 / darwin-arm64 / linux-x64 …
pub fn platform_tag() -> String {
    let platform = if cfg!(windows) {
        "win32"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else {
        "linux"
    };
    let arch = if cfg!(target_arch = "aarch64") { "arm64" } else { "x64" };
    format!("{platform}-{arch}")
}

pub fn os_category() -> &'static str {
    if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    }
}

fn client_timezone() -> String {
    std::env::var("TZ").unwrap_or_else(|_| "Asia/Shanghai".into())
}

fn ins(h: &mut HeaderMap, k: &'static str, v: String) {
    if let Ok(v) = HeaderValue::from_str(&v) {
        h.insert(HeaderName::from_static(k), v);
    }
}

/// zcode.z.ai 请求头(zaiHeaders)。
pub fn zai_headers(token: &str, device_mid: Option<&str>) -> HeaderMap {
    let ver = zcode_app_version();
    let mut h = HeaderMap::new();
    ins(&mut h, "user-agent", format!("ZCode/{ver}"));
    ins(&mut h, "http-referer", "https://zcode.z.ai".into());
    ins(&mut h, "x-title", "Z Code@electron".into());
    ins(&mut h, "x-zcode-app-version", ver);
    ins(&mut h, "x-platform", platform_tag());
    ins(&mut h, "x-release-channel", "stable".into());
    ins(&mut h, "x-client-language", "zh-CN".into());
    ins(&mut h, "x-client-timezone", client_timezone());
    ins(&mut h, "x-os-category", os_category().into());
    if let Some(mid) = device_mid.filter(|m| !m.is_empty()) {
        ins(&mut h, "x-device-mid", mid.to_string());
    }
    ins(&mut h, "authorization", format!("Bearer {token}"));
    ins(&mut h, "x-request-id", uuid::Uuid::new_v4().to_string());
    h
}

/// open.bigmodel.cn 请求头(bigmodelHeaders)。
pub fn bigmodel_headers(token: &str) -> HeaderMap {
    let mut h = HeaderMap::new();
    ins(&mut h, "authorization", format!("Bearer {token}"));
    ins(&mut h, "user-agent", format!("ZCode/{}", zcode_app_version()));
    ins(&mut h, "x-request-id", uuid::Uuid::new_v4().to_string());
    h
}

// ── 业务码 ──

/// 领取失败码 → 中文提示(取自 zcode-switch i18n.rs)。
pub fn claim_fail_message(code: i64) -> &'static str {
    match code {
        1001 => "套餐不存在",
        1002 => "活动已结束或套餐暂不可领取",
        1003 => "该套餐已经领取过",
        1004 => "不符合领取条件",
        1005 => "今日领取名额已用完",
        3001 => "领取参数错误，请刷新后重试",
        3007 => "验证码校验失败，请重试",
        401 => "请先登录后再领取",
        _ => "领取失败",
    }
}

/// 业务码 → ClaimOutcome(状态机):1003 → already;3007/3001 → need-captcha;其余 → failed。
/// code == 0 视为成功,由调用方先行处理,不走这里。
pub fn map_claim_code(code: i64) -> ClaimOutcome {
    match code {
        1003 => ClaimOutcome::Already,
        3007 | 3001 => ClaimOutcome::NeedCaptcha,
        _ => ClaimOutcome::Failed,
    }
}

/// 业务接口错误(带 code,供编排层做状态机分流)。
#[derive(Debug, Clone)]
pub struct ZcodeApiError {
    pub code: i64,
    pub message: String,
}

impl From<ZcodeApiError> for AppError {
    fn from(e: ZcodeApiError) -> Self {
        AppError::Api(e.message)
    }
}

fn business_ok(v: &Value) -> bool {
    let code = v.get("code");
    let code_ok = code.and_then(|c| c.as_i64()).map(|c| c == 200 || c == 0).unwrap_or_else(|| code.is_none());
    code_ok && v.get("success").and_then(|s| s.as_bool()).unwrap_or(true)
}

async fn request_json(
    client: &reqwest::Client,
    method: reqwest::Method,
    url: &str,
    headers: &HeaderMap,
    body: Option<Value>,
    timeout: Duration,
) -> Result<(u16, Value), ZcodeApiError> {
    let mut req = client.request(method, url).headers(headers.clone()).timeout(timeout);
    if let Some(b) = body {
        req = req.json(&b);
    }
    let resp = req.send().await.map_err(|e| ZcodeApiError { code: -1, message: format!("网络请求失败: {e}") })?;
    let status = resp.status().as_u16();
    let text = resp
        .text()
        .await
        .map_err(|e| ZcodeApiError { code: -1, message: format!("读取响应失败: {e}") })?;
    let v = serde_json::from_str(&text).unwrap_or_else(|_| json!({ "code": status, "msg": text.chars().take(120).collect::<String>() }));
    Ok((status, v))
}

async fn get_json(client: &reqwest::Client, url: &str, headers: &HeaderMap) -> Result<Value, ZcodeApiError> {
    request_json(client, reqwest::Method::GET, url, headers, None, GET_TIMEOUT).await.map(|(_, v)| v)
}

// ── 活动套餐(preview → normalizePlans) ──

/// 可领取的活动套餐。
#[derive(Debug, Clone, PartialEq)]
pub struct ClaimPlan {
    pub plan_id: String,
    pub name: String,
    pub description: String,
    pub priority: i64,
    /// 权益描述:"模型名 · 1.5万 Token（每日）"
    pub grants: Vec<String>,
}

const PLAN_PERIOD_LABEL: [(&str, &str); 3] = [("daily", "每日"), ("weekly", "每周"), ("monthly", "每月")];

fn fmt_grant_units(n: f64) -> String {
    if n >= 1e8 {
        format!("{:.1}亿", n / 1e8)
    } else if n >= 1e4 {
        format!("{:.1}万", n / 1e4)
    } else {
        format!("{}", n.round() as i64)
    }
}

/// billing/preview → 可领取活动列表(parse_plan 的移植;按 priority 降序,planId 升序)。
pub fn normalize_plans(preview: &Value) -> Vec<ClaimPlan> {
    let mut out: Vec<ClaimPlan> = Vec::new();
    let Some(plans) = preview.pointer("/data/plans").and_then(|v| v.as_array()) else {
        return out;
    };
    for p in plans {
        let plan_id = p
            .get("plan_id")
            .or_else(|| p.get("planId"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if plan_id.is_empty() {
            continue;
        }
        let mut grants: Vec<String> = Vec::new();
        for e in p.get("entitlements").and_then(|v| v.as_array()).into_iter().flatten() {
            if e.get("meter").and_then(|v| v.as_str()) != Some("model_usage")
                || e.get("unit_type").and_then(|v| v.as_str()) != Some("token")
            {
                continue;
            }
            let Some(name) = e.get("show_name").and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty()) else {
                continue;
            };
            let units = e.get("grant_units").or_else(|| e.get("grantUnits")).and_then(|v| v.as_f64()).unwrap_or(0.0);
            let period = e.get("period").and_then(|v| v.as_str()).unwrap_or("one_time");
            let period_label = PLAN_PERIOD_LABEL
                .iter()
                .find(|(k, _)| *k == period)
                .map(|(_, label)| *label)
                .unwrap_or("一次性");
            grants.push(format!("{name} · {} Token（{period_label}）", fmt_grant_units(units)));
        }
        out.push(ClaimPlan {
            plan_id,
            name: p.get("name").and_then(|v| v.as_str()).unwrap_or("").trim().to_string(),
            description: p.get("description").and_then(|v| v.as_str()).unwrap_or("").trim().to_string(),
            priority: p.get("priority").and_then(|v| v.as_i64()).unwrap_or(0),
            grants,
        });
    }
    out.sort_by(|a, b| b.priority.cmp(&a.priority).then_with(|| a.plan_id.cmp(&b.plan_id)));
    out
}

/// 可领取的活动列表。账户当前没有可领活动时返回 [](不是错误)。
pub async fn fetch_claim_plans(
    client: &reqwest::Client,
    token: &str,
    device_mid: Option<&str>,
) -> AppResult<Vec<ClaimPlan>> {
    let url = format!(
        "{ZAI_BASE}{BILLING_PREVIEW_PATH}?app_version={}&platform={}",
        zcode_app_version(),
        platform_tag()
    );
    let v = get_json(client, &url, &zai_headers(token, device_mid))
        .await
        .map_err(ZcodeApiError::into_app_error)?;
    if v.get("code").and_then(|c| c.as_i64()) != Some(0) {
        let msg = v
            .get("msg")
            .or_else(|| v.get("message"))
            .and_then(|m| m.as_str())
            .map(str::to_owned)
            .unwrap_or_else(|| format!("查询活动列表失败（code {}）", v.get("code").and_then(|c| c.as_i64()).unwrap_or(-1)));
        return Err(AppError::Api(msg));
    }
    Ok(normalize_plans(&v))
}

// ── 领取(claim) ──

/// 领取成功返回的套餐信息。
#[derive(Debug, Clone, PartialEq)]
pub struct ClaimedPlan {
    pub plan_name: String,
    pub starts_at: Option<String>,
    pub ends_at: Option<String>,
}

impl ZcodeApiError {
    fn into_app_error(self) -> AppError {
        self.into()
    }
}

fn iso_from_unix(v: f64) -> Option<String> {
    if !v.is_finite() || v <= 0.0 {
        return None;
    }
    let ms = if v >= 1e12 { v as i64 } else { (v * 1000.0) as i64 };
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|t| t.to_rfc3339_opts(SecondsFormat::Millis, true))
}

/// 领取活动套餐。captcha 为 None 时不带验证码头(服务端风控可能拒绝,错误信息原样返回)。
/// code != 0 → Err(ZcodeApiError)(调用方按 1003/3007/3001 分流)。
pub async fn claim_plan(
    client: &reqwest::Client,
    token: &str,
    device_mid: Option<&str>,
    plan_id: &str,
    captcha: Option<(&str, &str)>,
) -> Result<ClaimedPlan, ZcodeApiError> {
    let mut headers = zai_headers(token, device_mid);
    if let Some((param, region)) = captcha {
        let param = param.trim();
        let region = region.trim();
        if !param.is_empty() {
            ins(&mut headers, "x-aliyun-captcha-verify-param", param.to_string());
        }
        if !region.is_empty() {
            ins(&mut headers, "x-aliyun-captcha-verify-region", region.to_string());
        }
    }
    let url = format!("{ZAI_BASE}{BILLING_CLAIM_PATH}");
    let (_, v) = request_json(
        client,
        reqwest::Method::POST,
        &url,
        &headers,
        Some(json!({ "plan_id": plan_id })),
        CLAIM_TIMEOUT,
    )
    .await?;
    let code = v.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
    if code != 0 {
        let server_msg = v
            .get("msg")
            .or_else(|| v.get("message"))
            .and_then(|m| m.as_str())
            .unwrap_or("");
        let base = claim_fail_message(code);
        return Err(ZcodeApiError {
            code,
            message: if server_msg.is_empty() { base.to_string() } else { format!("{base}（{server_msg}）") },
        });
    }
    let plan = v.pointer("/data/plan").cloned().unwrap_or(Value::Null);
    Ok(ClaimedPlan {
        plan_name: plan
            .get("name")
            .and_then(|n| n.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or(plan_id)
            .to_string(),
        starts_at: plan.get("starts_at").and_then(|t| t.as_f64()).and_then(iso_from_unix),
        ends_at: plan.get("ends_at").and_then(|t| t.as_f64()).and_then(iso_from_unix),
    })
}

// ── 额度查询 ──

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct QuotaPart {
    pub name: String,
    /// 非积分单位:次 / 分钟 / Token;空串表示纯数值
    pub unit: String,
    pub total: f64,
    pub used: f64,
    pub remaining: f64,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub expires_at: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct QuotaSnapshot {
    /// "bigmodel" | "zcode.z.ai" | ""
    pub source: String,
    pub total: f64,
    pub used: f64,
    pub remaining: f64,
    pub unit: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub plan: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub plan_expires_at: Option<String>,
    #[serde(default)]
    pub parts: Vec<QuotaPart>,
    pub empty: bool,
}

fn empty_quota() -> QuotaSnapshot {
    QuotaSnapshot { source: String::new(), empty: true, ..Default::default() }
}

/// BigModel quota/limit → 统一结构(主额度优先"使用时长",与 zcode-switch 一致)。
pub fn normalize_quota_limit(limit: &Value, sub: Option<&Value>) -> QuotaSnapshot {
    let mut parts: Vec<QuotaPart> = Vec::new();
    if let Some(limits) = limit.pointer("/data/limits").and_then(|v| v.as_array()) {
        for l in limits {
            let ltype = l.get("type").and_then(|v| v.as_str()).unwrap_or("");
            let (name, unit) = match ltype {
                "TOKENS_LIMIT" => ("提示次数", "次"),
                "TIME_LIMIT" => ("使用时长", "分钟"),
                other => (other, ""),
            };
            let total = l.get("usage").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let used = l.get("currentValue").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let remaining = l
                .get("remaining")
                .and_then(|v| v.as_f64())
                .unwrap_or((total - used).max(0.0));
            parts.push(QuotaPart {
                name: name.to_string(),
                unit: unit.to_string(),
                total,
                used,
                remaining,
                expires_at: l.get("nextResetTime").and_then(|v| v.as_f64()).and_then(iso_from_unix),
            });
        }
    }
    let sub = sub.filter(|s| business_ok(s));
    let sub_data = sub.and_then(|s| s.get("data")).and_then(|d| d.as_array());
    let sub_entry = sub_data.and_then(|arr| {
        arr.iter()
            .find(|s| s.get("status").and_then(|v| v.as_str()) == Some("VALID") && s.get("inCurrentPeriod").and_then(|v| v.as_bool()) != Some(false))
            .or_else(|| arr.first())
    });
    let main = parts
        .iter()
        .find(|p| p.unit == "分钟" && p.total > 0.0)
        .or_else(|| parts.iter().find(|p| p.total > 0.0))
        .cloned();
    QuotaSnapshot {
        source: "bigmodel".into(),
        total: main.as_ref().map(|m| m.total).unwrap_or(0.0),
        used: main.as_ref().map(|m| m.used).unwrap_or(0.0),
        remaining: main.as_ref().map(|m| m.remaining).unwrap_or(0.0),
        unit: main.as_ref().map(|m| m.unit.clone()).unwrap_or_default(),
        plan: sub_entry
            .and_then(|s| s.get("productName").and_then(|v| v.as_str()).map(str::to_owned))
            .or_else(|| limit.pointer("/data/level").and_then(|v| v.as_str()).map(str::to_owned)),
        plan_expires_at: sub_entry
            .and_then(|s| {
                s.get("endTime")
                    .or_else(|| s.get("expireTime"))
                    .and_then(|v| v.as_str().map(str::to_owned))
            }),
        parts,
        empty: false,
    }
}

/// zcode.z.ai billing/balance → 统一结构(balances 为空时从生效套餐权益派生,未来的权益标 pending)。
pub fn normalize_balance(balance: &Value) -> QuotaSnapshot {
    let d = balance.get("data").cloned().unwrap_or(Value::Null);
    let epoch = |v: &Value| -> Option<String> {
        let n = v.as_f64()?;
        let ms = if n < 1e12 { n * 1000.0 } else { n };
        chrono::DateTime::from_timestamp_millis(ms as i64).map(|t| t.to_rfc3339_opts(SecondsFormat::Millis, true))
    };
    let plans: Vec<Value> = d
        .get("plans")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter(|p| p.get("status").and_then(|v| v.as_str()).map(|s| s.eq_ignore_ascii_case("active")) == Some(true))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    let mut parts: Vec<QuotaPart> = d
        .get("balances")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .map(|b| {
                    let total = b.get("total_units").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    let used = b.get("used_units").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    let remaining = b
                        .get("remaining_units")
                        .or_else(|| b.get("available_units"))
                        .and_then(|v| v.as_f64())
                        .unwrap_or((total - used).max(0.0));
                    let unit_type = b.get("unit_type").and_then(|v| v.as_str()).unwrap_or("");
                    QuotaPart {
                        name: b
                            .get("show_name")
                            .or_else(|| b.get("name"))
                            .or_else(|| b.get("entitlement_id"))
                            .or_else(|| b.get("plan_id"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("额度")
                            .to_string(),
                        unit: if unit_type == "token" { "Token".into() } else { unit_type.to_string() },
                        total,
                        used,
                        remaining,
                        expires_at: b
                            .get("expire_at")
                            .or_else(|| b.get("period_end"))
                            .and_then(epoch),
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    // balances 可能为空(活动权益还没到 effective_at):从生效套餐的 token 权益派生
    if parts.is_empty() {
        let now_ms = chrono::Utc::now().timestamp_millis();
        for p in &plans {
            for e in p.get("entitlements").and_then(|v| v.as_array()).into_iter().flatten() {
                if e.get("unit_type").and_then(|v| v.as_str()) != Some("token") {
                    continue;
                }
                let grant = e.get("grant_units").and_then(|v| v.as_f64()).unwrap_or(0.0);
                if grant <= 0.0 {
                    continue;
                }
                let starts_at = e
                    .get("effective_at")
                    .or_else(|| p.get("starts_at"))
                    .and_then(|v| v.as_f64())
                    .map(|t| if t < 1e12 { t * 1000.0 } else { t });
                let pending = starts_at.map(|s| s as i64 > now_ms).unwrap_or(false);
                parts.push(QuotaPart {
                    name: e
                        .get("show_name")
                        .or_else(|| e.get("entitlement_id"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("活动额度")
                        .to_string(),
                    unit: "Token".into(),
                    total: grant,
                    used: 0.0,
                    remaining: if pending { 0.0 } else { grant },
                    expires_at: p.get("ends_at").and_then(epoch),
                });
            }
        }
    }
    let sum = |f: fn(&QuotaPart) -> f64| parts.iter().map(f).sum();
    QuotaSnapshot {
        source: "zcode.z.ai".into(),
        total: sum(|p| p.total),
        used: sum(|p| p.used),
        remaining: sum(|p| p.remaining),
        unit: parts.first().map(|p| p.unit.clone()).unwrap_or_else(|| "Token".into()),
        plan: plans
            .first()
            .and_then(|p| p.get("name").and_then(|v| v.as_str()).map(str::to_owned).or_else(|| p.get("plan_id").and_then(|v| v.as_str()).map(str::to_owned))),
        plan_expires_at: plans
            .iter()
            .filter_map(|p| p.get("ends_at").and_then(epoch))
            .min(),
        empty: parts.is_empty() && plans.is_empty(),
        parts,
    }
}

/// 额度查询:先试 BigModel Coding Plan(quota/limit),再试 Z.ai / Start Plan(billing/balance)。
/// 都没有有效套餐时返回 empty(不是错误:很多账号只用免费额度);全部鉴权失败时报错。
pub async fn fetch_quota(
    client: &reqwest::Client,
    tokens: &[String],
    billing: &[String],
    device_mid: Option<&str>,
) -> AppResult<QuotaSnapshot> {
    if tokens.is_empty() && billing.is_empty() {
        return Err(AppError::Credential("账号快照里没有可用 token，请在 ZCode 登录后重新导入".into()));
    }
    let mut last_err: Option<String> = None;
    let mut auth_fails = 0usize;
    for t in tokens {
        let (_, limit) = match request_json(client, reqwest::Method::GET, &format!("{BIGMODEL_BASE}{QUOTA_LIMIT_PATH}"), &bigmodel_headers(t), None, GET_TIMEOUT).await {
            Ok(v) => v,
            Err(e) => {
                last_err = Some(e.message);
                continue;
            }
        };
        if business_ok(&limit) {
            let sub = get_json(client, &format!("{BIGMODEL_BASE}{SUBSCRIPTION_LIST_PATH}"), &bigmodel_headers(t))
                .await
                .ok();
            return Ok(normalize_quota_limit(&limit, sub.as_ref()));
        }
        if limit.get("code").and_then(|c| c.as_i64()) == Some(401) {
            auth_fails += 1;
        } else {
            last_err = limit
                .get("msg")
                .and_then(|m| m.as_str())
                .map(str::to_owned)
                .or(last_err);
        }
    }
    for t in billing {
        let url = format!("{ZAI_BASE}{BILLING_BALANCE_PATH}?app_version={}", zcode_app_version());
        let (_, bal) = match request_json(client, reqwest::Method::GET, &url, &zai_headers(t, device_mid), None, GET_TIMEOUT).await {
            Ok(v) => v,
            Err(e) => {
                last_err = Some(e.message);
                continue;
            }
        };
        if business_ok(&bal) {
            return Ok(normalize_balance(&bal));
        }
        if bal.get("code").and_then(|c| c.as_i64()) == Some(401) {
            auth_fails += 1;
        }
    }
    if !tokens.is_empty() && auth_fails >= tokens.len() {
        return Err(AppError::Credential("鉴权失败，登录可能已过期，请在 ZCode 重新登录后重新导入".into()));
    }
    // "当前用户不存在coding plan"之类属于无套餐,按空额度返回
    let no_plan = last_err
        .as_deref()
        .map(|e| e.contains("不存在") || e.to_lowercase().contains("no") && e.to_lowercase().contains("plan"))
        .unwrap_or(true);
    if no_plan {
        return Ok(empty_quota());
    }
    Err(AppError::Api(format!("额度查询失败：{}", last_err.unwrap_or_else(|| "未知错误".into()))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn map_claim_code_state_machine() {
        assert_eq!(map_claim_code(1003), ClaimOutcome::Already);
        assert_eq!(map_claim_code(3007), ClaimOutcome::NeedCaptcha);
        assert_eq!(map_claim_code(3001), ClaimOutcome::NeedCaptcha);
        for code in [1001, 1002, 1004, 1005, 401, -1, 9999] {
            assert_eq!(map_claim_code(code), ClaimOutcome::Failed, "code {code} 应映射 failed");
        }
        // 中文提示映射
        assert_eq!(claim_fail_message(1003), "该套餐已经领取过");
        assert_eq!(claim_fail_message(3007), "验证码校验失败，请重试");
        assert_eq!(claim_fail_message(1005), "今日领取名额已用完");
        assert_eq!(claim_fail_message(12345), "领取失败");
    }

    #[test]
    fn normalize_plans_parses_and_sorts() {
        let preview = json!({
            "code": 0,
            "data": {
                "server_time": 1767000000,
                "plans": [
                    {"plan_id": "p-low", "name": "低优", "priority": 1, "description": "d",
                     "entitlements": [{"meter": "model_usage", "unit_type": "token", "show_name": "GLM-5", "grant_units": 15000, "period": "daily"}]},
                    {"planId": "p-high", "name": "高优", "priority": 9,
                     "entitlements": [
                        {"meter": "model_usage", "unit_type": "token", "show_name": "GLM-5.3", "grantUnits": 1.2e8, "period": "one_time"},
                        {"meter": "other", "unit_type": "token", "show_name": "忽略", "grant_units": 9}
                     ]},
                    {"name": "无 id 跳过"}
                ]
            }
        });
        let plans = normalize_plans(&preview);
        assert_eq!(plans.len(), 2);
        assert_eq!(plans[0].plan_id, "p-high", "priority 降序在前");
        assert_eq!(plans[0].plan_id, "p-high");
        assert_eq!(plans[0].grants, vec!["GLM-5.3 · 1.2亿 Token（一次性）"]);
        assert_eq!(plans[1].grants, vec!["GLM-5 · 1.5万 Token（每日）"]);
        assert_eq!(plans[1].description, "d");
        // 非 camelCase 的 grantUnits 兼容
        assert!(plans[0].grants[0].contains("1.2亿"));
        assert!(normalize_plans(&json!({})).is_empty());
    }

    #[test]
    fn normalize_quota_limit_picks_time_main() {
        let limit = json!({
            "code": 200,
            "data": {
                "level": "GLM Coding Plan",
                "limits": [
                    {"type": "TOKENS_LIMIT", "usage": 500, "currentValue": 100},
                    {"type": "TIME_LIMIT", "usage": 300, "currentValue": 40, "remaining": 260, "nextResetTime": 1767000000}
                ]
            }
        });
        let sub = json!({"code": 200, "data": [{"status": "VALID", "inCurrentPeriod": true, "productName": "Coding Pro", "endTime": "2026-12-31"}]});
        let q = normalize_quota_limit(&limit, Some(&sub));
        assert_eq!(q.source, "bigmodel");
        assert_eq!(q.unit, "分钟", "主额度优先使用时长");
        assert_eq!(q.total, 300.0);
        assert_eq!(q.remaining, 260.0);
        assert_eq!(q.plan.as_deref(), Some("Coding Pro"));
        assert_eq!(q.plan_expires_at.as_deref(), Some("2026-12-31"));
        assert_eq!(q.parts.len(), 2);
        assert!(q.parts[0].name == "提示次数" && q.parts[0].unit == "次");
        assert_eq!(q.parts[0].remaining, 400.0, "remaining 缺失时按 total-used");
    }

    #[test]
    fn normalize_balance_derives_entitlements_when_empty() {
        let balance = json!({
            "code": 0,
            "data": {
                "plans": [
                    {"plan_id": "p1", "name": "Start Plan", "status": "ACTIVE", "ends_at": 1767000000,
                     "entitlements": [{"unit_type": "token", "grant_units": 100000, "show_name": "赠送额度", "effective_at": 1000}]}
                ],
                "balances": []
            }
        });
        let q = normalize_balance(&balance);
        assert_eq!(q.source, "zcode.z.ai");
        assert_eq!(q.plan.as_deref(), Some("Start Plan"));
        assert_eq!(q.total, 100000.0);
        assert_eq!(q.remaining, 100000.0, "effective_at 已过 → 不 pending");
        assert_eq!(q.unit, "Token");
        // 全 future effective_at → pending,remaining=0
        let future = json!({
            "code": 0,
            "data": {"plans": [{"name": "x", "status": "active",
                "entitlements": [{"unit_type": "token", "grant_units": 5, "effective_at": 4102444800i64}]}]}
        });
        let q2 = normalize_balance(&future);
        assert_eq!(q2.remaining, 0.0);
        assert!(q2.total >= 5.0);
    }

    #[test]
    fn iso_from_unix_seconds_and_millis() {
        assert_eq!(iso_from_unix(1767000000.0).as_deref(), Some("2025-12-29T09:20:00.000Z"));
        assert_eq!(iso_from_unix(1767000000000.0).as_deref(), Some("2025-12-29T09:20:00.000Z"), "毫秒自动识别");
        assert!(iso_from_unix(0.0).is_none());
        assert!(iso_from_unix(-1.0).is_none());
    }

    #[test]
    fn headers_carry_version_and_platform() {
        let h = zai_headers("tok", Some("mid-1"));
        assert_eq!(h.get("authorization").unwrap(), "Bearer tok");
        let ua = h.get("user-agent").unwrap().to_str().unwrap();
        assert!(ua.starts_with("ZCode/"), "{ua}");
        assert_eq!(h.get("x-device-mid").unwrap(), "mid-1");
        assert_eq!(h.get("x-os-category").unwrap(), os_category());
        assert!(h.get("x-request-id").is_some());
        // 无 device_mid 不带头
        let h2 = zai_headers("tok", None);
        assert!(h2.get("x-device-mid").is_none());
        let bm = bigmodel_headers("tok");
        assert_eq!(bm.get("authorization").unwrap(), "Bearer tok");
        // platform_tag 形如 {win32|darwin|linux}-{x64|arm64}
        let tag = platform_tag();
        assert!(
            ["win32-x64", "win32-arm64", "darwin-x64", "darwin-arm64", "linux-x64", "linux-arm64"].contains(&tag.as_str()),
            "platform_tag 实际: {tag}"
        );
    }
}
