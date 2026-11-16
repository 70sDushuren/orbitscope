use crate::model::NoradId;
use thiserror::Error;

/// orbit-core 的错误类型。
#[derive(Debug, Error, Clone, PartialEq)]
pub enum CoreError {
    #[error("JSON 解析失败: {0}")]
    Json(String),

    #[error("NORAD {norad_id}: SGP4 外推失败: {message}")]
    Propagation { norad_id: NoradId, message: String },

    #[error("时间超出可外推范围: {0}")]
    TimeOutOfRange(String),

    #[error("无效的过滤条件: {0}")]
    InvalidFilter(String),

    #[error("物体下标越界: {0}")]
    IndexOutOfRange(usize),

    #[error("无效参数: {0}")]
    InvalidArgument(String),
}

/// 单条记录的解析错误。
#[derive(Debug, Clone, PartialEq, Error)]
#[error("记录 #{index}{}: {message}", norad_id.map(|id| format!(" (NORAD {id})")).unwrap_or_default())]
pub struct RecordError {
    /// 记录在输入中的序号(从 0 开始)。
    pub index: usize,
    /// 若能识别出编号则给出。
    pub norad_id: Option<NoradId>,
    pub message: String,
}
