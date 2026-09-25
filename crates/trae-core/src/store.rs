// 本地数据存储:账号、日志、设置。持久化为 JSON(app_data_dir),原子写(临时文件+rename)。

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::error::{AppError, AppResult};
use crate::models::{Account, AppSettings, CheckinLog, PartialAppSettings};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct StoreData {
    #[serde(default)]
    pub accounts: Vec<Account>,
    #[serde(default)]
    pub logs: Vec<CheckinLog>,
    #[serde(default)]
    pub settings: AppSettings,
}

/// 应用全局状态:数据 + 存储根目录。
/// base_dir 由宿主传入(workbuddy 数据根 `~/.wb-switch/trae/`),主存储与其 data/ 子目录都在其下。
pub struct TraeState {
    pub data: Mutex<StoreData>,
    /// 数据根目录(~/.wb-switch/trae/)
    pub base_dir: PathBuf,
    /// JWT 刷新锁:防止多个并发请求同时 ExchangeToken(用 tokio Mutex,持锁可跨 await)
    pub jwt_refresh_lock: tokio::sync::Mutex<()>,
}

impl TraeState {
    /// 主存储文件(trae-check-data.json,与旧 TraeMate 同名便于迁移)。
    pub fn store_file(&self) -> PathBuf {
        self.base_dir.join("trae-check-data.json")
    }

    /// 参考式 JSON 数据文件路径:base_dir/data/{name},自动建目录。
    pub fn data_path(&self, name: &str) -> PathBuf {
        let dir = self.base_dir.join("data");
        let _ = std::fs::create_dir_all(&dir);
        dir.join(name)
    }

    /// 从 base_dir 加载数据构建状态(文件不存在则用默认值)。
    pub fn new(base_dir: PathBuf) -> Self {
        let _ = std::fs::create_dir_all(&base_dir);
        let store_file = base_dir.join("trae-check-data.json");
        let data = StoreData::load(&store_file).unwrap_or_default();
        Self {
            data: Mutex::new(data),
            base_dir,
            jwt_refresh_lock: tokio::sync::Mutex::new(()),
        }
    }

    /// 持久化当前数据到主存储文件。
    pub fn save(&self) -> AppResult<()> {
        let data = self.data.lock().unwrap();
        data.save(&self.store_file())
    }
}

/// 账号的有效 user_id:desktop 用 desktop_user_id,jwt 用 user_id。
/// 用于 device_map / cooldown / credits 等按用户维度的数据文件索引。
pub fn effective_user_id(a: &Account) -> Option<&str> {
    match a.source {
        crate::models::AccountSource::Jwt => a.user_id.as_deref(),
        crate::models::AccountSource::Desktop => a.desktop_user_id.as_deref(),
    }
    .filter(|s| !s.is_empty())
}

impl StoreData {
    pub fn load(path: &Path) -> AppResult<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = std::fs::read_to_string(path)?;
        if raw.trim().is_empty() {
            return Ok(Self::default());
        }
        let store: StoreData = serde_json::from_str(&raw)?;
        Ok(store)
    }

    pub fn save(&self, path: &Path) -> AppResult<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    pub fn get_accounts(&self) -> &[Account] {
        &self.accounts
    }

    /// 按 desktopUserId upsert:匹配则更新(保留原 id),否则新增。对应原 upsertDesktopAccount。
    pub fn upsert_desktop_account(&mut self, mut incoming: Account) -> Account {
        if let Some(i) = self
            .accounts
            .iter()
            .position(|a| a.desktop_user_id == incoming.desktop_user_id)
        {
            incoming.id = self.accounts[i].id.clone();
            self.accounts[i] = incoming.clone();
            incoming
        } else {
            self.accounts.push(incoming.clone());
            incoming
        }
    }

    pub fn delete_account(&mut self, id: &str) {
        self.accounts.retain(|a| a.id != id);
    }

    /// 用 JSON 对象合并更新账号字段(前端传 partial,任意字段)。
    pub fn update_account(&mut self, id: &str, updates: serde_json::Value) -> Option<Account> {
        let i = self.accounts.iter().position(|a| a.id == id)?;
        let mut cur = serde_json::to_value(&self.accounts[i]).ok()?;
        if let (Some(obj), Some(upd)) = (cur.as_object_mut(), updates.as_object()) {
            for (k, v) in upd {
                obj.insert(k.clone(), v.clone());
            }
        }
        let merged: Account = serde_json::from_value(cur).ok()?;
        self.accounts[i] = merged.clone();
        Some(merged)
    }

    /// 返回最近 limit 条日志,最新在前(对应原 slice(-limit).reverse())
    pub fn get_logs(&self, limit: usize) -> Vec<CheckinLog> {
        let n = self.logs.len();
        let start = n.saturating_sub(limit);
        self.logs[start..].iter().rev().cloned().collect()
    }

    pub fn add_log(&mut self, log: CheckinLog) {
        self.logs.push(log);
        if self.logs.len() > 500 {
            let drop_n = self.logs.len() - 500;
            self.logs.drain(0..drop_n);
        }
    }

    pub fn clear_logs(&mut self) {
        self.logs.clear();
    }

    pub fn get_settings(&self) -> AppSettings {
        self.settings.clone()
    }

    pub fn save_settings(&mut self, partial: PartialAppSettings) -> AppSettings {
        self.settings.merge(partial);
        self.settings.clone()
    }
}

/// 生成简单唯一 ID(对应原 Date.now().toString(36) + random)
pub fn generate_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    // 用进程内计数器补充随机性(Math.random 替代)
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{:x}-{}", ms, n)
}

// 显式标注:TraeState 内部用 Mutex,AppError 路径错误也归为 IO
#[allow(dead_code)]
fn _unused(_e: AppError) {}
