// 从本机 WPS 灵犀客户端(Electron)导入当前登录态。
//
// 协议事实(本机已验证):
//   1. userData 固定为 %APPDATA%\WPS 灵犀\(与安装目录无关,目录名带空格)
//   2. Cookie 存 Chromium SQLite:%APPDATA%\WPS 灵犀\Network\Cookies(表 cookies)
//   3. 解密密钥:%APPDATA%\WPS 灵犀\Local State → os_crypt.encrypted_key
//      (base64,前 5 字节 "DPAPI" → CryptUnprotectData → AES-256 密钥)
//   4. encrypted_value 前缀 "v10" = AES-256-GCM(nonce 12B + 密文 + tag 16B);
//      "v20" 为 App-Bound 加密暂不支持;无前缀为旧版明文直接用
//   5. 灵犀运行中时 Cookies 文件可能被锁:std::fs::read 在 Windows 以
//      FILE_SHARE_READ|WRITE|DELETE 打开,多数情况能读到;读失败则提示先退出客户端
//
// 多账号现实:一个灵犀客户端同一时刻只有一个登录态,多账号 = 用户在客户端
// 切换登录后逐个导入(调用方负责提示)。

use rusqlite::OpenFlags;

use crate::error::{AppError, AppResult};

/// 灵犀客户端 userData 目录名(%APPDATA% 下,带空格)。
const APP_DIR_NAME: &str = "WPS 灵犀";

/// 导入结果:拼好的 Cookie 请求头 + host + 有效 cookie 数。
#[derive(Debug, Clone)]
pub struct LingxiLocalImport {
    pub cookie_header: String,
    pub host: String,
    pub cookie_count: usize,
}

/// 灵犀客户端数据目录(dirs::config_dir() = %APPDATA%)。
pub fn lingxi_data_dir() -> std::path::PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(APP_DIR_NAME)
}

/// 从 URL 字符串提取 host(手动解析,纯函数):跳过 scheme,取到第一个 / ? # 或结尾,
/// 去掉 userinfo 与端口,转小写。仅支持 http/https 及省略 scheme 的 "host/..." 形态。
pub fn extract_host(url: &str) -> Option<String> {
    let raw = url.trim();
    if raw.is_empty() {
        return None;
    }
    let after_scheme = match raw.find("://") {
        Some(i) => &raw[i + 3..],
        None => raw,
    };
    // "//host/..." 协议相对形态
    let after_scheme = after_scheme.strip_prefix("//").unwrap_or(after_scheme);
    let authority = after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("");
    // 去掉 userinfo(user:pass@host)
    let host_port = authority.rsplit('@').next().unwrap_or(authority);
    // IPv6 字面量 [::1]:8080 → 保留 [] 内整体
    if let Some(end) = host_port.rfind(']') {
        let host = host_port.get(..=end)?;
        if host.starts_with('[') {
            return Some(host.to_ascii_lowercase());
        }
        return None;
    }
    let host = host_port.split(':').next()?;
    if host.is_empty() {
        return None;
    }
    Some(host.to_ascii_lowercase())
}

/// host 与 Chromium host_key 的匹配(纯函数,与查询语义一致):
/// 精确相等;或 host_key 以 '.' 开头(host == 去点域名,或 host 是 host_key 的后缀,即父域 cookie)。
pub fn is_host_match(host: &str, host_key: &str) -> bool {
    let host = host.to_ascii_lowercase();
    let hk = host_key.to_ascii_lowercase();
    if let Some(domain) = hk.strip_prefix('.') {
        return host == domain || host.ends_with(&hk);
    }
    hk == host
}

/// Chromium expires_utc(1601-01-01 起 100ns)是否已过期(纯函数)。
/// <=0 为会话 cookie,永不过期;换算 Unix 秒后 <= now 视为过期(RFC 6265 口径)。
pub fn is_cookie_expired(expires_utc: i64, now_unix_secs: i64) -> bool {
    if expires_utc <= 0 {
        return false;
    }
    let unix_secs = expires_utc / 10_000_000 - 11_644_473_600;
    unix_secs <= now_unix_secs
}

/// Local State → os_crypt.encrypted_key → DPAPI 解出 AES-256 密钥(仅 Windows)。
#[cfg(windows)]
fn read_os_crypt_key(data_dir: &std::path::Path) -> AppResult<Vec<u8>> {
    use base64::engine::general_purpose::STANDARD as B64;
    use base64::Engine as _;
    use windows::Win32::Security::Cryptography::{CryptUnprotectData, CRYPT_INTEGER_BLOB};

    let ls_path = data_dir.join("Local State");
    let ls: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&ls_path)
            .map_err(|e| AppError::Credential(format!("读取灵犀 Local State 失败: {e}")))?,
    )?;
    let enc_key_b64 = ls
        .pointer("/os_crypt/encrypted_key")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AppError::Credential("灵犀 Local State 中没有 os_crypt.encrypted_key".into()))?;
    let enc = B64
        .decode(enc_key_b64)
        .map_err(|e| AppError::Credential(format!("encrypted_key base64 解码失败: {e}")))?;
    if enc.len() < 5 || &enc[..5] != b"DPAPI" {
        return Err(AppError::Credential("灵犀 Local State 中没有 DPAPI 密钥".into()));
    }
    let dpapi_unprotect = |data: &[u8]| -> AppResult<Vec<u8>> {
        unsafe {
            let in_blob = CRYPT_INTEGER_BLOB {
                cbData: data.len() as u32,
                pbData: data.as_ptr() as *mut u8,
            };
            let mut out_blob = CRYPT_INTEGER_BLOB {
                cbData: 0,
                pbData: std::ptr::null_mut(),
            };
            CryptUnprotectData(&in_blob, None, None, None, None, 0, &mut out_blob)
                .map_err(|e| AppError::Dpapi(e.to_string()))?;
            Ok(std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize).to_vec())
        }
    };
    let key = dpapi_unprotect(&enc[5..])?;
    if key.len() != 32 {
        return Err(AppError::Credential(format!("解出的 AES 密钥长度异常: {}", key.len())));
    }
    Ok(key)
}

#[cfg(not(windows))]
fn read_os_crypt_key(_data_dir: &std::path::Path) -> AppResult<Vec<u8>> {
    Err(AppError::Credential(
        "从本机灵犀导入登录态仅支持 Windows".into(),
    ))
}

/// 解密单条 encrypted_value(纯逻辑,可测):
/// v10 → AES-256-GCM;nul 前缀 v20 → 明确报暂不支持;无前缀 → 明文直接用。
fn decrypt_cookie_value(key: &[u8], encrypted: &[u8]) -> AppResult<String> {
    if encrypted.starts_with(b"v20") {
        return Err(AppError::Credential(
            "灵犀 Cookie 为 v20(App-Bound)加密格式，暂不支持".into(),
        ));
    }
    if encrypted.starts_with(b"v10") {
        if key.len() != 32 {
            return Err(AppError::Credential(format!("AES 密钥长度异常: {}", key.len())));
        }
        if encrypted.len() < 3 + 12 + 16 {
            return Err(AppError::Credential("v10 Cookie 数据过短".into()));
        }
        let nonce = &encrypted[3..15];
        let tag = &encrypted[encrypted.len() - 16..];
        let ct = &encrypted[15..encrypted.len() - 16];
        use aes_gcm::aead::{Aead, KeyInit};
        use aes_gcm::{Aes256Gcm, Nonce};
        let cipher = Aes256Gcm::new_from_slice(key)
            .map_err(|e| AppError::Credential(format!("密钥初始化失败: {e}")))?;
        let mut buf = ct.to_vec();
        buf.extend_from_slice(tag);
        let plain = cipher
            .decrypt(Nonce::from_slice(nonce), buf.as_ref())
            .map_err(|_| AppError::Credential("AES-GCM 解密失败(密钥不匹配或数据损坏)".into()))?;
        return String::from_utf8(plain)
            .map_err(|e| AppError::Credential(format!("Cookie 明文不是 UTF-8: {e}")));
    }
    // 无前缀:Chromium 旧版明文
    String::from_utf8(encrypted.to_vec())
        .map_err(|e| AppError::Credential(format!("Cookie 明文不是 UTF-8: {e}")))
}

/// Cookies 行(排序与拼接所需的最小字段)。
struct CookieRow {
    host_key: String,
    name: String,
    value: String,
    path: String,
    expires_utc: i64,
}

/// 输出排序(确定性):host_key 精确度倒序 → name 字典序 → path 长度倒序 → path 字典序。
fn sort_rows(rows: &mut [CookieRow]) {
    rows.sort_by(|a, b| {
        b.host_key
            .len()
            .cmp(&a.host_key.len())
            .then_with(|| a.host_key.cmp(&b.host_key))
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| b.path.len().cmp(&a.path.len()))
            .then_with(|| b.path.cmp(&a.path))
    });
}

/// 查询并解密 Cookies SQLite 字节(已读入内存)中 host 匹配且未过期的 cookie,
/// 返回 "name=value; name=value" 形式的 Cookie 请求头。key 由调用方解出(便于单测)。
fn query_cookie_header(bytes: &[u8], host: &str, now_unix_secs: i64, key: &[u8]) -> AppResult<String> {
    // 灵犀运行中可能持有文件锁:写唯一临时副本后只读打开(Windows 下 fs::read 以
    // FILE_SHARE_READ|WRITE|DELETE 打开,多数情况可直接读;失败由调用方提示退出客户端)。
    let tmp = std::env::temp_dir().join(format!(
        "wb-switch-lingxi-cookies-{}.db",
        uuid::Uuid::new_v4()
    ));
    std::fs::write(&tmp, bytes)?;
    let rows = (|| -> AppResult<Vec<(String, String, Vec<u8>, String, i64)>> {
        let conn = rusqlite::Connection::open_with_flags(&tmp, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| AppError::Credential(format!("打开 Cookies 数据库失败: {e}")))?;
        let mut stmt = conn
            .prepare(
                "SELECT host_key, name, encrypted_value, path, expires_utc FROM cookies",
            )
            .map_err(|e| AppError::Credential(format!("Cookies 数据库缺少 cookies 表: {e}")))?;
        let raw = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            })
            .map_err(|e| AppError::Credential(format!("读取 Cookies 数据库失败: {e}")))?;
        let mut collected = Vec::new();
        for r in raw {
            let (host_key, name, encrypted, path, expires_utc) =
                r.map_err(|e| AppError::Credential(format!("读取 Cookies 行失败: {e}")))?;
            collected.push((host_key, name, encrypted, path, expires_utc));
        }
        Ok(collected)
    })();
    let _ = std::fs::remove_file(&tmp); // 临时副本用完即删(失败不影响主流程)
    let raw_rows = rows?;

    let mut rows: Vec<CookieRow> = Vec::new();
    for (host_key, name, encrypted, path, expires_utc) in raw_rows {
        if name.is_empty() || !is_host_match(host, &host_key) {
            continue;
        }
        if is_cookie_expired(expires_utc, now_unix_secs) {
            continue;
        }
        if encrypted.is_empty() {
            continue;
        }
        // v20(App-Bound)加密整批直接报明确错误;其余单条解密失败跳过,不拖垮导入
        if encrypted.starts_with(b"v20") {
            return Err(AppError::Credential(
                "灵犀 Cookie 为 v20(App-Bound)加密格式，暂不支持".into(),
            ));
        }
        if let Ok(value) = decrypt_cookie_value(&key, &encrypted) {
            if !value.is_empty() {
                rows.push(CookieRow { host_key, name, value, path, expires_utc });
            }
        }
    }
    sort_rows(&mut rows);
    Ok(rows
        .iter()
        .map(|r| format!("{}={}", r.name, r.value))
        .collect::<Vec<_>>()
        .join("; "))
}

/// 从本机灵犀客户端导入当前登录态:读取 Cookies SQLite(临时副本),
/// 解密出 host 匹配且未过期的 cookie,拼成 Cookie 请求头。
pub fn import_from_local(checkin_url: &str) -> AppResult<LingxiLocalImport> {
    let host = extract_host(checkin_url)
        .ok_or_else(|| AppError::Credential("无法从 checkinUrl 解析主机名，请检查链接".into()))?;

    let cookies_path = lingxi_data_dir().join("Network").join("Cookies");
    if !cookies_path.exists() {
        return Err(AppError::Credential(format!(
            "未找到灵犀客户端 Cookie 数据({})，请确认已安装灵犀客户端并登录过",
            cookies_path.display()
        )));
    }
    // 解密密钥:Local State → DPAPI(非 Windows 在此明确报仅支持)
    let key = read_os_crypt_key(&lingxi_data_dir())?;
    // 读失败(灵犀运行中持锁等)→ 明确提示先退出客户端
    let bytes = std::fs::read(&cookies_path).map_err(|_| {
        AppError::Credential("灵犀客户端正在运行，请先完全退出后重试".into())
    })?;

    let now_unix_secs = chrono::Utc::now().timestamp();
    let cookie_header = query_cookie_header(&bytes, &host, now_unix_secs, &key)?;
    let cookie_count = cookie_header.split("; ").filter(|s| !s.is_empty()).count();
    if cookie_count == 0 {
        return Err(AppError::Credential(format!(
            "未在灵犀客户端找到 {host} 的登录 Cookie，请确认客户端当前已登录该站点"
        )));
    }
    Ok(LingxiLocalImport { cookie_header, host, cookie_count })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_host_variants() {
        assert_eq!(extract_host("https://lingxi.wps.cn/api/checkin"), Some("lingxi.wps.cn".into()));
        assert_eq!(extract_host("http://Lingxi.WPS.CN:8080/x?y=1"), Some("lingxi.wps.cn".into()));
        assert_eq!(extract_host("POST https://a.b.com/c"), Some("a.b.com".into()), "容忍抓包粘贴的前缀");
        assert_eq!(extract_host("lingxi.wps.cn/checkin"), Some("lingxi.wps.cn".into()), "无 scheme 形态");
        assert_eq!(extract_host("https://user:pass@host.example.com/p"), Some("host.example.com".into()));
        assert_eq!(extract_host("https://host.example.com#frag"), Some("host.example.com".into()));
        assert_eq!(extract_host("http://[::1]:8080/x"), Some("[::1]".into()));
        assert_eq!(extract_host(""), None);
        assert_eq!(extract_host("   "), None);
        assert_eq!(extract_host("https://"), None);
    }

    #[test]
    fn host_match_rules() {
        // 精确
        assert!(is_host_match("lingxi.wps.cn", "lingxi.wps.cn"));
        // 自身点前缀
        assert!(is_host_match("lingxi.wps.cn", ".lingxi.wps.cn"));
        // 父域 cookie
        assert!(is_host_match("lingxi.wps.cn", ".wps.cn"));
        assert!(is_host_match("lingxi.wps.cn", ".cn"));
        // 大小写不敏感
        assert!(is_host_match("Lingxi.WPS.CN", ".WPS.CN"));
        // 点前缀即子域边界:任意 *wps.cn 都命中 .cn
        assert!(is_host_match("xwps.cn", ".cn"));
        // 不匹配
        assert!(!is_host_match("lingxi.wps.cn", "other.wps.cn"));
        assert!(!is_host_match("a.b.com", "b.com"), "无点前缀的 host_key 不做后缀匹配");
    }

    #[test]
    fn expiry_filter_chromium_epoch() {
        // 会话 cookie(expires_utc=0)保留
        assert!(!is_cookie_expired(0, 1_000_000_000));
        assert!(!is_cookie_expired(-5, 1_000_000_000));
        // 恰好等于当前秒 → 过期(RFC 6265 口径)
        let expiry_chromium = (1_000_000_020 + 11_644_473_600) * 10_000_000;
        assert!(!is_cookie_expired(expiry_chromium, 1_000_000_010), "未来未过期");
        assert!(is_cookie_expired(expiry_chromium, 1_000_000_020), "到期秒视为过期");
        assert!(is_cookie_expired(expiry_chromium, 1_000_000_030), "已过视为过期");
    }

    #[test]
    fn v10_decrypt_roundtrip_and_prefixes() {
        use aes_gcm::aead::{Aead, KeyInit};
        use aes_gcm::{Aes256Gcm, Nonce};
        let key = [0x42u8; 32];
        let cipher = Aes256Gcm::new_from_slice(&key).unwrap();
        let nonce_bytes = [7u8; 12];
        let plain = b"wps_sid=abc123; path=/";
        let sealed = cipher
            .encrypt(Nonce::from_slice(&nonce_bytes), plain.as_ref())
            .unwrap();
        let mut data = Vec::new();
        data.extend_from_slice(b"v10");
        data.extend_from_slice(&nonce_bytes);
        data.extend_from_slice(&sealed);

        let got = decrypt_cookie_value(&key, &data).unwrap();
        assert_eq!(got, String::from_utf8_lossy(plain));

        // 密钥错误 → 解密失败
        let bad_key = [0x43u8; 32];
        assert!(decrypt_cookie_value(&bad_key, &data).is_err());

        // 无前缀 → 明文直接用
        assert_eq!(decrypt_cookie_value(&key, b"plain-cookie").unwrap(), "plain-cookie");

        // v20 → 明确报暂不支持
        let err = decrypt_cookie_value(&key, b"v20_rest_is_opaque").unwrap_err().to_string();
        assert!(err.contains("暂不支持"), "实际: {err}");
        // v10 过短 → 报错而非 panic
        assert!(decrypt_cookie_value(&key, b"v10short").is_err());
    }

    #[test]
    fn sort_rows_deterministic_order() {
        let mk = |host: &str, name: &str, path: &str| CookieRow {
            host_key: host.into(),
            name: name.into(),
            value: "v".into(),
            path: path.into(),
            expires_utc: 0,
        };
        let mut rows = vec![
            mk("lingxi.wps.cn", "a", "/"),
            mk(".wps.cn", "a", "/"),
            mk(".wps.cn", "b", "/deep/path"),
            mk(".wps.cn", "a", "/deep/path"),
            mk("lingxi.wps.cn", "b", "/"),
        ];
        sort_rows(&mut rows);
        let keys: Vec<String> = rows.iter().map(|r| format!("{}/{}", r.host_key, r.name)).collect();
        // host_key 精确度倒序 → name 字典序
        assert_eq!(
            keys,
            vec![
                "lingxi.wps.cn/a",
                "lingxi.wps.cn/b",
                ".wps.cn/a",
                ".wps.cn/a",
                ".wps.cn/b"
            ],
            "实际: {keys:?}"
        );
        // 同 host+name 下 path 长度倒序
        let paths: Vec<&str> = rows[2..4].iter().map(|r| r.path.as_str()).collect();
        assert_eq!(paths, vec!["/deep/path", "/"]);
    }

    #[test]
    fn import_from_local_rejects_bad_url() {
        // 无效 URL → host 解析失败(纯逻辑,不依赖本机环境)
        let err = import_from_local("::::").unwrap_err().to_string();
        assert!(err.contains("主机名"), "实际: {err}");
        let err = import_from_local("").unwrap_err().to_string();
        assert!(err.contains("主机名"), "实际: {err}");
    }

    /// 测试用 v10 加密(布局与 decrypt_cookie_value 对应:v10 + nonce12 + ct + tag16)。
    fn seal_v10(key: &[u8; 32], plain: &[u8]) -> Vec<u8> {
        use aes_gcm::aead::{Aead, KeyInit};
        use aes_gcm::{Aes256Gcm, Nonce};
        let cipher = Aes256Gcm::new_from_slice(key).unwrap();
        let nonce = [0x11u8; 12];
        let mut data = Vec::new();
        data.extend_from_slice(b"v10");
        data.extend_from_slice(&nonce);
        data.extend_from_slice(&cipher.encrypt(Nonce::from_slice(&nonce), plain).unwrap());
        data
    }

    /// 造一个 Chromium 形态的 Cookies 库(最小列集)并返回文件字节。
    fn synth_cookies_db(rows: &[(&str, &str, Vec<u8>, &str, i64)]) -> Vec<u8> {
        let path = std::env::temp_dir().join(format!("wb-test-cookies-{}.db", uuid::Uuid::new_v4()));
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute(
            "CREATE TABLE cookies (host_key TEXT, name TEXT, encrypted_value BLOB, path TEXT, is_secure INTEGER, is_httponly INTEGER, expires_utc INTEGER)",
            [],
        )
        .unwrap();
        for (host_key, name, encrypted, path, expires_utc) in rows {
            conn.execute(
                "INSERT INTO cookies VALUES (?1, ?2, ?3, ?4, 0, 0, ?5)",
                rusqlite::params![host_key, name, encrypted, path, expires_utc],
            )
            .unwrap();
        }
        drop(conn);
        let bytes = std::fs::read(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        bytes
    }

    #[test]
    fn query_cookie_header_pipeline() {
        let key = [0x42u8; 32];
        let now = 1_000_000_000i64;
        let future = (now + 3600 + 11_644_473_600) * 10_000_000;
        let past = (now - 3600 + 11_644_473_600) * 10_000_000;
        let bytes = synth_cookies_db(&[
            ("lingxi.wps.cn", "wps_sid", seal_v10(&key, b"sid-main"), "/", 0),
            (".wps.cn", "common", seal_v10(&key, b"parent-val"), "/", future),
            ("lingxi.wps.cn", "gone", seal_v10(&key, b"expired"), "/", past),
            ("other.example.com", "wps_sid", seal_v10(&key, b"zzz"), "/", 0),
            (".wps.cn", "deep", b"rawval".to_vec(), "/a/b", 0),
            ("lingxi.wps.cn", "", seal_v10(&key, b"noname"), "/", 0),
            ("lingxi.wps.cn", "corrupt", b"v10garbage-not-aes".to_vec(), "/", 0),
        ]);
        let header = query_cookie_header(&bytes, "lingxi.wps.cn", now, &key).unwrap();
        // host 匹配 + 过滤过期/无名/损坏 + 稳定排序(host_key 精确度倒序 → name)
        assert_eq!(header, "wps_sid=sid-main; common=parent-val; deep=rawval", "实际: {header}");
        // 零命中
        let empty = synth_cookies_db(&[("other.example.com", "wps_sid", seal_v10(&key, b"z"), "/", 0)]);
        let header = query_cookie_header(&empty, "lingxi.wps.cn", now, &key).unwrap();
        assert_eq!(header, "");
    }

    #[test]
    fn query_cookie_header_rejects_v20() {
        let key = [0x42u8; 32];
        let bytes = synth_cookies_db(&[(
            "lingxi.wps.cn",
            "wps_sid",
            b"v20app-bound-opaque".to_vec(),
            "/",
            0,
        )]);
        let err = query_cookie_header(&bytes, "lingxi.wps.cn", 0, &key).unwrap_err().to_string();
        assert!(err.contains("暂不支持") && err.contains("v20"), "实际: {err}");
    }
}
