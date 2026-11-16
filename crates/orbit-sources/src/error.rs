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

    /// 稳定的机器可读错误代码(界面据此显示本地化说明)。
    pub fn code(&self) -> &'static str {
        match self {
            Self::Http { .. } => "http",
            Self::Status { .. } => "status",
            Self::Login(_) => "login",
            Self::MissingCredentials(_) => "missing_credentials",
            Self::Cache { .. } => "cache",
            Self::File { .. } => "file",
            Self::Parse { .. } => "parse",
            Self::NoCache(_) => "no_cache",
            Self::Config(_) => "config",
        }
    }

    /// 错误的具体内容(地址、路径、底层错误等),不含固定的说明文字,供界面与本地化说明拼接。
    pub fn detail(&self) -> String {
        match self {
            Self::Http { url, message } => format!("{url}: {message}"),
            Self::Status { url, status } => format!("{status} ({url})"),
            Self::Cache { path, message } | Self::File { path, message } => {
                format!("{path}: {message}")
            }
            Self::Parse { what, message } => format!("{what}: {message}"),
            Self::Login(s) | Self::MissingCredentials(s) | Self::NoCache(s) | Self::Config(s) => s.clone(),
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
