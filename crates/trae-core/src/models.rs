// 数据模型,镜像 electron/types.ts。serde camelCase 与前端字段对齐。
// 新数据模型(device_map/cooldown/credits 等)移植自参考项目 models.rs。

use serde::{Deserialize, Serialize};

/// 账号来源:desktop = 从 TRAE 桌面实例导入(DPAPI 加密完整凭据 + 多开);
/// jwt = 手动录入的 JWT + refresh_token(参考模式,伪设备 ID + ExchangeToken 自动刷新)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum AccountSource {
    #[default]
    Desktop,
    Jwt,
}

/// 账号(完整,含加密凭据,仅用于本地存储与内部逻辑)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub cookie: String,
    pub created_at: i64,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub last_checkin_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub last_checkin_result: Option<String>, // "success" | "failed" | "pending"
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub last_checkin_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub points: Option<i64>,
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub desktop_user_id: Option<String>,
    /// DPAPI 加密后的 Credential JSON(base64)。仅在本地存储中流转。
    #[serde(default)]
    pub encrypted_credential: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub credential_status: Option<String>, // "valid" | "expiring" | "expired"
    /// 多开实例的独立 data-dir 路径(%APPDATA%\TRAE SOLO CN_{userId}),首次多开时生成并回写
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub data_dir: Option<String>,
    /// 多开实例的机器码(每实例独立 UUID,不动系统注册表)
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub machine_id: Option<String>,
    // ===== 参考模式新增字段(jwt 账号 / 展示态,全部 default 保证旧数据兼容) =====
    #[serde(default)]
    pub source: AccountSource,
    /// jwt 账号的 user_id(desktop 账号沿用 desktop_user_id)
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub user_id: Option<String>,
    /// DPAPI 加密的 JwtCredential JSON(base64)。仅在本地存储中流转。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub encrypted_jwt: Option<String>,
    // —— 以下为展示态,由 get_accounts 从各 JSON 合并填充,持久化便于前端直接读 ——
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub jwt_exp_timestamp: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub jwt_status: Option<String>, // "ok" | "warn" | "expired" | "unknown"
    #[serde(default)]
    pub has_refresh_token: bool,
    #[serde(default)]
    pub jwt_auto_refresh: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub remaining_credits: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub credits_expire_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub device_id_masked: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cooldown_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cooldown_until: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cooldown_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub checked_today: Option<bool>,
}

/// 账号(脱敏,返回前端)。不含加密凭据。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicAccount {
    pub id: String,
    pub name: String,
    pub cookie: String,
    pub created_at: i64,
    pub last_checkin_at: Option<i64>,
    pub last_checkin_result: Option<String>,
    pub last_checkin_message: Option<String>,
    pub points: Option<i64>,
    pub enabled: bool,
    pub desktop_user_id: Option<String>,
    pub credential_status: Option<String>,
    pub data_dir: Option<String>,
    pub machine_id: Option<String>,
    pub source: AccountSource,
    pub user_id: Option<String>,
    pub jwt_exp_timestamp: Option<i64>,
    pub jwt_status: Option<String>,
    pub has_refresh_token: bool,
    pub jwt_auto_refresh: bool,
    pub remaining_credits: Option<f64>,
    pub credits_expire_at: Option<i64>,
    pub device_id_masked: Option<String>,
    pub cooldown_type: Option<String>,
    pub cooldown_until: Option<i64>,
    pub cooldown_reason: Option<String>,
    pub checked_today: Option<bool>,
}

impl From<Account> for PublicAccount {
    fn from(a: Account) -> Self {
        Self {
            id: a.id,
            name: a.name,
            cookie: a.cookie,
            created_at: a.created_at,
            last_checkin_at: a.last_checkin_at,
            last_checkin_result: a.last_checkin_result,
            last_checkin_message: a.last_checkin_message,
            points: a.points,
            enabled: a.enabled,
            desktop_user_id: a.desktop_user_id,
            credential_status: a.credential_status,
            data_dir: a.data_dir,
            machine_id: a.machine_id,
            source: a.source,
            user_id: a.user_id,
            jwt_exp_timestamp: a.jwt_exp_timestamp,
            jwt_status: a.jwt_status,
            has_refresh_token: a.has_refresh_token,
            jwt_auto_refresh: a.jwt_auto_refresh,
            remaining_credits: a.remaining_credits,
            credits_expire_at: a.credits_expire_at,
            device_id_masked: a.device_id_masked,
            cooldown_type: a.cooldown_type,
            cooldown_until: a.cooldown_until,
            cooldown_reason: a.cooldown_reason,
            checked_today: a.checked_today,
        }
    }
}

/// 签到日志
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckinLog {
    pub id: String,
    pub account_id: String,
    pub account_name: String,
    pub time: i64,
    pub result: String, // "success" | "failed"
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub points_gained: Option<i64>,
}

/// 应用设置(仅 desktop 模式相关字段)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub auto_checkin: bool,
    pub checkin_time: String, // "HH:mm"
    pub retry_count: u32,
    pub retry_delay: u32, // 秒
    /// 自动签到(定时)时账号间的执行间隔(分钟),最短 3
    #[serde(default = "default_checkin_interval")]
    pub auto_checkin_interval_min: u32,
    pub notify_on_success: bool,
    pub notify_on_failed: bool,
    pub launch_at_login: bool,
}

fn default_checkin_interval() -> u32 {
    15
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            auto_checkin: false,
            checkin_time: "08:00".into(),
            retry_count: 3,
            retry_delay: 60,
            auto_checkin_interval_min: default_checkin_interval(),
            notify_on_success: true,
            notify_on_failed: true,
            launch_at_login: false,
        }
    }
}

/// 设置的部分更新(前端 saveSettings 传 partial)
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PartialAppSettings {
    #[serde(default)]
    pub auto_checkin: Option<bool>,
    #[serde(default)]
    pub checkin_time: Option<String>,
    #[serde(default)]
    pub retry_count: Option<u32>,
    #[serde(default)]
    pub retry_delay: Option<u32>,
    #[serde(default)]
    pub auto_checkin_interval_min: Option<u32>,
    #[serde(default)]
    pub notify_on_success: Option<bool>,
    #[serde(default)]
    pub notify_on_failed: Option<bool>,
    #[serde(default)]
    pub launch_at_login: Option<bool>,
}

impl AppSettings {
    /// 用 partial 覆盖现有设置
    pub fn merge(&mut self, p: PartialAppSettings) {
        if let Some(v) = p.auto_checkin {
            self.auto_checkin = v;
        }
        if let Some(v) = p.checkin_time {
            self.checkin_time = v;
        }
        if let Some(v) = p.retry_count {
            self.retry_count = v;
        }
        if let Some(v) = p.retry_delay {
            self.retry_delay = v;
        }
        if let Some(v) = p.auto_checkin_interval_min {
            // 最短 3 分钟
            self.auto_checkin_interval_min = v.max(3);
        }
        if let Some(v) = p.notify_on_success {
            self.notify_on_success = v;
        }
        if let Some(v) = p.notify_on_failed {
            self.notify_on_failed = v;
        }
        if let Some(v) = p.launch_at_login {
            self.launch_at_login = v;
        }
    }
}

/// TRAE 桌面凭证(内部,不序列化到前端)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Credential {
    pub token: String,
    pub refresh_token: String,
    pub expires_at: i64, // 毫秒时间戳
    pub refresh_expires_at: i64,
    pub device_id: String,
    pub machine_id: String,
    pub private_key_pem: String,
    pub public_key_pem: String,
    pub user_id: String,
    pub account_name: String,
    pub host: String,
    /// 账号邮箱(多开写 storage.json 用,从桌面凭据 account 提取)
    #[serde(default)]
    pub email: Option<String>,
    /// 头像 URL(多开写 storage.json 用)
    #[serde(default)]
    pub avatar_url: Option<String>,
    /// 区域("CN"/"SG",多开写 storage.json 用,从 userRegion 或 host 推断)
    #[serde(default)]
    pub region: Option<String>,
}

impl Credential {
    /// 空占位凭据:宽松回读目录登录信息时的兜底(缺失字段补空值)
    pub fn empty() -> Self {
        Self {
            token: String::new(),
            refresh_token: String::new(),
            expires_at: 0,
            refresh_expires_at: 0,
            device_id: String::new(),
            machine_id: String::new(),
            private_key_pem: String::new(),
            public_key_pem: String::new(),
            user_id: String::new(),
            account_name: String::new(),
            host: String::new(),
            email: None,
            avatar_url: None,
            region: None,
        }
    }
}

/// JWT 账号的凭据(内部,不序列化到前端)。DPAPI 加密后存 Account.encrypted_jwt。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JwtCredential {
    pub jwt: String, // 完整值(可能带 "Cloud-IDE-JWT " 前缀)
    #[serde(default)]
    pub refresh_token: Option<String>,
}

/// 签到结果
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckinResult {
    pub success: bool,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub points: Option<i64>,
}

/// 积分查询结果
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PointsResult {
    pub success: bool,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_points: Option<i64>,
}

/// 多开启动结果
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchResult {
    pub data_dir: String,
    pub machine_id: String,
    pub launched: bool,
}

/// 凭证状态
pub fn credential_status(expires_at: i64, now_ms: i64) -> &'static str {
    if expires_at <= now_ms {
        "expired"
    } else if expires_at - now_ms <= 15 * 60 * 1000 {
        "expiring"
    } else {
        "valid"
    }
}

// ===== 参考式新数据模型(device_map / cooldowns / credits) =====

use std::collections::HashMap;

/// 单个账号的稳定伪设备身份(device_map.json 条目)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceEntry {
    pub device_id: String,        // 15 位数字
    pub market_user_id: String,   // UUIDv4
    pub session_id: String,       // 64 位 hex
    #[serde(default)]
    pub created: String,
    #[serde(default = "default_device_gen")]
    pub gen: u32,
}

fn default_device_gen() -> u32 {
    2
}

pub type DeviceMap = HashMap<String, DeviceEntry>;

/// 单个账号的冷却状态(account_cooldowns.json 条目)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CooldownEntry {
    #[serde(rename = "type", default)]
    pub error_type: String, // PlanLimit | SoftRate | SessionDead | NotFound | Server | Client | BusinessError | Unknown
    #[serde(default)]
    pub until: i64, // Unix 秒;SessionDead=9999999999 表示永久
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub error_count: i32,
}

/// 冷却状态文件:account_cooldowns.json
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AccountCooldownsFile {
    #[serde(default)]
    pub cooldowns: HashMap<String, CooldownEntry>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

/// 单条积分历史(credits_history.json)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CreditRecord {
    pub date: String, // "YYYY-MM-DD"
    pub user_id: String,
    pub credits: i64,
    #[serde(default)]
    pub delta: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CreditsFile {
    pub records: Vec<CreditRecord>,
}

/// 每日积分快照(credits_daily.json,积分看板三线趋势数据源)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CreditsDailySnapshot {
    pub date: String,
    pub total: f64,
    pub earned: f64,
    pub consumed: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CreditsDailyFile {
    #[serde(default)]
    pub snapshots: Vec<CreditsDailySnapshot>,
}

/// 最近一次签到批次摘要(checkin_summary.json,供 checked_today 判定)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CheckinSummary {
    #[serde(default)]
    pub time: Option<String>,
    #[serde(default)]
    pub results: Vec<serde_json::Value>,
    #[serde(default)]
    pub total_ok: i32,
    #[serde(default)]
    pub already: i32,
    #[serde(default)]
    pub failed: i32,
}

/// 剩余积分缓存(remaining_credits.json):user_id -> 剩余积分 / 最早过期时间
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RemainingCreditsFile {
    #[serde(default)]
    pub credits: HashMap<String, f64>,
    #[serde(default)]
    pub expire_times: HashMap<String, i64>,
    #[serde(default)]
    pub updated_at: Option<String>,
}
