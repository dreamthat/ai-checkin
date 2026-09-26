// 应用错误类型(仿 trae-core AppError)。实现 Serialize 以便跨 IPC/HTTP 边界返回前端(序列化为字符串)。

use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),
    #[error("数据解析错误: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("网络请求失败: {0}")]
    Network(String),
    #[error("接口错误: {0}")]
    Api(String),
    #[error("凭据错误: {0}")]
    Credential(String),
    #[error("Windows DPAPI 错误: {0}")]
    Dpapi(String),
    #[error("未找到: {0}")]
    NotFound(String),
    #[error("配置错误: {0}")]
    Config(String),
}

impl Serialize for AppError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.to_string().as_ref())
    }
}

pub type AppResult<T> = Result<T, AppError>;
