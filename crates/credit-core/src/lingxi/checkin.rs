// 灵犀签到编排:POST checkinUrl(固定 UA + Cookie,10s 超时)→ 响应文本三级判定
// → Success/Already 记当日 lastSuccessDate(Asia/Shanghai 幂等)→ 写账号状态与日志。
// skipped(当日已成功)为纯跳过:不改状态、不写日志,避免覆盖上一次成功结果。

use serde_json::json;

use crate::log::{LogEntry, LogStore, PLATFORM_LINGXI};
use crate::store::LingxiState;

use super::{classify_response, LingxiAccount, Outcome};

/// 签到请求固定 UA(灵犀服务端校验浏览器特征;与 Lingxi-Check-in 一致)。
pub const CHECKIN_UA: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/151.0.0.0 Safari/537.36";
/// 请求超时(秒)。
pub const TIMEOUT_SECS: u64 = 10;

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// 单账号签到结果。
#[derive(Debug, Clone)]
pub struct LingxiCheckinOutcome {
    pub outcome: Outcome,
    pub message: String,
}

/// 失败/未知时给用户看的响应摘要(响应非结构化,截断防超长)。
fn snippet(text: &str) -> String {
    let trimmed = text.trim();
    let s: String = trimmed.chars().take(120).collect();
    if trimmed.chars().count() > 120 {
        format!("{s}…")
    } else {
        s
    }
}

/// 结果写回:账号 lastCheckinAt/lastResult/lastMessage + 成功口径写 lastSuccessDate + 日志。
fn persist_result(state: &LingxiState, account: &LingxiAccount, o: &LingxiCheckinOutcome, today: &str) {
    let mut data = state.data.lock().unwrap();
    let mut updates = json!({
        "lastCheckinAt": now_ms(),
        "lastResult": o.outcome.as_str(),
        "lastMessage": o.message,
    });
    if o.outcome.is_success() {
        updates["lastSuccessDate"] = json!(today);
    }
    data.update_account(&account.id, updates);
    data.append_log(LogEntry::new(
        &account.id,
        &account.name,
        PLATFORM_LINGXI,
        o.outcome.as_str(),
        &o.message,
    ));
    drop(data);
    let _ = state.save();
}

/// 单账号签到:引擎 + 状态回写。`today` 为 Asia/Shanghai 本地日期(YYYY-MM-DD)。
/// 当日已成功(lastSuccessDate == today)→ Skipped 跳过,不发起请求、不留痕。
pub async fn checkin_one(
    state: &LingxiState,
    client: &reqwest::Client,
    account: &LingxiAccount,
    today: &str,
) -> LingxiCheckinOutcome {
    if account.last_success_date.as_deref() == Some(today) {
        return LingxiCheckinOutcome {
            outcome: Outcome::Skipped,
            message: "今日已签到成功，跳过".into(),
        };
    }
    let outcome = checkin_request(client, account).await;
    persist_result(state, account, &outcome, today);
    outcome
}

/// 协议请求与判定(不写状态):POST checkinUrl,headers 仅 UA + Cookie。
async fn checkin_request(client: &reqwest::Client, account: &LingxiAccount) -> LingxiCheckinOutcome {
    let url = account.checkin_url.trim();
    let cookie = account.cookie.trim();
    if url.is_empty() {
        return LingxiCheckinOutcome { outcome: Outcome::Failed, message: "签到链接为空，请编辑账号补全".into() };
    }
    if cookie.is_empty() {
        return LingxiCheckinOutcome { outcome: Outcome::Failed, message: "Cookie 为空，请编辑账号补全".into() };
    }
    let resp = client
        .post(url)
        .header("User-Agent", CHECKIN_UA)
        .header("Cookie", cookie)
        .timeout(std::time::Duration::from_secs(TIMEOUT_SECS))
        .send()
        .await;
    let text = match resp {
        Ok(r) => match r.text().await {
            Ok(t) => t,
            Err(e) => {
                return LingxiCheckinOutcome { outcome: Outcome::Failed, message: format!("读取响应失败：{e}") };
            }
        },
        Err(e) => {
            return LingxiCheckinOutcome { outcome: Outcome::Failed, message: format!("请求失败：{e}") };
        }
    };
    let outcome = classify_response(&text);
    let message = match outcome {
        Outcome::Success => "签到成功".into(),
        Outcome::Already => "今日已签到（已签过）".into(),
        Outcome::Failed => format!("签到失败：{}", snippet(&text)),
        Outcome::Unknown => format!("响应无法识别，稍后重试：{}", snippet(&text)),
        Outcome::Skipped => unreachable!("classify_response 不产生 skipped"),
    };
    LingxiCheckinOutcome { outcome, message }
}

/// 全部启用账号签到(调度到点执行一轮的入口;调用方传 Asia/Shanghai 今日日期)。
pub async fn checkin_all(
    state: &LingxiState,
    client: &reqwest::Client,
    today: &str,
) -> Vec<(String, LingxiCheckinOutcome)> {
    let accounts: Vec<LingxiAccount> = {
        let data = state.data.lock().unwrap();
        data.get_accounts().iter().filter(|a| a.enabled).cloned().collect()
    };
    let mut results = Vec::new();
    for account in &accounts {
        let o = checkin_one(state, client, account, today).await;
        results.push((account.id.clone(), o));
    }
    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippet_truncates_long_text() {
        let long = "a".repeat(200);
        let s = snippet(&long);
        assert!(s.chars().count() == 121 && s.ends_with('…'), "{s}");
        assert_eq!(snippet("  short  "), "short");
    }

    #[test]
    fn checkin_request_fails_fast_on_empty_fields() {
        let client = reqwest::Client::new();
        let empty_url = LingxiAccount {
            id: "1".into(),
            name: "n".into(),
            cookie: "c".into(),
            ..Default::default()
        };
        let empty_cookie = LingxiAccount {
            id: "2".into(),
            name: "n".into(),
            checkin_url: "https://example.com".into(),
            ..Default::default()
        };
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async {
            let o = checkin_request(&client, &empty_url).await;
            assert_eq!(o.outcome, Outcome::Failed);
            assert!(o.message.contains("签到链接为空"));
            let o = checkin_request(&client, &empty_cookie).await;
            assert_eq!(o.outcome, Outcome::Failed);
            assert!(o.message.contains("Cookie 为空"));
        });
    }
}
