// ZCode 本机凭据 — 移植自 CreditDaddy zcodeLocal.js + zcrypto.js(原 zcode-switch zcrypto.rs,MIT)。
//
// 凭据文件 ~/.zcode/v2/credentials.json 里每个值可能是:
//   - 明文字符串
//   - "enc:v1:<nonce>.<tag>.<密文>"(URL-safe base64 无 padding),AES-256-GCM,
//     key = SHA256(secret),secret 默认 "zcode-credential-fallback:<platform>:<home>:<username>",
//     可用环境变量 ZCODE_CREDENTIAL_SECRET 覆盖(客户端本身也读这个变量)。
// 解不开时诚实报错,不静默吞掉。

use base64::engine::general_purpose::{URL_SAFE as B64U, URL_SAFE_NO_PAD as B64U_NP};
use base64::Engine as _;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::error::{AppError, AppResult};
use crate::models::ZCodeAccount;

pub const ENC_PREFIX: &str = "enc:v1:";
const DEVICE_KEY_PREFIX: &str = "web-remote-control:";

/// 平台标识(zcode 客户端口径:win32 / darwin / linux)。
pub fn zcode_platform_id() -> &'static str {
    if cfg!(windows) {
        "win32"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else {
        "linux"
    }
}

/// 未设置 ZCODE_CREDENTIAL_SECRET 时的回退密钥(纯函数)。
pub fn fallback_secret(home: &str, platform: &str, username: &str) -> String {
    format!("zcode-credential-fallback:{platform}:{home}:{username}")
}

/// 默认密钥来源:环境变量 ZCODE_CREDENTIAL_SECRET 优先,否则回退串。
pub fn default_secret(home: &str) -> String {
    if let Ok(s) = std::env::var("ZCODE_CREDENTIAL_SECRET") {
        if !s.is_empty() {
            return s;
        }
    }
    let username = std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_else(|_| "unknown".into());
    fallback_secret(home, zcode_platform_id(), &username)
}

fn derive_key(secret: &str) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(secret.as_bytes());
    h.finalize().into()
}

pub fn is_encrypted(v: &str) -> bool {
    v.starts_with(ENC_PREFIX)
}

fn b64u_decode(s: &str) -> AppResult<Vec<u8>> {
    B64U_NP
        .decode(s)
        .or_else(|_| B64U.decode(s))
        .map_err(|e| AppError::Credential(format!("enc:v1 base64 解码失败: {e}")))
}

/// 解密单个 "enc:v1:…" 值;非该格式或解密失败报错。
pub fn decrypt_with_secret(value: &str, secret: &str) -> AppResult<String> {
    let rest = value.strip_prefix(ENC_PREFIX).ok_or_else(|| AppError::Credential("不是 enc:v1 格式".into()))?;
    let parts: Vec<&str> = rest.split('.').collect();
    if parts.len() != 3 {
        return Err(AppError::Credential("enc:v1 格式不正确".into()));
    }
    let nonce = b64u_decode(parts[0])?;
    if nonce.len() != 12 {
        return Err(AppError::Credential("nonce 长度异常".into()));
    }
    let tag = b64u_decode(parts[1])?;
    let ct = b64u_decode(parts[2])?;
    use aes_gcm::aead::{Aead, KeyInit};
    use aes_gcm::{Aes256Gcm, Nonce};
    let cipher = Aes256Gcm::new_from_slice(&derive_key(secret)).map_err(|e| AppError::Credential(format!("密钥初始化失败: {e}")))?;
    let mut buf = ct;
    buf.extend_from_slice(&tag);
    let plain = cipher
        .decrypt(Nonce::from_slice(&nonce), buf.as_ref())
        .map_err(|_| AppError::Credential("解密失败（密钥不匹配或数据损坏）".into()))?;
    String::from_utf8(plain).map_err(|e| AppError::Credential(format!("明文不是 UTF-8: {e}")))
}

/// 加密成 "enc:v1:…"(与 decrypt_with_secret 互逆,供测试与本地工具用)。
pub fn encrypt_with_secret(plain: &str, secret: &str) -> String {
    use aes_gcm::aead::{Aead, KeyInit};
    use aes_gcm::{Aes256Gcm, Nonce};
    let cipher = Aes256Gcm::new_from_slice(&derive_key(secret)).expect("密钥长度固定 32");
    let nonce_bytes: [u8; 12] = rand::random();
    let sealed = cipher.encrypt(Nonce::from_slice(&nonce_bytes), plain.as_bytes()).expect("AES-GCM 加密失败");
    let (ct, tag) = sealed.split_at(sealed.len() - 16);
    format!(
        "{}{}.{}.{}",
        ENC_PREFIX,
        B64U_NP.encode(nonce_bytes),
        B64U_NP.encode(tag),
        B64U_NP.encode(ct)
    )
}

/// 值可能加密也可能明文,统一转明文;失败返回 None(不抛错)。
pub fn safe_decrypt(v: &Value, secret: &str) -> Option<String> {
    let s = v.as_str()?;
    if !is_encrypted(s) {
        return Some(s.to_string());
    }
    decrypt_with_secret(s, secret).ok()
}

/// 明文/加密的 JSON 字符串 → 解析后的对象;失败返回 None。
pub fn decrypt_json_opt(v: &Value, secret: &str) -> Option<Value> {
    let plain = safe_decrypt(v, secret)?;
    serde_json::from_str(&plain).ok()
}

/// 解码 JWT payload(不校验签名,仅用于识别用户)。
pub fn decode_jwt_payload(jwt: &str) -> Option<Value> {
    let seg = jwt.split('.').nth(1)?;
    let bytes = B64U_NP.decode(seg).or_else(|_| B64U.decode(seg)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// 账号身份(对齐 zcrypto.rs identity_with_secret)。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ZcodeIdentity {
    pub provider: String,
    pub username: Option<String>,
    pub display_name: Option<String>,
    pub email: Option<String>,
    pub user_id: Option<String>,
}

impl ZcodeIdentity {
    fn empty() -> Self {
        Self { provider: "bigmodel".into(), ..Default::default() }
    }
    pub fn has_signal(&self) -> bool {
        self.user_id.is_some() || self.username.is_some() || self.email.is_some()
    }
}

/// 从凭据对象里提取账号身份:先看 active_provider 的 user_info,找不到 user_id 时退而解码 access_token JWT。
pub fn identity_with_secret(creds: &Value, secret: &str) -> ZcodeIdentity {
    let mut id = ZcodeIdentity::empty();
    if let Some(ap) = creds.get("oauth:active_provider").and_then(|v| safe_decrypt(v, secret)) {
        if !ap.is_empty() {
            id.provider = ap;
        }
    } else if let Some(obj) = creds.as_object() {
        // 无 active_provider:从现有 oauth:<p>:access_token 键推断(zcrypto 兜底行为)
        if let Some(p) = obj
            .keys()
            .find_map(|k| k.strip_prefix("oauth:").and_then(|r| r.strip_suffix(":access_token")))
        {
            id.provider = p.to_string();
        }
    }
    if let Some(ui) = creds
        .get(format!("oauth:{}:user_info", id.provider))
        .and_then(|v| decrypt_json_opt(v, secret))
        .filter(|ui| ui.is_object())
    {
        id.username = ui.get("username").and_then(|v| v.as_str()).map(str::to_owned);
        id.display_name = ui.get("displayName").and_then(|v| v.as_str()).map(str::to_owned);
        id.email = ui
            .get("email")
            .and_then(|v| v.as_str())
            .or_else(|| ui.pointer("/rawProfile/email").and_then(|v| v.as_str()))
            .map(str::to_owned);
        if let Some(uid) = ui.get("id").filter(|v| !v.is_null()) {
            id.user_id = Some(match uid {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            });
        }
    }
    if id.user_id.is_none() {
        let at = creds.get(format!("oauth:{}:access_token", id.provider));
        let plain = at.and_then(|v| safe_decrypt(v, secret));
        if let Some(jwt) = plain.and_then(|p| decode_jwt_payload(&p)) {
            id.user_id = jwt
                .get("user_id")
                .or_else(|| jwt.get("sub"))
                .and_then(|v| v.as_str().map(str::to_owned).or_else(|| v.as_i64().map(|i| i.to_string())));
        }
    }
    id
}

pub fn identity_label(id: &ZcodeIdentity) -> Option<String> {
    id.display_name
        .clone()
        .or_else(|| id.username.clone())
        .or_else(|| id.email.clone())
}

/// 是否已登录:有任意 oauth:*:access_token,或有非空 zcodejwttoken。
pub fn is_logged_in(creds: &Value) -> bool {
    let Some(obj) = creds.as_object() else {
        return false;
    };
    if obj.keys().any(|k| k.starts_with("oauth:") && k.ends_with(":access_token")) {
        return true;
    }
    obj.get("zcodejwttoken").and_then(|v| v.as_str()).map(|s| !s.trim().is_empty()).unwrap_or(false)
}

/// 键序无关的稳定序列化(对象键排序)。
fn stable_stringify(v: &Value) -> String {
    match v {
        Value::Array(a) => format!("[{}]", a.iter().map(stable_stringify).collect::<Vec<_>>().join(",")),
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            let inner = keys
                .into_iter()
                .map(|k| format!("{}:{}", serde_json::to_string(k).unwrap_or_default(), stable_stringify(&m[k])))
                .collect::<Vec<_>>()
                .join(",");
            format!("{{{inner}}}")
        }
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

/// 账号去重哈希:剔除设备相关键(web-remote-control:*)后对 JSON 取 SHA256。
pub fn canonical_hash(creds: &Value) -> String {
    let obj = match creds {
        Value::Object(m) => {
            let filtered: serde_json::Map<String, Value> =
                m.iter().filter(|(k, _)| !k.starts_with(DEVICE_KEY_PREFIX)).map(|(k, v)| (k.clone(), v.clone())).collect();
            Value::Object(filtered)
        }
        other => other.clone(),
    };
    let mut h = Sha256::new();
    h.update(stable_stringify(&obj).as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

// ─── 本机凭据文件读取 ───

/// ZCode 客户端数据文件路径。
#[derive(Debug, Clone)]
pub struct ZcodePaths {
    pub home: PathBuf,
    pub credentials: PathBuf,
    pub config: PathBuf,
    pub telemetry: PathBuf,
}

use std::path::PathBuf;

pub fn zcode_paths_at(home: &Path) -> ZcodePaths {
    let v2 = home.join(".zcode").join("v2");
    ZcodePaths {
        home: home.to_path_buf(),
        credentials: v2.join("credentials.json"),
        config: v2.join("config.json"),
        telemetry: v2.join("telemetry-state.json"),
    }
}

/// 默认路径:ZCODE_HOME 环境变量优先,否则用户主目录。
pub fn zcode_paths() -> ZcodePaths {
    let home = std::env::var("ZCODE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| dirs::home_dir().unwrap_or_default());
    zcode_paths_at(&home)
}

use std::path::Path;

/// 本机 ZCode 登录快照(供导入账号)。
#[derive(Debug, Clone)]
pub struct LocalZcodeSnapshot {
    /// credentials.json 原文(各字段保持 enc:v1 密文)
    pub creds: Value,
    pub config: Option<Value>,
    pub device_mid: Option<String>,
    pub identity: ZcodeIdentity,
    pub label: String,
}

fn read_json_opt(path: &Path) -> Option<Value> {
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

/// 读取本机 ZCode 当前登录(读 ~/.zcode/v2/credentials.json + 解密身份)。
/// 文件缺失/未登录/解不开身份时诚实报错。
pub fn read_local_snapshot() -> AppResult<LocalZcodeSnapshot> {
    read_local_snapshot_at(&zcode_paths())
}

pub fn read_local_snapshot_at(paths: &ZcodePaths) -> AppResult<LocalZcodeSnapshot> {
    if !paths.credentials.exists() {
        return Err(AppError::Credential(format!(
            "未找到 ZCode 凭据文件({}),请确认已安装并登录 ZCode 桌面客户端",
            paths.credentials.display()
        )));
    }
    let creds = read_json_opt(&paths.credentials)
        .ok_or_else(|| AppError::Credential("credentials.json 解析失败".into()))?;
    if !is_logged_in(&creds) {
        return Err(AppError::Credential("ZCode 尚未登录(凭据里没有 access_token / zcodejwttoken)".into()));
    }
    let secret = default_secret(&paths.home.to_string_lossy());
    let identity = identity_with_secret(&creds, &secret);
    if !identity.has_signal() {
        return Err(AppError::Credential(
            "凭据解密失败:无法提取账号身份(密钥不匹配或数据损坏;如设过 ZCODE_CREDENTIAL_SECRET 请确认一致)".into(),
        ));
    }
    let label = identity_label(&identity).unwrap_or_else(|| identity.user_id.clone().unwrap_or_default());
    let device_mid = read_json_opt(&paths.telemetry)
        .and_then(|t| t.get("deviceMid").and_then(|v| v.as_str().map(str::to_owned)));
    Ok(LocalZcodeSnapshot {
        creds,
        config: read_json_opt(&paths.config),
        device_mid,
        identity,
        label,
    })
}

// ─── token 解析(zcodeClient.js candidateTokens / billingTokens / claimToken) ───

fn push_token(out: &mut Vec<String>, t: Option<String>) {
    if let Some(t) = t {
        if t.trim().len() > 20 && !out.contains(&t) {
            out.push(t);
        }
    }
}

/// 按 zcode-switch candidate_tokens 的顺序收集可用 token(去重):
/// coding-plan provider apiKey(启用优先)→ zcodejwttoken → oauth 候选。
/// 无凭据快照的手动账号直接用 token 字段。
pub fn candidate_tokens(account: &ZCodeAccount, secret: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if account.credentials.is_none() {
        push_token(&mut out, Some(account.token.clone()));
        return out;
    }
    let creds = account.credentials.clone().unwrap_or_default();
    let config = account.config.clone().unwrap_or_default();
    if let Some(providers) = config.get("provider").and_then(|v| v.as_object()) {
        let mut entries: Vec<(&String, &Value)> = providers.iter().collect();
        entries.sort_by_key(|(_, p)| p.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false) == true);
        for (id, p) in entries {
            let k = p.pointer("/options/apiKey").and_then(|v| v.as_str());
            if id.contains("coding-plan") && k.is_some_and(|k| !k.starts_with("enc:")) {
                push_token(&mut out, k.map(str::to_owned));
            }
        }
    }
    push_token(&mut out, creds.get("zcodejwttoken").and_then(|v| safe_decrypt(v, secret)));
    let active = creds
        .get("oauth:active_provider")
        .and_then(|v| safe_decrypt(v, secret))
        .unwrap_or_else(|| "zai".into());
    for key in [format!("oauth:{active}:access_token"), "oauth:bigmodel:access_token".into(), "oauth:zai:access_token".into()] {
        push_token(&mut out, creds.get(key).and_then(|v| safe_decrypt(v, secret)));
    }
    out
}

/// Start Plan / billing 渠道 token:zcodejwttoken 优先,其次 config 中 start-plan provider 的 key。
pub fn billing_tokens(account: &ZCodeAccount, secret: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if account.credentials.is_none() {
        push_token(&mut out, Some(account.token.clone()));
        return out;
    }
    let creds = account.credentials.clone().unwrap_or_default();
    let config = account.config.clone().unwrap_or_default();
    push_token(&mut out, creds.get("zcodejwttoken").and_then(|v| safe_decrypt(v, secret)));
    if let Some(providers) = config.get("provider").and_then(|v| v.as_object()) {
        for (id, p) in providers {
            let k = p.pointer("/options/apiKey").and_then(|v| v.as_str());
            if id.contains("start-plan") && k.is_some_and(|k| !k.starts_with("enc:")) {
                push_token(&mut out, k.map(str::to_owned));
            }
        }
    }
    out
}

/// 领取用的 token:zcodejwttoken 优先(与 claim.rs claim_token 一致);没有则诚实报错。
pub fn claim_token(account: &ZCodeAccount) -> AppResult<String> {
    let home = dirs::home_dir().unwrap_or_default().to_string_lossy().to_string();
    let secret = default_secret(&home);
    billing_tokens(account, &secret)
        .into_iter()
        .next()
        .ok_or_else(|| {
            AppError::Credential("账号快照里没有可用的 zcodejwttoken，请在 ZCode 重新登录（或重新本机导入）后再领取".into())
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const SECRET: &str = "test-secret";

    #[test]
    fn enc_v1_roundtrip_and_format() {
        let enc = encrypt_with_secret("hello 智谱", SECRET);
        assert!(enc.starts_with("enc:v1:"));
        assert_eq!(enc.split('.').count(), 3, "enc:v1:<nonce>.<tag>.<ct>");
        assert_eq!(decrypt_with_secret(&enc, SECRET).unwrap(), "hello 智谱");
        // 错误密钥解不开 → 诚实报错
        assert!(decrypt_with_secret(&enc, "wrong").is_err());
        assert!(decrypt_with_secret("plain", SECRET).is_err(), "非 enc:v1 格式报错");
        assert!(decrypt_with_secret("enc:v1:only.two", SECRET).is_err());
        assert!(is_encrypted(&enc));
        assert!(!is_encrypted("plain"));
    }

    #[test]
    fn fallback_secret_format_and_env_override() {
        assert_eq!(
            fallback_secret("C:\\Users\\admin", "win32", "admin"),
            "zcode-credential-fallback:win32:C:\\Users\\admin:admin"
        );
        // 环境变量覆盖(仅在变量已设置时生效,不强制断言其存在)
        let s = default_secret("C:\\h");
        assert!(!s.is_empty());
        assert_eq!(zcode_platform_id(), if cfg!(windows) { "win32" } else if cfg!(target_os = "macos") { "darwin" } else { "linux" });
    }

    #[test]
    fn identity_extraction_and_jwt_fallback() {
        // user_info 路径
        let creds = json!({
            "oauth:active_provider": encrypt_with_secret("bigmodel", SECRET),
            "oauth:bigmodel:user_info": encrypt_with_secret(
                &json!({"username": "u1", "displayName": "用户一", "email": "u1@z.ai", "id": 12345}).to_string(),
                SECRET
            ),
        });
        let id = identity_with_secret(&creds, SECRET);
        assert_eq!(id.provider, "bigmodel");
        assert_eq!(id.user_id.as_deref(), Some("12345"), "数字 id 转字符串");
        assert_eq!(id.email.as_deref(), Some("u1@z.ai"));
        assert_eq!(identity_label(&id).as_deref(), Some("用户一"));
        // JWT 兜底路径
        let jwt_payload = json!({"user_id": "uid-from-jwt"});
        let jwt = format!("h.{}.s", B64U_NP.encode(jwt_payload.to_string()));
        let creds2 = json!({ "oauth:zai:access_token": jwt });
        let id2 = identity_with_secret(&creds2, SECRET);
        assert_eq!(id2.provider, "zai");
        assert_eq!(id2.user_id.as_deref(), Some("uid-from-jwt"));
    }

    #[test]
    fn is_logged_in_and_canonical_hash() {
        assert!(!is_logged_in(&json!({})));
        assert!(is_logged_in(&json!({"zcodejwttoken": "tok"})));
        assert!(is_logged_in(&json!({"oauth:zai:access_token": "tok"})));
        assert!(!is_logged_in(&json!({"zcodejwttoken": "  "})));
        // canonical_hash:键序无关;剔除 web-remote-control:*
        let a = json!({"zcodejwttoken": "t", "web-remote-control:relay": "secret-device-key"});
        let b = json!({"web-remote-control:relay": "secret-device-key", "zcodejwttoken": "t"});
        assert_eq!(canonical_hash(&a), canonical_hash(&b), "键序无关且设备键被剔除");
        let c = json!({"zcodejwttoken": "other"});
        assert_ne!(canonical_hash(&a), canonical_hash(&c));
    }

    #[test]
    fn token_resolution_order() {
        // billing_tokens:zcodejwttoken 优先
        let account = ZCodeAccount {
            id: "z".into(),
            credentials: Some(json!({
                "zcodejwttoken": encrypt_with_secret("jwt-token-0123456789abcdef", SECRET),
                "oauth:zai:access_token": encrypt_with_secret("oauth-token-0123456789abcdef", SECRET),
            })),
            ..Default::default()
        };
        let secret = "test-secret";
        let billing = billing_tokens(&account, secret);
        assert_eq!(billing[0], "jwt-token-0123456789abcdef");
        let candidates = candidate_tokens(&account, secret);
        assert!(candidates.contains(&"jwt-token-0123456789abcdef".to_string()));
        assert!(candidates.contains(&"oauth-token-0123456789abcdef".to_string()));
        // 手动账号:直接用 token 字段
        let manual = ZCodeAccount { id: "m".into(), token: "manual-api-key-0123456789".into(), ..Default::default() };
        assert_eq!(candidate_tokens(&manual, secret), vec!["manual-api-key-0123456789".to_string()]);
        assert_eq!(billing_tokens(&manual, secret), vec!["manual-api-key-0123456789".to_string()]);
        // 无 token → 诚实报错
        assert!(claim_token(&ZCodeAccount::default()).is_err());
    }

    #[test]
    fn read_local_snapshot_honest_errors() {
        let dir = std::env::temp_dir().join(format!("credit-core-zc-{}", uuid::Uuid::new_v4()));
        let paths = zcode_paths_at(&dir);
        // 文件不存在
        assert!(read_local_snapshot_at(&paths).is_err());
        // 未登录
        std::fs::create_dir_all(&paths.credentials.parent().unwrap()).unwrap();
        std::fs::write(&paths.credentials, "{}").unwrap();
        let err = read_local_snapshot_at(&paths).unwrap_err().to_string();
        assert!(err.contains("尚未登录"), "{err}");
        // 已登录但身份解不开(乱密文)→ 诚实报错
        std::fs::write(&paths.credentials, json!({"zcodejwttoken": "t"}).to_string()).unwrap();
        // zcodejwttoken 非空即算登录;身份提取需要 user_info/access_token,这里 access_token 是明文 JWT
        let jwt = format!("h.{}.s", B64U_NP.encode(json!({"sub": "u9"}).to_string()));
        std::fs::write(&paths.credentials, json!({"oauth:zai:access_token": jwt}).to_string()).unwrap();
        let snap = read_local_snapshot_at(&paths).unwrap();
        assert_eq!(snap.identity.user_id.as_deref(), Some("u9"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
