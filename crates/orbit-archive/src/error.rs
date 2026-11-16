use thiserror::Error;

#[derive(Debug, Error)]
pub enum ArchiveError {
    #[error("历史库错误: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("文件读写失败 ({path}): {message}")]
    Io { path: String, message: String },

    #[error("编码失败: {0}")]
    Encode(String),

    #[error("配置错误: {0}")]
    Config(String),
}
