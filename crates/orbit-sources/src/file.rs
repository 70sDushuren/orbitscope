//! 本地文件数据源:TLE / 3LE / OMM JSON。按内容自动识别格式。

use crate::SourceBatch;
use crate::error::SourceError;
use orbit_core::{DataSource, ParseOutcome, parse_omm_json, parse_tle_text};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FileConfig {
    pub paths: Vec<PathBuf>,
}

/// 按内容识别格式并解析:以 `[` 或 `{` 开头视为 OMM JSON,否则视为 TLE。
pub fn parse_any(text: &str) -> Result<ParseOutcome, SourceError> {
    if text.trim_start().starts_with(['[', '{']) {
        parse_omm_json(text, DataSource::File).map_err(|e| SourceError::Parse {
            what: "OMM JSON".into(),
            message: e.to_string(),
        })
    } else {
        Ok(parse_tle_text(text, DataSource::File))
    }
}

#[derive(Debug, Clone)]
pub struct FileSource {
    cfg: FileConfig,
}

impl FileSource {
    pub fn new(cfg: FileConfig) -> Self {
        Self { cfg }
    }

    /// 读取全部文件。单个文件失败只记入 notes;全部失败才返回错误。
    pub async fn fetch(&self) -> Result<SourceBatch, SourceError> {
        let mut batch = SourceBatch::new(DataSource::File, "file");
        let mut last_err = None;
        for path in &self.cfg.paths {
            let result = match tokio::fs::read_to_string(path).await {
                Ok(text) => parse_any(&text),
                Err(e) => Err(SourceError::File {
                    path: path.display().to_string(),
                    message: e.to_string(),
                }),
            };
            match result {
                Ok(out) => {
                    batch.record_errors += out.errors.len();
                    batch.records.extend(out.records);
                }
                Err(e) => {
                    batch.notes.push(format!("{}: {e}", path.display()));
                    last_err = Some(e);
                }
            }
        }
        match last_err {
            Some(e) if batch.records.is_empty() => Err(e),
            _ => Ok(batch),
        }
    }
}
