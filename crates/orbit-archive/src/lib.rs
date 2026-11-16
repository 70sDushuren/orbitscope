//! # orbit-archive
//!
//! 历史归档(设计见 `docs/03-design.md` 4.6 节):
//! - [`History`] —— 根数历史库(SQLite),每条根数只写一次并记录首次获取时间,永不覆盖;
//! - [`SnapshotStore`] —— 每次网络下载的原始内容(gzip),按文件数与空间上限淘汰最旧的;
//! - 按时刻策略([`TimePolicy`])为每个物体选取根数,并给出数据依据([`Provenance`])。

pub mod config;
pub mod error;
pub mod history;
pub mod snapshots;

pub use config::{ArchiveConfig, CollectConfig, CollectMode, HistoryConfig, SnapshotConfig, TimePolicy};
pub use error::ArchiveError;
pub use history::{ElsetInfo, FetchRow, History, HistoryUsage, IngestStats, Provenance};
pub use snapshots::{SnapshotFile, SnapshotStore, SnapshotUsage};

use chrono::{DateTime, Utc};
use orbit_core::{BuildReport, Catalog, apply_meta};
use orbit_sources::{Cache, FetchAllResult, Origin};
use serde::Serialize;

/// 一次归档的结果。
#[derive(Debug, Clone, Default, Serialize)]
pub struct ArchiveReport {
    pub history: Option<IngestStats>,
    pub snapshots_saved: usize,
    pub snapshots_pruned: usize,
    pub elsets_pruned: u64,
}

/// 归档占用。
#[derive(Debug, Clone, Default, Serialize)]
pub struct ArchiveUsage {
    pub dir: String,
    pub history: Option<HistoryUsage>,
    pub snapshots: SnapshotUsage,
}

/// 归档:历史库 + 快照。
#[derive(Debug)]
pub struct Archive {
    cfg: ArchiveConfig,
    dir: std::path::PathBuf,
    history: Option<History>,
    snapshots: SnapshotStore,
}

fn origin_name(o: &Origin) -> &'static str {
    match o {
        Origin::Network => "network",
        Origin::NotModified => "not_modified",
        Origin::Cache { .. } => "cache",
        Origin::StaleCache { .. } => "stale_cache",
    }
}

impl Archive {
    pub fn open(cfg: &ArchiveConfig) -> Result<Self, ArchiveError> {
        let dir = cfg
            .resolved_dir()
            .ok_or_else(|| ArchiveError::Config("无法确定归档目录,请设置 [archive] dir".into()))?;
        let history = if cfg.history.enabled {
            Some(History::open(dir.join("history.sqlite"), cfg.history.clone())?)
        } else {
            None
        };
        let snapshots = SnapshotStore::new(dir.join("snapshots"), cfg.snapshots.clone());
        Ok(Self {
            cfg: cfg.clone(),
            dir,
            history,
            snapshots,
        })
    }

    pub fn config(&self) -> &ArchiveConfig {
        &self.cfg
    }

    pub fn dir(&self) -> &std::path::Path {
        &self.dir
    }

    pub fn history(&self) -> Option<&History> {
        self.history.as_ref()
    }

    pub fn snapshots(&self) -> &SnapshotStore {
        &self.snapshots
    }

    /// 归档一次获取结果:原始快照(仅网络下载的资源)、根数历史、采集记录,然后按配置清理。
    ///
    /// 同步执行(含 SQLite 与文件 IO),异步代码中请放到 `spawn_blocking`。
    pub fn record(
        &self,
        result: &FetchAllResult,
        cache: &Cache,
        now: DateTime<Utc>,
    ) -> Result<ArchiveReport, ArchiveError> {
        let mut report = ArchiveReport::default();

        if self.cfg.snapshots.enabled {
            for st in &result.statuses {
                for r in st.resources.iter().filter(|r| r.origin == Origin::Network) {
                    match cache.read_blocking(&r.key) {
                        Ok(Some((meta, body))) if meta.fetched_at == r.fetched_at => {
                            self.snapshots.save(&r.key, r.fetched_at, body.as_bytes())?;
                            report.snapshots_saved += 1;
                        }
                        Ok(_) => tracing::warn!(key = %r.key, "缓存内容已变化或缺失,跳过快照"),
                        Err(e) => tracing::warn!(key = %r.key, error = %e, "读取缓存失败,跳过快照"),
                    }
                }
            }
            report.snapshots_pruned = self.snapshots.prune()?.0;
        }

        if let Some(h) = &self.history {
            let mut records = result.records.clone();
            apply_meta(&mut records, &result.extra_meta);
            // 只记录真正与服务器交互的获取(下载或 304)
            let fetches: Vec<FetchRow> = result
                .statuses
                .iter()
                .flat_map(|s| {
                    s.resources
                        .iter()
                        .filter(|r| matches!(r.origin, Origin::Network | Origin::NotModified))
                        .map(|r| FetchRow {
                            fetched_at: r.fetched_at,
                            source: s.label.clone(),
                            resource: r.key.clone(),
                            origin: origin_name(&r.origin).into(),
                            bytes: r.bytes,
                        })
                })
                .collect();
            report.history = Some(h.ingest(&records, &fetches, now)?);
            report.elsets_pruned = h.prune(now)?;
        }
        Ok(report)
    }

    /// 按时刻策略从历史库构建编目。历史库关闭时返回 `None`。
    ///
    /// `now` 用于判断再入(取 `t`,即数据时刻)。
    pub fn catalog_at(
        &self,
        policy: TimePolicy,
        t: DateTime<Utc>,
    ) -> Result<Option<(Catalog, BuildReport, Provenance)>, ArchiveError> {
        let Some(h) = &self.history else { return Ok(None) };
        let (records, prov) = h.select(policy, t)?;
        let (cat, report) = Catalog::build(records, t);
        Ok(Some((cat, report, prov)))
    }

    pub fn usage(&self) -> Result<ArchiveUsage, ArchiveError> {
        Ok(ArchiveUsage {
            dir: self.dir.display().to_string(),
            history: self.history.as_ref().map(History::usage).transpose()?,
            snapshots: self.snapshots.usage()?,
        })
    }

    /// 手动清理。
    pub fn prune(&self, now: DateTime<Utc>) -> Result<ArchiveReport, ArchiveError> {
        Ok(ArchiveReport {
            history: None,
            snapshots_saved: 0,
            snapshots_pruned: self.snapshots.prune()?.0,
            elsets_pruned: match &self.history {
                Some(h) => h.prune(now)?,
                None => 0,
            },
        })
    }
}
