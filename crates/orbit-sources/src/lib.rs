//! # orbit-sources
//!
//! OrbitScope 数据源层:从 Space-Track、CelesTrak、本地文件获取 GP 根数与元数据,
//! 带磁盘缓存、条件请求、限速与离线回退,输出 [`orbit_core::GpRecord`]。
//!
//! 典型用法:
//!
//! ```no_run
//! # async fn demo() -> Result<(), orbit_sources::SourceError> {
//! use orbit_sources::{FetchMode, SourceSet, SourcesConfig, spacetrack::Credentials};
//! let set = SourceSet::from_config(&SourcesConfig::default(), Credentials::from_env())?;
//! let result = set.fetch_all(FetchMode::Normal, chrono::Utc::now()).await;
//! let (catalog, report) = result.build_catalog(chrono::Utc::now());
//! # Ok(()) }
//! ```

pub mod cache;
pub mod celestrak;
pub mod error;
pub mod file;
pub mod http;
pub mod ratelimit;
pub mod satcat;
pub mod spacetrack;

pub use cache::{Cache, CacheMeta};
pub use error::SourceError;
pub use http::{FetchMode, Origin, ResourceStatus, USER_AGENT};

use celestrak::{CelesTrakConfig, CelesTrakSource};
use chrono::{DateTime, Utc};
use file::{FileConfig, FileSource};
use orbit_core::{BuildReport, Catalog, DataSource, GpRecord, NoradId, ObjectMeta, apply_meta};
use serde::{Deserialize, Serialize};
use spacetrack::{Credentials, SpaceTrackConfig, SpaceTrackSource};
use std::collections::HashMap;
use std::path::PathBuf;

/// 单个数据源一次获取的结果。
#[derive(Debug)]
pub struct SourceBatch {
    pub source: DataSource,
    pub label: String,
    pub records: Vec<GpRecord>,
    /// 补充元数据(如 SATCAT),按编号合并到所有记录
    pub extra_meta: HashMap<NoradId, ObjectMeta>,
    /// 被跳过的单条记录数
    pub record_errors: usize,
    pub resources: Vec<ResourceStatus>,
    pub notes: Vec<String>,
}

impl SourceBatch {
    pub fn new(source: DataSource, label: &str) -> Self {
        Self {
            source,
            label: label.to_string(),
            records: Vec::new(),
            extra_meta: HashMap::new(),
            record_errors: 0,
            resources: Vec::new(),
            notes: Vec::new(),
        }
    }
}

/// 数据源状态摘要(供 CLI / API / UI 展示)。
#[derive(Debug, Clone, Serialize)]
pub struct SourceStatus {
    pub label: String,
    pub source: DataSource,
    pub ok: bool,
    pub records: usize,
    pub record_errors: usize,
    pub extra_meta: usize,
    pub resources: Vec<ResourceStatus>,
    pub notes: Vec<String>,
    pub error: Option<String>,
    /// 失败时的错误代码(见 [`SourceError::code`]),界面据此显示本地化说明
    pub error_code: Option<&'static str>,
    /// 失败时的具体内容(见 [`SourceError::detail`])
    pub error_detail: Option<String>,
}

/// 数据源总配置。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SourcesConfig {
    /// 缓存目录;默认为用户缓存目录下的 `orbitscope`
    pub cache_dir: Option<PathBuf>,
    pub celestrak: CelesTrakConfig,
    pub spacetrack: SpaceTrackConfig,
    pub files: FileConfig,
}

/// 一个已配置的数据源。
#[derive(Debug)]
pub enum Source {
    SpaceTrack(SpaceTrackSource),
    CelesTrak(CelesTrakSource),
    File(FileSource),
}

impl Source {
    pub fn label(&self) -> &'static str {
        match self {
            Self::SpaceTrack(_) => "space-track",
            Self::CelesTrak(_) => "celestrak",
            Self::File(_) => "file",
        }
    }

    pub fn kind(&self) -> DataSource {
        match self {
            Self::SpaceTrack(_) => DataSource::SpaceTrack,
            Self::CelesTrak(_) => DataSource::CelesTrak,
            Self::File(_) => DataSource::File,
        }
    }

    pub async fn fetch(&self, mode: FetchMode, now: DateTime<Utc>) -> Result<SourceBatch, SourceError> {
        match self {
            Self::SpaceTrack(s) => s.fetch(mode, now).await,
            Self::CelesTrak(s) => s.fetch(mode, now).await,
            Self::File(s) => s.fetch().await,
        }
    }
}

/// 全部数据源的合并结果。
#[derive(Debug, Default)]
pub struct FetchAllResult {
    pub records: Vec<GpRecord>,
    pub extra_meta: HashMap<NoradId, ObjectMeta>,
    pub statuses: Vec<SourceStatus>,
}

impl FetchAllResult {
    /// 合并补充元数据并构建编目。
    pub fn build_catalog(mut self, now: DateTime<Utc>) -> (Catalog, BuildReport) {
        apply_meta(&mut self.records, &self.extra_meta);
        Catalog::build(self.records, now)
    }

    /// 是否至少有一个数据源成功。
    pub fn any_ok(&self) -> bool {
        self.statuses.iter().any(|s| s.ok)
    }
}

/// 一组数据源。顺序即优先级:Space-Track → CelesTrak → 文件
/// (同一编号 epoch 相同时,先出现者的根数被保留)。
#[derive(Debug)]
pub struct SourceSet {
    sources: Vec<Source>,
    cache: Cache,
}

impl SourceSet {
    /// 按配置创建。Space-Track 启用但没有凭据时返回错误。
    pub fn from_config(cfg: &SourcesConfig, creds: Option<Credentials>) -> Result<Self, SourceError> {
        let root = cfg
            .cache_dir
            .clone()
            .or_else(Cache::default_root)
            .ok_or_else(|| SourceError::Config("无法确定缓存目录,请设置 cache_dir".into()))?;
        let cache = Cache::new(root);

        let mut sources = Vec::new();
        if cfg.spacetrack.enabled {
            let creds = creds.ok_or_else(|| {
                SourceError::MissingCredentials(format!(
                    "已启用 Space-Track,请设置环境变量 {} 和 {}",
                    spacetrack::ENV_USER,
                    spacetrack::ENV_PASSWORD
                ))
            })?;
            sources.push(Source::SpaceTrack(SpaceTrackSource::new(
                cfg.spacetrack.clone(),
                creds,
                cache.clone(),
            )?));
        }
        if cfg.celestrak.enabled {
            sources.push(Source::CelesTrak(CelesTrakSource::new(
                cfg.celestrak.clone(),
                cache.clone(),
            )?));
        }
        if !cfg.files.paths.is_empty() {
            sources.push(Source::File(FileSource::new(cfg.files.clone())));
        }
        if sources.is_empty() {
            return Err(SourceError::Config("没有启用任何数据源".into()));
        }
        Ok(Self { sources, cache })
    }

    /// 由已构建的数据源创建(测试或自定义组合)。
    pub fn new(sources: Vec<Source>, cache: Cache) -> Self {
        Self { sources, cache }
    }

    pub fn sources(&self) -> &[Source] {
        &self.sources
    }

    pub fn cache(&self) -> &Cache {
        &self.cache
    }

    /// 依次获取全部数据源。单个数据源失败只体现在其状态中。
    pub async fn fetch_all(&self, mode: FetchMode, now: DateTime<Utc>) -> FetchAllResult {
        let mut out = FetchAllResult::default();
        for src in &self.sources {
            match src.fetch(mode, now).await {
                Ok(batch) => {
                    out.statuses.push(SourceStatus {
                        label: batch.label,
                        source: batch.source,
                        ok: true,
                        records: batch.records.len(),
                        record_errors: batch.record_errors,
                        extra_meta: batch.extra_meta.len(),
                        resources: batch.resources,
                        notes: batch.notes,
                        error: None,
                        error_code: None,
                        error_detail: None,
                    });
                    out.records.extend(batch.records);
                    for (id, m) in batch.extra_meta {
                        out.extra_meta.entry(id).or_default().fill_missing_from(&m);
                    }
                }
                Err(e) => {
                    tracing::warn!(source = src.label(), error = %e, "数据源获取失败");
                    out.statuses.push(SourceStatus {
                        label: src.label().to_string(),
                        source: src.kind(),
                        ok: false,
                        records: 0,
                        record_errors: 0,
                        extra_meta: 0,
                        resources: Vec::new(),
                        notes: Vec::new(),
                        error: Some(e.to_string()),
                        error_code: Some(e.code()),
                        error_detail: Some(e.detail()),
                    });
                }
            }
        }
        out
    }
}
