// 通用信用平台本地存储:账号、日志、设置。完全仿 TraeState 的 Mutex + 原子写模式。
// 数据目录:~/.wb-switch/qoder/ 与 ~/.wb-switch/zcode/(base_dir 由宿主传入或用默认)。

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::de::DeserializeOwned;

use crate::error::{AppError, AppResult};
use crate::log::LogEntry;
use crate::models::{
    CreditSettings, PartialSettings, QoderAccount, QoderSettings, ZCodeAccount, ZcodeSettings,
};

/// 主存储数据(单文件 JSON)。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CreditStoreData<A, S> {
    // 注意:此处不加 #[serde(default)]——它会让 serde 为 A 追加 Default 约束;
    // 文件缺失/损坏时由 CreditState::new 的 unwrap_or_else(empty) 兜底。
    pub accounts: Vec<A>,
    #[serde(default)]
    pub logs: Vec<LogEntry>,
    #[serde(default)]
    pub settings: S,
}

/// 应用全局状态:数据 + 存储根目录。泛型化为 QoderState / ZcodeState。
pub struct CreditState<A, S> {
    pub data: Mutex<CreditStoreData<A, S>>,
    /// 数据根目录(如 ~/.wb-switch/qoder/)
    pub base_dir: PathBuf,
    store_name: String,
}

impl<A, S> CreditState<A, S>
where
    A: Clone + serde::Serialize + DeserializeOwned,
    S: CreditSettings,
{
    /// 从 base_dir 加载数据构建状态(文件不存在则用默认值),目录自动创建。
    pub fn new(base_dir: PathBuf, store_name: &str) -> Self {
        let _ = std::fs::create_dir_all(&base_dir);
        let store_file = base_dir.join(store_name);
        let data = CreditStoreData::load(&store_file).unwrap_or_else(|_| CreditStoreData::empty());
        Self {
            data: Mutex::new(data),
            base_dir,
            store_name: store_name.into(),
        }
    }

    /// 主存储文件路径(base_dir/{store_name})。
    pub fn store_file(&self) -> PathBuf {
        self.base_dir.join(&self.store_name)
    }

    /// 参考式 JSON 数据文件路径:base_dir/data/{name},自动建目录。
    pub fn data_path(&self, name: &str) -> PathBuf {
        let dir = self.base_dir.join("data");
        let _ = std::fs::create_dir_all(&dir);
        dir.join(name)
    }

    /// 持久化当前数据到主存储文件。
    pub fn save(&self) -> AppResult<()> {
        let data = self.data.lock().unwrap();
        data.save(&self.store_file())
    }
}

impl<A, S> CreditStoreData<A, S>
where
    A: Clone + serde::Serialize + DeserializeOwned,
    S: CreditSettings,
{
    /// 空数据(文件缺失/损坏时的兜底,不依赖 A: Default)。
    pub fn empty() -> Self {
        CreditStoreData {
            accounts: Vec::new(),
            logs: Vec::new(),
            settings: S::default(),
        }
    }

    pub fn load(path: &Path) -> AppResult<Self> {
        if !path.exists() {
            return Ok(Self::empty());
        }
        let raw = std::fs::read_to_string(path)?;
        if raw.trim().is_empty() {
            return Ok(Self::empty());
        }
        let store: Self = serde_json::from_str(&raw)?;
        Ok(store)
    }

    /// 原子写:临时文件 + rename(与 trae-core 同款)。
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

    pub fn get_accounts(&self) -> &[A] {
        &self.accounts
    }

    pub fn delete_account(&mut self, id: &str) {
        self.accounts.retain(|a| account_id(a) != id);
    }

    /// 用 JSON 对象合并更新账号字段(前端传 partial,任意字段)。找不到返回 None。
    pub fn update_account(&mut self, id: &str, updates: serde_json::Value) -> Option<A> {
        let i = self.accounts.iter().position(|a| account_id(a) == id)?;
        let mut cur = serde_json::to_value(&self.accounts[i]).ok()?;
        if let (Some(obj), Some(upd)) = (cur.as_object_mut(), updates.as_object()) {
            for (k, v) in upd {
                obj.insert(k.clone(), v.clone());
            }
        }
        let merged: A = serde_json::from_value(cur).ok()?;
        self.accounts[i] = merged.clone();
        Some(merged)
    }

    pub fn get_settings(&self) -> S {
        self.settings.clone()
    }

    pub fn save_settings(&mut self, partial: PartialSettings) -> S {
        self.settings.merge_partial(&partial);
        self.settings.clone()
    }
}

/// 账号 id 抽取(泛型下通过 serde 取 "id" 字段,模型契约保证存在)。
fn account_id<A: serde::Serialize>(a: &A) -> String {
    serde_json::to_value(a)
        .ok()
        .and_then(|v| v.get("id").and_then(|x| x.as_str().map(str::to_owned)))
        .unwrap_or_default()
}

// ===== 平台别名与默认目录 =====

pub type QoderState = CreditState<QoderAccount, QoderSettings>;
pub type ZcodeState = CreditState<ZCodeAccount, ZcodeSettings>;

pub const QODER_STORE_NAME: &str = "qoder-store.json";
pub const ZCODE_STORE_NAME: &str = "zcode-store.json";

/// 默认数据根:~/.wb-switch/{platform}/
pub fn default_base_dir(platform_dir: &str) -> PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    home.join(".wb-switch").join(platform_dir)
}

pub fn qoder_base_dir() -> PathBuf {
    default_base_dir("qoder")
}

pub fn zcode_base_dir() -> PathBuf {
    default_base_dir("zcode")
}

/// 打开默认位置的 Qoder 状态(~/.wb-switch/qoder/)。
pub fn open_qoder_state() -> QoderState {
    QoderState::new(qoder_base_dir(), QODER_STORE_NAME)
}

/// 打开指定目录的 Qoder 状态(测试/自定义宿主用)。
pub fn open_qoder_state_at(base_dir: PathBuf) -> QoderState {
    QoderState::new(base_dir, QODER_STORE_NAME)
}

/// 打开默认位置的 ZCode 状态(~/.wb-switch/zcode/)。
pub fn open_zcode_state() -> ZcodeState {
    ZcodeState::new(zcode_base_dir(), ZCODE_STORE_NAME)
}

/// 打开指定目录的 ZCode 状态(测试/自定义宿主用)。
pub fn open_zcode_state_at(base_dir: PathBuf) -> ZcodeState {
    ZcodeState::new(base_dir, ZCODE_STORE_NAME)
}

/// 生成简单唯一 ID(同 trae-core:毫秒时间戳 + 进程内计数)。
pub fn generate_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{:x}-{}", ms, n)
}

#[allow(dead_code)]
fn _unused(e: AppError) -> AppError {
    e
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log::{LogStore, PLATFORM_QODER};
    use crate::models::ClaimOutcome;

    fn tmp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("credit-core-test-{tag}-{}", uuid::Uuid::new_v4()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn store_save_load_roundtrip() {
        let dir = tmp_dir("roundtrip");
        {
            let state = open_qoder_state_at(dir.clone());
            let mut data = state.data.lock().unwrap();
            data.accounts.push(QoderAccount {
                id: "a1".into(),
                name: "测试账号".into(),
                token: "dt-token".into(),
                region: "cn".into(),
                enabled: true,
                user_id: Some("uid1".into()),
                ..Default::default()
            });
            data.append_log(LogEntry::new("a1", "测试账号", PLATFORM_QODER, "checked-in", "领取成功"));
            data.settings.auto_claim_enabled = true;
            data.save(&state.store_file()).unwrap();
        }
        // 重新加载
        let state = open_qoder_state_at(dir.clone());
        let data = state.data.lock().unwrap();
        assert_eq!(data.accounts.len(), 1);
        assert_eq!(data.accounts[0].id, "a1");
        assert_eq!(data.accounts[0].region, "cn");
        assert_eq!(data.logs.len(), 1);
        assert!(data.settings.auto_claim_enabled);
        assert!(state.store_file().ends_with(QODER_STORE_NAME));
        drop(data);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn store_update_and_delete_account() {
        let dir = tmp_dir("updel");
        let state = open_qoder_state_at(dir.clone());
        {
            let mut data = state.data.lock().unwrap();
            data.accounts.push(QoderAccount {
                id: "a1".into(),
                name: "n".into(),
                enabled: true,
                ..Default::default()
            });
            let updated = data
                .update_account("a1", serde_json::json!({"token": "dt-new", "lastResult": "already"}))
                .unwrap();
            assert_eq!(updated.token, "dt-new");
            assert_eq!(updated.last_result.as_deref(), Some("already"));
            assert_eq!(updated.name, "n", "未更新的字段保留");
            assert!(data.update_account("missing", serde_json::json!({"x": 1})).is_none());
            data.delete_account("a1");
            assert!(data.get_accounts().is_empty());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn store_settings_partial_and_zcode_state() {
        let dir = tmp_dir("settings");
        let state = open_zcode_state_at(dir.clone());
        {
            let mut data = state.data.lock().unwrap();
            data.accounts.push(ZCodeAccount {
                id: "z1".into(),
                name: "zn".into(),
                enabled: true,
                ..Default::default()
            });
            let s = data.save_settings(PartialSettings {
                auto_claim_enabled: Some(true),
                interval_min: Some(200),
                region: Some("cn".into()),
            });
            assert!(s.auto_claim_enabled);
            assert_eq!(s.interval_min, 200);
            assert_eq!(
                serde_json::to_value(&s).unwrap().get("region"),
                None,
                "ZCode 设置无 region 字段"
            );
        }
        // 持久化往返
        state.save().unwrap();
        let reloaded = open_zcode_state_at(dir.clone());
        let data = reloaded.data.lock().unwrap();
        assert_eq!(data.accounts[0].id, "z1");
        assert!(data.settings.auto_claim_enabled);
        drop(data);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn store_empty_or_missing_file_defaults() {
        let dir = tmp_dir("missing");
        let state = open_qoder_state_at(dir.clone());
        assert!(state.data.lock().unwrap().get_accounts().is_empty());
        // 空文件
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(state.store_file(), "  \n").unwrap();
        let state2 = open_qoder_state_at(dir.clone());
        assert_eq!(state2.data.lock().unwrap().settings.region, "intl");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn claim_outcome_state_machine_mapping() {
        // ZCode 业务码 → ClaimOutcome(claim.rs 的 map_claim_code 委托这里;集中验证)
        use crate::zcode::client::map_claim_code;
        assert_eq!(map_claim_code(1003), ClaimOutcome::Already);
        assert_eq!(map_claim_code(3007), ClaimOutcome::NeedCaptcha);
        assert_eq!(map_claim_code(3001), ClaimOutcome::NeedCaptcha);
        assert_eq!(map_claim_code(1001), ClaimOutcome::Failed);
        assert_eq!(map_claim_code(1002), ClaimOutcome::Failed);
        assert_eq!(map_claim_code(1004), ClaimOutcome::Failed);
        assert_eq!(map_claim_code(1005), ClaimOutcome::Failed);
        assert_eq!(map_claim_code(401), ClaimOutcome::Failed);
        assert_eq!(map_claim_code(9999), ClaimOutcome::Failed);
    }
}
