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
    InvalidFilter(FilterIssue),

    #[error("物体下标越界: {0}")]
    IndexOutOfRange(usize),

    #[error("无效参数: {0}")]
    InvalidArgument(String),
}

impl CoreError {
    /// 稳定的机器可读错误代码(API 对外提供,界面据此显示本地化说明)。
    pub fn code(&self) -> &'static str {
        match self {
            Self::Json(_) => "json",
            Self::Propagation { .. } => "propagation",
            Self::TimeOutOfRange(_) => "time_out_of_range",
            Self::InvalidFilter(issue) => issue.code(),
            Self::IndexOutOfRange(_) => "index_out_of_range",
            Self::InvalidArgument(_) => "invalid_argument",
        }
    }
}

/// 过滤条件不合法的具体原因。
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum FilterIssue {
    #[error("高度范围不能为 NaN")]
    AltNan,
    #[error("alt_min_km 不能大于 alt_max_km")]
    AltOrder,
    #[error("min_size_cm 必须是非负有限数")]
    MinSize,
    #[error("inclination_min_deg 不能大于 inclination_max_deg")]
    InclinationOrder,
    #[error("max_epoch_age_days 必须非负")]
    EpochAge,
}

impl FilterIssue {
    /// 稳定的机器可读错误代码。
    pub fn code(self) -> &'static str {
        match self {
            Self::AltNan => "filter_alt_nan",
            Self::AltOrder => "filter_alt_order",
            Self::MinSize => "filter_min_size",
            Self::InclinationOrder => "filter_inc_order",
            Self::EpochAge => "filter_epoch_age",
        }
    }
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
