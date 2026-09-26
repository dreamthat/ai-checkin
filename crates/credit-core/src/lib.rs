//! Qoder / ZCode 签到与活动领取核心逻辑(移植自 CreditDaddy)。
//! 不依赖 Tauri:桌面宿主(src-tauri)与 HTTP server(wb-switch-server)共用。
//! 数据目录:`~/.wb-switch/qoder/`、`~/.wb-switch/zcode/`。
//!
//! 协议事实来源:C:\Users\admin\AppData\Local\Temp\CreditDaddy\src\*(constants / qoderClient /
//! qoderApp / zcodeClient / zcodeAutoClaim / zcodeLocal / zcrypto / checkin)。

pub mod error;
pub mod log;
pub mod models;
pub mod qoder;
pub mod schedule;
pub mod store;
pub mod zcode;
