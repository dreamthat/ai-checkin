//! TRAE 签到/多开核心逻辑（自 trae-mate 平移）。
//! 不依赖 Tauri：桌面宿主（src-tauri）与 HTTP server（wb-switch-server）共用。
//! 数据根目录由宿主传入（`~/.wb-switch/trae/`），TRAE 功能仅 Windows 可用。

pub mod accounts;
pub mod checkin;
pub mod cooldown;
pub mod credentials;
pub mod credits;
pub mod device_map;
pub mod error;
pub mod fs_utils;
pub mod jwt;
pub mod migrate;
pub mod models;
pub mod schedule;
pub mod store;
pub mod trae_auth;
pub mod trae_instance;
pub mod trae_machine;
pub mod views;
