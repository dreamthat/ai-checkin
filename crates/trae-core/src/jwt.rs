//! JWT 解析与刷新。移植自参考项目 jwt.rs + accounts.rs::refresh_jwt。
//! 解析不校验签名,仅本地展示/取 user_id/exp 用。

use base64::Engine;
use serde::Serialize;

/// JWT 解析结果
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct JwtInfo {
    pub user_id: Option<String>,
    pub exp_hours: Option<f64>,
    pub exp_timestamp: Option<i64>,
}

/// 解析 JWT(支持 `Cloud-IDE-JWT `/`Bearer `/裸 token 前缀;payload 取 data.id 与 exp)。
pub fn parse(jwt_full: &str) -> JwtInfo {
    let trimmed = jwt_full.trim();
    let token = trimmed
        .strip_prefix("Cloud-IDE-JWT ")
        .or_else(|| trimmed.strip_prefix("Bearer "))
        .unwrap_or(trimmed)
        .trim();
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() < 2 {
        return JwtInfo::default();
    }
    // JWT payload 使用 base64url 无填充编码
    let payload_b64 = parts[1].trim_end_matches('=');
    let Ok(bytes) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload_b64) else {
        return JwtInfo::default();
    };
    let Ok(payload) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return JwtInfo::default();
    };
    let user_id = payload
        .get("data")
        .and_then(|d| d.get("id"))
        .and_then(|v| {
            v.as_str()
                .map(|s| s.to_string())
                .or_else(|| v.as_i64().map(|n| n.to_string()))
        })
        .or_else(|| {
            payload
                .get("auth_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .or_else(|| {
            payload
                .get("sub")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        });

    let exp_timestamp = payload.get("exp").and_then(|v| {
        v.as_i64()
            .or_else(|| v.as_f64().map(|f| f as i64))
            .or_else(|| v.as_str().and_then(|s| s.parse::<i64>().ok()))
    });

    let exp_hours = exp_timestamp.map(|exp| {
        let now = chrono::Utc::now().timestamp();
        (exp - now) as f64 / 3600.0
    });

    JwtInfo {
        user_id,
        exp_hours,
        exp_timestamp,
    }
}

/// 由 exp 剩余小时数推导状态:>24 ok / <=24 && >0 warn / <=0 expired / None unknown
pub fn status_of(exp_hours: Option<f64>) -> &'static str {
    match exp_hours {
        Some(h) if h > 24.0 => "ok",
        Some(h) if h > 0.0 => "warn",
        Some(_) => "expired",
        None => "unknown",
    }
}

/// 规范化完整 JWT:缺失 `Cloud-IDE-JWT ` 前缀时补上。
pub fn normalize_full(jwt: &str) -> String {
    let t = jwt.trim();
    if t.starts_with("Cloud-IDE-JWT ") || t.starts_with("Bearer ") {
        t.to_string()
    } else {
        format!("Cloud-IDE-JWT {t}")
    }
}

/// 用 refresh_token 调 ExchangeToken 换取新 JWT(可能轮换 refresh_token)。
/// 返回 (新完整 JWT, 新 refresh_token 可选)。user_id 一致性由调用方校验。
pub async fn exchange_token(
    refresh_token: &str,
    client: &reqwest::Client,
) -> Result<(String, Option<String>), String> {
    let resp = client
        .post("https://api.trae.com.cn/cloudide/api/v3/trae/oauth/ExchangeToken")
        .header("content-type", "application/json")
        .header("accept", "*/*")
        .json(&serde_json::json!({
            "ClientID": "en1oxy7wnw8j9n",
            "RefreshToken": refresh_token,
            "ClientSecret": "-",
            "UserID": ""
        }))
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| format!("ExchangeToken 请求失败: {e}"))?;

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("ExchangeToken 解析响应失败: {e}"))?;

    let code = body.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
    if code != 0 {
        let msg = body
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("未知错误");
        return Err(format!("ExchangeToken 失败 (code={code}): {msg}"));
    }
    let data = body
        .get("data")
        .ok_or_else(|| "ExchangeToken 响应缺少 data".to_string())?;
    let new_access = data
        .get("access_token")
        .or_else(|| data.get("token"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| "ExchangeToken 响应缺少 access_token".to_string())?;
    let new_jwt = normalize_full(new_access);
    let new_rt = data
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    Ok((new_jwt, new_rt))
}
