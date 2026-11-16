use thiserror::Error;

/// 数据源错误。注意:任何变体都不得包含凭据。
#[derive(Debug, Error)]
pub enum SourceError {
    #[error("HTTP 请求失败 ({url}): {message}")]
    Http { url: String, message: String },

    #[error("HTTP {status} ({url})")]
    Status { url: String, status: u16 },

    #[error("Space-Track 登录失败: {0}")]
    Login(String),

    #[error("缺少凭据: {0}")]
    MissingCredentials(String),

    #[error("缓存读写失败 ({path}): {message}")]
    Cache { path: String, message: String },

    #[error("读取文件失败 ({path}): {message}")]
    File { path: String, message: String },

    #[error("数据解析失败 ({what}): {message}")]
    Parse { what: String, message: String },

    #[error("离线模式且无缓存: {0}")]
    NoCache(String),

    #[error("配置错误: {0}")]
    Config(String),
}

impl SourceError {
    pub(crate) fn http(url: &str, e: impl std::fmt::Display) -> Self {
        Self::Http {
            url: url.to_string(),
            message: e.to_string(),
        }
    }

    /// 是否值得重试(网络错误、429、5xx)。
    pub fn is_transient(&self) -> bool {
        match self {
            Self::Http { .. } => true,
            Self::Status { status, .. } => *status == 429 || *status >= 500,
            _ => false,
        }
    }
}
