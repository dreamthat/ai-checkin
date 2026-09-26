//! Qoder 模块:协议客户端(constants/qoderClient)+ 设备风控身份(qoderApp/qoderUmid)+ 领取编排(checkin)。

pub mod checkin;
pub mod client;
pub mod identity;

pub use client::{api_base, build_headers, CampaignsResp, DeviceIdentity};
pub use identity::RiskIdentity;
