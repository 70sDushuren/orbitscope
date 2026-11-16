//! 磁盘缓存:每个资源一个正文文件 + 一个元数据 JSON。写入采用"临时文件 + rename",避免半写文件。
//!
//! 目录:`<root>/<key>.body` 与 `<root>/<key>.meta.json`;key 中的非安全字符替换为 `_`。

use crate::error::SourceError;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// 缓存元数据。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CacheMeta {
    /// 资源 URL(不含任何凭据)
    pub url: String,
    /// 最近一次与服务器确认内容的时间(下载成功或 304)
    pub fetched_at: DateTime<Utc>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub bytes: u64,
}

impl CacheMeta {
    /// 缓存年龄(秒)。
    pub fn age_secs(&self, now: DateTime<Utc>) -> i64 {
        (now - self.fetched_at).num_seconds()
    }
}

/// 磁盘缓存。
#[derive(Debug, Clone)]
pub struct Cache {
    root: PathBuf,
}

fn sanitize(key: &str) -> String {
    key.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn io_err(path: &Path, e: impl std::fmt::Display) -> SourceError {
    SourceError::Cache {
        path: path.display().to_string(),
        message: e.to_string(),
    }
}

impl Cache {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// 默认缓存目录:`$XDG_CACHE_HOME/orbitscope`(各平台的用户缓存目录)。
    pub fn default_root() -> Option<PathBuf> {
        dirs::cache_dir().map(|d| d.join("orbitscope"))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn paths(&self, key: &str) -> (PathBuf, PathBuf) {
        let k = sanitize(key);
        (
            self.root.join(format!("{k}.body")),
            self.root.join(format!("{k}.meta.json")),
        )
    }

    /// 只读元数据。
    pub async fn read_meta(&self, key: &str) -> Result<Option<CacheMeta>, SourceError> {
        let (_, meta_path) = self.paths(key);
        match tokio::fs::read_to_string(&meta_path).await {
            Ok(s) => match serde_json::from_str(&s) {
                Ok(m) => Ok(Some(m)),
                Err(e) => {
                    tracing::warn!(path = %meta_path.display(), error = %e, "缓存元数据损坏,忽略");
                    Ok(None)
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(io_err(&meta_path, e)),
        }
    }

    /// 读取元数据与正文;任一缺失视为无缓存。
    pub async fn read(&self, key: &str) -> Result<Option<(CacheMeta, String)>, SourceError> {
        let Some(meta) = self.read_meta(key).await? else {
            return Ok(None);
        };
        let (body_path, _) = self.paths(key);
        match tokio::fs::read_to_string(&body_path).await {
            Ok(body) => Ok(Some((meta, body))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(io_err(&body_path, e)),
        }
    }

    async fn write_atomic(path: &Path, data: &[u8]) -> Result<(), SourceError> {
        let tmp = path.with_extension("tmp");
        tokio::fs::write(&tmp, data).await.map_err(|e| io_err(&tmp, e))?;
        tokio::fs::rename(&tmp, path).await.map_err(|e| io_err(path, e))
    }

    /// 写入正文与元数据(先正文后元数据,保证元数据存在时正文完整)。
    pub async fn write(&self, key: &str, meta: &CacheMeta, body: &str) -> Result<(), SourceError> {
        tokio::fs::create_dir_all(&self.root)
            .await
            .map_err(|e| io_err(&self.root, e))?;
        let (body_path, meta_path) = self.paths(key);
        Self::write_atomic(&body_path, body.as_bytes()).await?;
        let json = serde_json::to_vec_pretty(meta).map_err(|e| io_err(&meta_path, e))?;
        Self::write_atomic(&meta_path, &json).await
    }

    /// 只更新元数据(例如收到 304 后刷新 `fetched_at`)。
    pub async fn write_meta(&self, key: &str, meta: &CacheMeta) -> Result<(), SourceError> {
        tokio::fs::create_dir_all(&self.root)
            .await
            .map_err(|e| io_err(&self.root, e))?;
        let (_, meta_path) = self.paths(key);
        let json = serde_json::to_vec_pretty(meta).map_err(|e| io_err(&meta_path, e))?;
        Self::write_atomic(&meta_path, &json).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_key() {
        assert_eq!(sanitize("celestrak/gp-active"), "celestrak_gp-active");
        assert_eq!(sanitize("../x?y=1"), ".._x_y_1");
    }

    #[tokio::test]
    async fn round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path().join("c"));
        assert!(cache.read("k").await.unwrap().is_none());
        let meta = CacheMeta {
            url: "https://example.org/x".into(),
            fetched_at: Utc::now(),
            etag: Some("\"abc\"".into()),
            last_modified: None,
            bytes: 5,
        };
        cache.write("a/k", &meta, "hello").await.unwrap();
        let (m, body) = cache.read("a/k").await.unwrap().unwrap();
        assert_eq!(m, meta);
        assert_eq!(body, "hello");
    }
}
