// Qoder 设备身份 — 移植自 CreditDaddy qoderApp.js + qoderUmid.js + localDetect.js。
//
//   1. 客户端探测:国际版 Qoder / 国内版 Qoder CN 的程序目录、数据目录、版本号
//   2. 设备风控身份:调用客户端自带的 runtime-info(UMID)生成
//      Cosy-MachineToken / Cosy-MachineCode / Cosy-MachineType(env:国际版 App=3、国内版 App=0;
//      CLI 组件国际版=4、国内版=0),按 provider:uid 缓存 TTL 50min
//   3. 登录凭据读取:auth.v1.dat(Electron safeStorage)=
//      "v10" + nonce(12) + AES-256-GCM(密文+tag),密钥 = DPAPI 解开 Local State 的 os_crypt.encrypted_key
//      (仅 Windows;非 Windows 诚实报错,仍可手动录入 token)

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;

use crate::error::{AppError, AppResult};

const DEFAULT_CLIENT_VERSION: &str = "0.2.5";
const RISK_TTL: Duration = Duration::from_secs(50 * 60);
const RISK_TIMEOUT: Duration = Duration::from_secs(25);
const MAX_RISK_FIELD: usize = 4096;

/// runtime-info env 值(客户端实测;qodercli 全球区=4、国内 ONLINE=0)
pub const APP_ENV_INTL: i64 = 3;
pub const APP_ENV_CN: i64 = 0;
pub const CLI_ENV_INTL: i64 = 4;
pub const CLI_ENV_CN: i64 = 0;

struct Variant {
    provider: &'static str,
    label: &'static str,
    install_name: &'static str,
    app_id: &'static str,
}

const VARIANTS: [Variant; 2] = [
    Variant { provider: "qoder", label: "Qoder 国际版", install_name: "Qoder", app_id: "com.qoder.app" },
    Variant { provider: "qoder-cn", label: "Qoder 国内版", install_name: "Qoder CN", app_id: "com.qodercn.app" },
];

fn variant_of(region: &str) -> &'static Variant {
    VARIANTS.iter().find(|v| v.provider == region).unwrap_or(&VARIANTS[0])
}

fn exists(p: &Path) -> bool {
    p.exists()
}

/// region("intl"/"cn")→ 本地 provider("qoder"/"qoder-cn")。
pub fn provider_of(region: &str) -> &'static str {
    variant_of(region).provider
}

fn candidate_resource_dirs(v: &Variant) -> Vec<PathBuf> {
    let home = dirs::home_dir().unwrap_or_default();
    if cfg!(windows) {
        let local = std::env::var("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home.join("AppData").join("Local"));
        let pf = std::env::var("ProgramFiles")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("C:\\Program Files"));
        return vec![local.join("Programs").join(v.install_name).join("resources"), pf.join(v.install_name).join("resources")];
    }
    if cfg!(target_os = "macos") {
        return vec![
            PathBuf::from(format!("/Applications/{}.app/Contents/Resources", v.install_name)),
            home.join("Applications").join(format!("{}.app", v.install_name)).join("Contents").join("Resources"),
        ];
    }
    let slug = v.install_name.to_lowercase().replace(' ', "-");
    vec![
        PathBuf::from(format!("/opt/{}/resources", v.install_name)),
        PathBuf::from(format!("/opt/{slug}/resources")),
        PathBuf::from(format!("/usr/lib/{slug}/resources")),
    ]
}

fn user_data_dir(v: &Variant) -> PathBuf {
    let home = dirs::home_dir().unwrap_or_default();
    let name = format!("{}.stable", v.app_id);
    if cfg!(windows) {
        std::env::var("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home.join("AppData").join("Roaming"))
            .join(name)
    } else if cfg!(target_os = "macos") {
        home.join("Library").join("Application Support").join(name)
    } else {
        std::env::var("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home.join(".config"))
            .join(name)
    }
}

fn read_version(resources_dir: &Path) -> Option<String> {
    let manifest = resources_dir.join("build-manifest.json");
    let raw = std::fs::read_to_string(manifest).ok()?;
    serde_json::from_str::<serde_json::Value>(&raw)
        .ok()?
        .get("productVersion")
        .and_then(|v| v.as_str())
        .map(str::to_owned)
}

/// 本机已安装 Qoder 客户端探测(国际版/国内版)。
#[derive(Debug, Clone)]
pub struct QoderAppInfo {
    pub provider: String,
    pub label: String,
    pub installed: bool,
    pub resources_dir: Option<PathBuf>,
    pub version: Option<String>,
    pub runtime_info: Option<PathBuf>,
    pub data_dir: PathBuf,
    pub signed_in: bool,
}

pub fn detect_apps() -> Vec<QoderAppInfo> {
    VARIANTS
        .iter()
        .map(|v| {
            let resources = candidate_resource_dirs(v).into_iter().find(|d| exists(d));
            let exe = if cfg!(windows) { "runtime-info.exe" } else { "runtime-info" };
            let runtime_info = resources
                .as_ref()
                .map(|r| r.join("umid").join(exe))
                .filter(|p| exists(p));
            let data = user_data_dir(v);
            let version = resources.as_deref().and_then(read_version);
            QoderAppInfo {
                provider: v.provider.to_string(),
                label: v.label.to_string(),
                installed: resources.is_some(),
                resources_dir: resources,
                version,
                runtime_info,
                data_dir: data.clone(),
                signed_in: exists(&data.join("auth.v1.dat")),
            }
        })
        .collect()
}

// ─── 客户端身份(machineOs / Hostname / Id / 版本) ───

/// 与客户端一致的 Cosy-MachineOS 格式:x86_64_win32 / aarch64_darwin / x86_64_linux …
pub fn machine_os() -> String {
    let arch = if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "unknown"
    };
    let platform = if cfg!(windows) {
        "win32"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else {
        "linux"
    };
    format!("{arch}_{platform}")
}

/// 与客户端一致的主机名清洗(纯函数):可打印 ASCII 直用过;超长截断并附短哈希;
/// 含非可打印字符时替换为 '-' 并附哈希。
pub fn machine_hostname_from(raw: &str) -> Option<String> {
    fn sha8(t: &str) -> String {
        use sha2::{Digest, Sha256};
        let d = Sha256::digest(t.as_bytes());
        d.iter().take(4).map(|b| format!("{b:02x}")).collect()
    }
    fn clip(t: &str) -> String {
        if t.chars().count() <= 96 {
            return t.to_string();
        }
        let h = sha8(t);
        let head: String = t.chars().take(96 - 8 - 1).collect();
        let head = head.trim_end_matches(['-', ' ']).to_string();
        if head.is_empty() {
            format!("unknown-{h}")
        } else {
            format!("{head}-{h}")
        }
    }
    let e = raw.trim();
    if e.is_empty() {
        return None;
    }
    let printable = |c: char| ('\x21'..='\x7e').contains(&c);
    let chars: Vec<char> = e.chars().collect();
    let ascii_tight = chars.first().is_some_and(|c| printable(*c))
        && chars.last().is_some_and(|c| printable(*c))
        && chars.iter().all(|c| printable(*c) || *c == ' ');
    if ascii_tight {
        return Some(clip(e));
    }
    let h = sha8(e);
    let mut n = String::new();
    for c in chars {
        if printable(c) {
            n.push(c);
        } else if !n.ends_with('-') {
            n.push('-');
        }
    }
    let n = n.trim_matches('-').to_string();
    let fallback = if n.is_empty() { format!("unknown-{h}") } else { format!("{n}-{h}") };
    Some(clip(&fallback))
}

/// 本机主机名(失败回退 None)。
pub fn machine_hostname() -> Option<String> {
    hostname::get().ok()?.into_string().ok().and_then(|h| machine_hostname_from(&h))
}

fn is_uuid_v4ish(id: &str) -> bool {
    // 对应 UUID_RE:8-4-4-4-12,第三组首字符 1-8,第四组首字符 89ab(忽略大小写)
    let id = id.to_ascii_lowercase();
    let parts: Vec<&str> = id.split('-').collect();
    let hex = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_hexdigit());
    parts.len() == 5
        && parts[0].len() == 8 && hex(parts[0])
        && parts[1].len() == 4 && hex(parts[1])
        && parts[2].len() == 4 && hex(parts[2]) && matches!(parts[2].as_bytes()[0], b'1'..=b'8')
        && parts[3].len() == 4 && hex(parts[3]) && matches!(parts[3].as_bytes()[0], b'8' | b'9' | b'a' | b'b')
        && parts[4].len() == 12 && hex(parts[4])
        && id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
}

/// 机器 ID:优先复用客户端的 auth.machine-id,否则在 base_dir 持久化一个稳定 UUID(生成一次)。
pub fn machine_id_for(base_dir: &Path, region: &str) -> String {
    let v = variant_of(region);
    let client_file = user_data_dir(v).join("auth.machine-id");
    for file in [client_file, base_dir.join("machine-id")] {
        if let Ok(id) = std::fs::read_to_string(&file) {
            let id = id.trim();
            if is_uuid_v4ish(id) {
                return id.to_string();
            }
        }
    }
    let id = uuid::Uuid::new_v4().to_string();
    let _ = std::fs::create_dir_all(base_dir);
    let _ = std::fs::write(base_dir.join("machine-id"), &id);
    id
}

/// 客户端版本号(Cosy-Version):优先匹配区域,其次任一已安装版本,缺省 0.2.5。
pub fn client_version(region: &str) -> String {
    let apps = detect_apps();
    let want = provider_of(region);
    apps.iter()
        .find(|a| a.provider == want)
        .and_then(|a| a.version.clone())
        .or_else(|| apps.iter().find_map(|a| a.version.clone()))
        .unwrap_or_else(|| DEFAULT_CLIENT_VERSION.to_string())
}

// ─── 设备风控身份(runtime-info / CLI umid,TTL 缓存) ───

#[derive(Debug, Clone)]
pub struct RiskIdentity {
    pub machine_token: String,
    pub machine_code: String,
    pub machine_type: String,
}

/// CLI 设备身份组件(qoderUmid.js 只在 Linux 下载安装;这里只复用本机已存在的安装,
/// 不做下载,缺失时降级)。目录约定与 CreditDaddy 一致:~/.creditdaddy/qoder-umid/。
fn installed_cli_umid() -> Option<PathBuf> {
    let dir = dirs::home_dir()?.join(".creditdaddy").join("qoder-umid");
    let bin = dir.join(if cfg!(windows) { "runtime-info.exe" } else { "runtime-info" });
    if !exists(&bin) {
        return None;
    }
    let manifest = std::fs::read_to_string(dir.join("manifest.json")).ok()?;
    let m: serde_json::Value = serde_json::from_str(&manifest).ok()?;
    let want_arch = if cfg!(target_arch = "x86_64") { "x64" } else { "arm64" };
    if m.get("arch").and_then(|v| v.as_str()) != Some(want_arch) {
        return None;
    }
    Some(bin)
}

/// 风控身份来源:优先本机 Qoder 客户端 runtime-info(国际版 env=3 / 国内版=0),
/// 其次 CLI 设备身份组件(国际版=4 / 国内版=0)。返回 (exe, env, source)。
pub fn risk_runner(region: &str) -> Option<(PathBuf, i64, &'static str)> {
    let v = variant_of(region);
    let apps: Vec<QoderAppInfo> =
        detect_apps().into_iter().filter(|a| a.runtime_info.is_some()).collect();
    if let Some(app) = apps.iter().find(|a| a.provider == v.provider).or_else(|| apps.last()) {
        let env = if v.provider == "qoder-cn" { APP_ENV_CN } else { APP_ENV_INTL };
        return Some((app.runtime_info.clone()?, env, "app"));
    }
    if let Some(cli) = installed_cli_umid() {
        let env = if v.provider == "qoder-cn" { CLI_ENV_CN } else { CLI_ENV_INTL };
        return Some((cli, env, "cli"));
    }
    None
}

/// 无风控身份时的原因说明(用于 no-activity 提示)。
pub fn risk_unavailable_reason() -> String {
    if cfg!(target_os = "linux") {
        "未安装 Qoder 设备身份组件，无法生成设备风控身份".into()
    } else {
        "本机未安装 Qoder 客户端，无法生成设备风控身份".into()
    }
}

/// 解析 runtime-info 输出(纯函数):首行 JSON,取 machineToken/machineCode/machineType(非空且 ≤4096)。
pub fn parse_risk_output(text: &str) -> Result<RiskIdentity, String> {
    let line = text.lines().find(|l| !l.trim().is_empty()).ok_or("runtime-info 无输出")?;
    let j: serde_json::Value = serde_json::from_str(line.trim()).map_err(|e| format!("runtime-info 输出解析失败: {e}"))?;
    let pick = |k: &str| -> Result<String, String> {
        let s = j
            .get(k)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .unwrap_or("");
        if s.is_empty() || s.len() > MAX_RISK_FIELD {
            return Err(format!("runtime-info 输出缺少 {k}"));
        }
        Ok(s.to_string())
    };
    Ok(RiskIdentity {
        machine_token: pick("machineToken")?,
        machine_code: pick("machineCode")?,
        machine_type: pick("machineType")?,
    })
}

/// 运行 runtime-info 生成风控身份(win32/darwin 通过 stdin 传 {"account":uid},linux 仅 env 参数)。
async fn run_runtime_info(exe: &Path, env: i64, uid: &str) -> Result<RiskIdentity, String> {
    use tokio::io::AsyncWriteExt;
    let with_stdin = cfg!(any(windows, target_os = "macos"));
    let mut cmd = tokio::process::Command::new(exe);
    cmd.arg(env.to_string());
    if with_stdin {
        cmd.arg("--account-stdin");
    }
    cmd.stdin(if with_stdin { std::process::Stdio::piped() } else { std::process::Stdio::null() })
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    let mut child = cmd.spawn().map_err(|e| format!("runtime-info 启动失败：{e}"))?;
    if with_stdin {
        if let Some(mut sin) = child.stdin.take() {
            let _ = sin.write_all(format!("{{\"account\":\"{uid}\"}}\n").as_bytes()).await;
            let _ = sin.shutdown().await;
        }
    }
    let out = tokio::time::timeout(RISK_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| "runtime-info 超时".to_string())?
        .map_err(|e| format!("runtime-info 执行失败：{e}"))?;
    if !out.status.success() && out.stdout.is_empty() {
        return Err(format!("runtime-info 退出码 {}", out.status.code().unwrap_or(-1)));
    }
    parse_risk_output(&String::from_utf8_lossy(&out.stdout))
}

type RiskCache = Mutex<HashMap<String, (RiskIdentity, Instant)>>;

fn risk_cache() -> &'static RiskCache {
    static CACHE: OnceLock<RiskCache> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 获取某账号的设备风控身份(缓存 key = provider:uid,TTL 50min)。
/// Ok(None) = 本机既没有 Qoder 客户端也没装 CLI 组件;Err(reason) = 有组件但运行失败。
pub async fn get_risk_identity_detailed(region: &str, uid: Option<&str>) -> Result<Option<RiskIdentity>, String> {
    let Some(uid) = uid.filter(|u| !u.is_empty()) else {
        return Ok(None);
    };
    let key = format!("{}:{uid}", provider_of(region));
    if let Some((v, at)) = risk_cache().lock().unwrap().get(&key) {
        if at.elapsed() < RISK_TTL {
            return Ok(Some(v.clone()));
        }
    }
    let Some((exe, env, _source)) = risk_runner(region) else {
        return Ok(None);
    };
    let value = run_runtime_info(&exe, env, uid).await?;
    risk_cache().lock().unwrap().insert(key, (value.clone(), Instant::now()));
    Ok(Some(value))
}

/// 便捷版:失败一律 None(风控身份不可用时照常签到,只是国际版可能拿不到每日活动)。
pub async fn get_risk_identity(region: &str, uid: Option<&str>) -> Option<RiskIdentity> {
    get_risk_identity_detailed(region, uid).await.ok().flatten()
}

// ─── 本机客户端凭据(auth.v1.dat,Electron safeStorage) ───

/// 本机 Qoder 客户端当前登录的账号。
#[derive(Debug, Clone)]
pub struct LocalQoderAccount {
    pub provider: String,
    pub source: String,
    pub token: String,
    pub refresh_token: Option<String>,
    pub expires_at: Option<String>,
    pub user_id: Option<String>,
    pub user_name: Option<String>,
    pub user_email: Option<String>,
}

#[cfg(windows)]
fn dpapi_unprotect(data: &[u8]) -> AppResult<Vec<u8>> {
    use windows::Win32::Security::Cryptography::{CryptUnprotectData, CRYPT_INTEGER_BLOB};
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
}

#[cfg(not(windows))]
fn dpapi_unprotect(_data: &[u8]) -> AppResult<Vec<u8>> {
    Err(AppError::Dpapi("DPAPI 仅 Windows 可用".into()))
}

/// AES-256-GCM 解密(密文与 tag 分段,Electron safeStorage 的 "v10" 布局)。
fn safe_storage_decrypt(key: &[u8], data: &[u8]) -> AppResult<String> {
    if key.len() != 32 {
        return Err(AppError::Credential(format!("safeStorage 密钥长度异常: {}", key.len())));
    }
    if data.len() < 3 + 12 + 16 || &data[..3] != b"v10" {
        return Err(AppError::Credential("未知的加密格式(需要 v10)".into()));
    }
    let nonce = &data[3..15];
    let tag = &data[data.len() - 16..];
    let ct = &data[15..data.len() - 16];
    use aes_gcm::aead::{Aead, KeyInit};
    use aes_gcm::{Aes256Gcm, Nonce};
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|e| AppError::Credential(format!("密钥初始化失败: {e}")))?;
    let mut buf = ct.to_vec();
    buf.extend_from_slice(tag);
    let plain = cipher
        .decrypt(Nonce::from_slice(nonce), buf.as_ref())
        .map_err(|_| AppError::Credential("AES-GCM 解密失败(密钥不匹配或数据损坏)".into()))?;
    String::from_utf8(plain).map_err(|e| AppError::Credential(format!("明文不是 UTF-8: {e}")))
}

#[cfg(windows)]
fn read_app_account(app: &QoderAppInfo) -> AppResult<LocalQoderAccount> {
    // 1) Local State → os_crypt.encrypted_key("DPAPI" 前缀 + DPAPI 密文)→ AES 密钥
    let ls_path = app.data_dir.join("Local State");
    let ls: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&ls_path).map_err(|e| AppError::Credential(format!("读取 Local State 失败: {e}")))?,
    )?;
    let enc_key_b64 = ls
        .pointer("/os_crypt/encrypted_key")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AppError::Credential("Local State 中没有 DPAPI 密钥".into()))?;
    let enc = B64.decode(enc_key_b64).map_err(|e| AppError::Credential(format!("encrypted_key base64 解码失败: {e}")))?;
    if enc.len() < 5 || &enc[..5] != b"DPAPI" {
        return Err(AppError::Credential("Local State 中没有 DPAPI 密钥".into()));
    }
    let key = dpapi_unprotect(&enc[5..])?;
    // 2) auth.v1.dat → AES-256-GCM 解密 → JSON
    let data = std::fs::read(app.data_dir.join("auth.v1.dat"))?;
    let plain = safe_storage_decrypt(&key, &data)?;
    let obj: serde_json::Value = serde_json::from_str(&plain)?;
    let token = obj
        .get("token")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::Credential("auth.v1.dat 中没有 token".into()))?
        .to_string();
    Ok(LocalQoderAccount {
        provider: app.provider.clone(),
        source: format!("{} 客户端", app.label),
        token,
        refresh_token: obj.get("refreshToken").and_then(|v| v.as_str()).map(str::to_owned),
        expires_at: obj.get("expiresAt").and_then(|v| v.as_str()).map(str::to_owned),
        user_id: obj.pointer("/user/id").and_then(|v| v.as_str().map(str::to_owned).or_else(|| v.as_i64().map(|i| i.to_string()))),
        user_name: obj.pointer("/user/name").and_then(|v| v.as_str()).map(str::to_owned),
        user_email: obj.pointer("/user/email").and_then(|v| v.as_str()).map(str::to_owned),
    })
}

#[cfg(not(windows))]
fn read_app_account(_app: &QoderAppInfo) -> AppResult<LocalQoderAccount> {
    Err(AppError::Dpapi("本地凭据解密仅 Windows 可用(其他平台请手动录入 token)".into()))
}

/// 读取本机 Qoder 客户端当前登录的账号(解密 auth.v1.dat)。
/// 返回 (成功账号列表, 错误列表)。
pub fn read_app_accounts() -> (Vec<LocalQoderAccount>, Vec<String>) {
    let mut accounts = Vec::new();
    let mut errors = Vec::new();
    for app in detect_apps() {
        if !app.signed_in {
            continue;
        }
        match read_app_account(&app) {
            Ok(a) => accounts.push(a),
            Err(e) => errors.push(format!("[{}] {e}", app.provider)),
        }
    }
    (accounts, errors)
}

/// 从文本中提取 Qoder token(localDetect.js):dt-*/pt-*,去重。
pub fn extract_qoder_tokens(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let is_word = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'-';
    let mut found: Vec<String> = Vec::new();
    let mut i = 0usize;
    while i + 3 <= bytes.len() {
        let prefix = (bytes[i] == b'd' && bytes[i + 1] == b't' && bytes[i + 2] == b'-')
            || (bytes[i] == b'p' && bytes[i + 1] == b't' && bytes[i + 2] == b'-');
        if prefix && (i == 0 || !is_word(bytes[i - 1])) {
            let mut j = i + 3;
            while j < bytes.len() && is_word(bytes[j]) {
                j += 1;
            }
            let mut end = j;
            while end > i + 3 + 16 && !bytes[end - 1].is_ascii_alphanumeric() {
                end -= 1; // 尾部 \b 语义:token 以字母数字结尾
            }
            if end - (i + 3) >= 16 {
                let tok = text[i..end].to_string();
                if !found.contains(&tok) {
                    found.push(tok);
                }
            }
            i = j;
        } else {
            i += 1;
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn machine_os_format() {
        let os = machine_os();
        if cfg!(windows) {
            assert!(os == "x86_64_win32" || os == "aarch64_win32", "实际: {os}");
        }
        assert!(os.contains('_'));
    }

    #[test]
    fn machine_hostname_sanitizes() {
        assert_eq!(machine_hostname_from("MY-PC"), Some("MY-PC".into()));
        assert_eq!(machine_hostname_from("  host name  "), Some("host name".into()));
        assert_eq!(machine_hostname_from(""), None);
        assert_eq!(machine_hostname_from("   "), None);
        // 超长截断 + 短哈希
        let long = "x".repeat(200);
        let clipped = machine_hostname_from(&long).unwrap();
        assert!(clipped.starts_with('x') && clipped.contains('-') && clipped.len() <= 96);
        assert_ne!(clipped, machine_hostname_from(&format!("{long}y")).unwrap(), "不同输入哈希不同");
        // 非可打印字符 → '-' + 哈希
        let weird = machine_hostname_from("电脑-PC").unwrap();
        assert!(weird.starts_with("--PC") || weird.starts_with("PC-") || !weird.contains("电脑"), "实际: {weird}");
        assert!(weird.len() <= 96);
    }

    #[test]
    fn uuid_v4ish_validation() {
        assert!(is_uuid_v4ish("550e8400-e29b-41d4-a716-446655440000"));
        assert!(is_uuid_v4ish("550E8400-E29B-81D4-B716-446655440000"));
        assert!(!is_uuid_v4ish("550e8400-e29b-01d4-a716-446655440000"), "第三组首字符须 1-8");
        assert!(!is_uuid_v4ish("550e8400-e29b-41d4-c716-446655440000"), "第四组首字符须 89ab");
        assert!(!is_uuid_v4ish("not-a-uuid"));
        assert!(!is_uuid_v4ish(""));
        assert!(is_uuid_v4ish(&uuid::Uuid::new_v4().to_string()));
    }

    #[test]
    fn machine_id_is_stable_and_persisted() {
        let dir = std::env::temp_dir().join(format!("credit-core-mid-{}", uuid::Uuid::new_v4()));
        let _ = std::fs::remove_dir_all(&dir);
        let a = machine_id_for(&dir, "intl");
        let b = machine_id_for(&dir, "intl");
        assert_eq!(a, b, "同一 base_dir 生成一次后稳定");
        assert!(is_uuid_v4ish(&a));
        assert!(dir.join("machine-id").exists(), "兜底 UUID 落盘");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_risk_output_variants() {
        let ok = parse_risk_output("{\"machineToken\":\"tk\",\"machineCode\":\"cd\",\"machineType\":\"tp\"}\ntrailing\n").unwrap();
        assert_eq!(ok.machine_token, "tk");
        assert_eq!(ok.machine_code, "cd");
        assert_eq!(ok.machine_type, "tp");
        assert!(parse_risk_output("").is_err());
        assert!(parse_risk_output("not json").is_err());
        assert!(parse_risk_output("{\"machineToken\":\"\"}").is_err(), "空字段视为缺失");
    }

    #[test]
    fn risk_runner_env_mapping() {
        // 有无安装都应保持 env 取值约定(找不到 runner 返回 None)
        if let Some((_, env, source)) = risk_runner("intl") {
            assert!(env == APP_ENV_INTL || (source == "cli" && env == CLI_ENV_INTL), "env={env} source={source}");
        }
        if let Some((_, env, source)) = risk_runner("cn") {
            assert!(env == APP_ENV_CN || (source == "cli" && env == CLI_ENV_CN), "env={env} source={source}");
        }
    }

    #[test]
    fn extract_tokens_dt_pt() {
        let text = "前缀 dt-Abcdefgh1234567890 结尾 pt-1234567890abcdefgh ,bad nodt-Abcdefgh1234567890, 重复 dt-Abcdefgh1234567890";
        let toks = extract_qoder_tokens(text);
        assert_eq!(toks, vec!["dt-Abcdefgh1234567890", "pt-1234567890abcdefgh"]);
        assert!(extract_qoder_tokens("").is_empty());
        assert!(extract_qoder_tokens("dt-short").is_empty(), "过短不是 token");
        assert!(extract_qoder_tokens("xdt-Abcdefgh1234567890").is_empty(), "前缀前有词字符不算");
    }
}
