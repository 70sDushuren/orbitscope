//! 带机器可读代码的错误。
//!
//! 对外的错误除中文说明(`message`)外,还带稳定的 `code` 与参数 `params`;
//! 界面据此显示所选语言的说明(词典键 `err.<code>`),不认识的代码回退到 `message`。

use orbit_core::CoreError;
use serde_json::{Map, Value};

/// 一条带代码的错误。
#[derive(Debug, Clone, PartialEq)]
pub struct Coded {
    pub code: &'static str,
    pub message: String,
    pub params: Map<String, Value>,
}

impl Coded {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            params: Map::new(),
        }
    }

    /// 附加一个参数(界面说明中的 `{name}` 占位符)。
    pub fn param(mut self, name: &str, value: impl Into<Value>) -> Self {
        self.params.insert(name.to_string(), value.into());
        self
    }
}

impl std::fmt::Display for Coded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl From<CoreError> for Coded {
    fn from(e: CoreError) -> Self {
        let coded = Coded::new(e.code(), e.to_string());
        match &e {
            // 过滤条件问题各有专门的代码,无需参数
            CoreError::InvalidFilter(_) => coded,
            CoreError::Json(d) | CoreError::TimeOutOfRange(d) | CoreError::InvalidArgument(d) => {
                coded.param("detail", d.clone())
            }
            CoreError::Propagation { norad_id, message } => {
                coded.param("id", *norad_id).param("detail", message.clone())
            }
            CoreError::IndexOutOfRange(i) => coded.param("detail", i.to_string()),
        }
    }
}
