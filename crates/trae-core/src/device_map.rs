//! 稳定伪设备身份派生(device_map.json)。移植自参考项目 device_proxy.py 的设备算法。
//!
//! 同一 user_id 永远派生同一套设备标识(x-device-id / x-market-user-id / vscode-sessionid),
//! 使多账号签到各自携带独立设备身份,规避服务端"每设备每天一次"配额。
//! 关键:seed 用**原始 user_id 字符串**(参考的 _normalize_seed 只服务已废弃的 random.Random 兼容路径,勿用)。

use std::path::PathBuf;

use sha2::{Digest, Sha256};

use crate::fs_utils;
use crate::models::{DeviceEntry, DeviceMap};

/// 设备标识生成算法版本;旧记录(gen 缺失=1)自动重建
pub const DEVICE_GEN: u32 = 2;

/// 确定性派生均匀字节流(SHA-256),避免 random.Random(seed) 的病态序列。
fn seeded_stream(seed: &str, salt: &str, nbytes: usize) -> Vec<u8> {
    let data = format!("{salt}:{seed}");
    let mut out = Vec::with_capacity(nbytes);
    let mut i: u32 = 0;
    while out.len() < nbytes {
        let mut hasher = Sha256::new();
        hasher.update(data.as_bytes());
        hasher.update(i.to_be_bytes());
        out.extend_from_slice(&hasher.finalize());
        i += 1;
    }
    out.truncate(nbytes);
    out
}

/// 15 位数字设备 id(salt="devid")
fn rand_digits(n: usize, seed: &str) -> String {
    let bs = seeded_stream(seed, "devid", n + 1);
    bs[..n].iter().map(|b| (b % 10).to_string()).collect()
}

/// n 位 hex(salt="sess")
fn rand_hex(n: usize, seed: &str) -> String {
    let need = (n + 1) / 2;
    let bs = seeded_stream(seed, "sess", need);
    let mut s = String::with_capacity(n);
    for b in &bs {
        s.push_str(&format!("{b:02x}"));
    }
    s.truncate(n);
    s
}

/// 标准 UUID v4(确定性派生,salt="market")
fn gen_market_uuid(seed: &str) -> String {
    let mut bs = seeded_stream(seed, "market", 16);
    bs[6] = (bs[6] & 0x0F) | 0x40; // version 4
    bs[8] = (bs[8] & 0x3F) | 0x80; // variant RFC 4122
    let hex: String = bs.iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

fn device_path(state: &crate::store::TraeState) -> PathBuf {
    state.data_path("device_map.json")
}

/// 读取 device_map.json(缺/坏回退空)。
pub fn load_device_map(state: &crate::store::TraeState) -> DeviceMap {
    fs_utils::read_json(&device_path(state))
}

/// 获取(必要时生成并持久化)该 user_id 的稳定设备身份。
pub fn get_device_for(state: &crate::store::TraeState, user_id: &str) -> DeviceEntry {
    let mut map: DeviceMap = load_device_map(state);
    let existing = map.get(user_id);
    let needs_rebuild = existing
        .map(|e| e.gen < DEVICE_GEN)
        .unwrap_or(true);
    if needs_rebuild {
        let entry = DeviceEntry {
            device_id: rand_digits(15, user_id),
            market_user_id: gen_market_uuid(user_id),
            session_id: rand_hex(64, user_id),
            created: fs_utils::now_iso(),
            gen: DEVICE_GEN,
        };
        map.insert(user_id.to_string(), entry.clone());
        let _ = fs_utils::write_json(&device_path(state), &map);
        entry
    } else {
        existing.unwrap().clone()
    }
}

/// 删除某 user_id 的设备记录(device_reset / 删除账号联动)。
pub fn reset_device_for(state: &crate::store::TraeState, user_id: &str) {
    let mut map: DeviceMap = load_device_map(state);
    if map.remove(user_id).is_some() {
        let _ = fs_utils::write_json(&device_path(state), &map);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 与参考项目 Python 算法(device_proxy.py / auto_checkin.py)输出比对。
    /// 参考值由 Python 脚本按同一算法计算,见 .reference/auto-checkin-hub/src-python/auto_checkin.py。
    #[test]
    fn derive_matches_python_reference() {
        let uid = "1234567890123456";
        // Python: rand_digits(15, seed="1234567890123456")
        assert_eq!(rand_digits(15, uid), "413174708280782");
        // Python: rand_hex(64, seed=uid)
        assert_eq!(
            rand_hex(64, uid),
            "fbabf7aa1e173b90385c623e1ec49157860cc07a4b01f53f7ca141b17d876eae"
        );
        // Python: gen_market_uuid(seed=uid)
        assert_eq!(gen_market_uuid(uid), "746608f9-7f37-4960-b4c7-8553cec6d366");

        // 第二组 uid 交叉验证
        let uid2 = "9876543210";
        assert_eq!(rand_digits(15, uid2), "630512735296035");
        assert_eq!(
            rand_hex(64, uid2),
            "78e8868abb83fdf82f3852e9563b07184ef8deeb462568d852affd8d59a4bbd9"
        );
        assert_eq!(gen_market_uuid(uid2), "c2aec55c-c9bb-4f68-b068-b8fa183b958b");

        // 自洽:同 seed 稳定、字符集正确、UUID v4 位
        assert_eq!(rand_digits(15, uid), rand_digits(15, uid));
        let u1 = gen_market_uuid(uid);
        assert_eq!(&u1[14..15], "4");
        assert!(matches!(&u1[19..20], "8" | "9" | "a" | "b"));
    }
}
