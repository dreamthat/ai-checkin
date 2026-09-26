//! ZCode(智谱 GLM / Z.ai)模块:本机凭据(zcodeLocal/zcrypto)+ 协议客户端(zcodeClient)+ 领取编排(zcodeAutoClaim)。

pub mod claim;
pub mod client;
pub mod credentials;

pub use client::{map_claim_code, ClaimPlan};
