//! 根数历史库(SQLite)。每条根数(编号 + 历元 + 数据源)只写入一次,永不覆盖。

use crate::config::{HistoryConfig, TimePolicy};
use crate::error::ArchiveError;
use chrono::{DateTime, Utc};
use orbit_core::{DataSource, GpRecord};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS fetches (
    id            INTEGER PRIMARY KEY,
    fetched_at_us INTEGER NOT NULL,
    source        TEXT    NOT NULL,
    resource      TEXT    NOT NULL,
    origin        TEXT    NOT NULL,
    bytes         INTEGER NOT NULL,
    UNIQUE (resource, fetched_at_us)
);
CREATE TABLE IF NOT EXISTS elsets (
    id            INTEGER PRIMARY KEY,
    norad         INTEGER NOT NULL,
    epoch_us      INTEGER NOT NULL,
    source        TEXT    NOT NULL,
    source_rank   INTEGER NOT NULL,
    first_seen_us INTEGER NOT NULL,
    record_json   TEXT    NOT NULL,
    UNIQUE (norad, epoch_us, source)
);
CREATE INDEX IF NOT EXISTS elsets_norad_seen  ON elsets (norad, first_seen_us);
CREATE INDEX IF NOT EXISTS elsets_norad_epoch ON elsets (norad, epoch_us);
CREATE INDEX IF NOT EXISTS elsets_seen        ON elsets (first_seen_us);
";

fn us(t: DateTime<Utc>) -> i64 {
    t.timestamp_micros()
}

fn from_us(v: i64) -> DateTime<Utc> {
    DateTime::from_timestamp_micros(v).unwrap_or(DateTime::<Utc>::UNIX_EPOCH)
}

fn source_rank(s: DataSource) -> i64 {
    match s {
        DataSource::SpaceTrack => 0,
        DataSource::CelesTrak => 1,
        DataSource::File => 2,
    }
}

/// 一次获取(资源级)的记录。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FetchRow {
    pub fetched_at: DateTime<Utc>,
    pub source: String,
    pub resource: String,
    pub origin: String,
    pub bytes: u64,
}

/// 历史中的一条根数(不含内容)。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ElsetInfo {
    pub epoch: DateTime<Utc>,
    pub first_seen: DateTime<Utc>,
    pub source: String,
}

/// 写入结果。
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct IngestStats {
    pub records: usize,
    pub new_elsets: usize,
    pub fetches: usize,
}

/// 历史库占用。
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct HistoryUsage {
    pub elsets: u64,
    pub objects: u64,
    pub fetches: u64,
    pub bytes: u64,
    pub first_seen_min: Option<DateTime<Utc>>,
    pub first_seen_max: Option<DateTime<Utc>>,
}

/// 按时刻选取结果的来源说明。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Provenance {
    pub policy: TimePolicy,
    /// 数据时刻 T
    pub data_time: DateTime<Utc>,
    pub records: usize,
    /// 所选根数的获取时间范围
    pub first_seen_min: Option<DateTime<Utc>>,
    pub first_seen_max: Option<DateTime<Utc>>,
    /// 所选根数的历元范围
    pub epoch_min: Option<DateTime<Utc>>,
    pub epoch_max: Option<DateTime<Utc>>,
}

/// 历史库。内部持有一个连接(加互斥锁);各操作为同步调用,异步代码中请放到 `spawn_blocking`。
#[derive(Debug)]
pub struct History {
    path: PathBuf,
    cfg: HistoryConfig,
    conn: Mutex<Connection>,
}

impl History {
    pub fn open(path: impl Into<PathBuf>, cfg: HistoryConfig) -> Result<Self, ArchiveError> {
        let path = path.into();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| ArchiveError::Io {
                path: dir.display().to_string(),
                message: e.to_string(),
            })?;
        }
        let conn = Connection::open(&path)?;
        // auto_vacuum 必须在建表前设置
        conn.execute_batch(
            "PRAGMA auto_vacuum = INCREMENTAL;
             PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA busy_timeout = 10000;",
        )?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self {
            path,
            cfg,
            conn: Mutex::new(conn),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// 写入一批根数;`fallback_seen` 用于没有 `fetched_at` 的记录。已存在的根数保持不变。
    pub fn ingest(
        &self,
        records: &[GpRecord],
        fetches: &[FetchRow],
        fallback_seen: DateTime<Utc>,
    ) -> Result<IngestStats, ArchiveError> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let mut stats = IngestStats {
            records: records.len(),
            ..Default::default()
        };
        {
            let mut ins = tx.prepare_cached(
                "INSERT OR IGNORE INTO elsets (norad, epoch_us, source, source_rank, first_seen_us, record_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            for r in records {
                let json = serde_json::to_string(r).map_err(|e| ArchiveError::Encode(e.to_string()))?;
                let seen = r.fetched_at.unwrap_or(fallback_seen);
                stats.new_elsets += ins.execute(params![
                    i64::from(r.norad_id()),
                    us(r.epoch()),
                    r.source.to_string(),
                    source_rank(r.source),
                    us(seen),
                    json
                ])?;
            }
            let mut f = tx.prepare_cached(
                "INSERT OR IGNORE INTO fetches (fetched_at_us, source, resource, origin, bytes) VALUES (?1, ?2, ?3, ?4, ?5)",
            )?;
            for row in fetches {
                stats.fetches += f.execute(params![
                    us(row.fetched_at),
                    row.source,
                    row.resource,
                    row.origin,
                    row.bytes as i64
                ])?;
            }
        }
        tx.commit()?;
        Ok(stats)
    }

    /// 按时刻策略为每个物体选取一条根数。
    pub fn select(
        &self,
        policy: TimePolicy,
        t: DateTime<Utc>,
    ) -> Result<(Vec<GpRecord>, Provenance), ArchiveError> {
        let sql = match policy {
            TimePolicy::KnownAt => {
                "SELECT record_json, first_seen_us, epoch_us FROM (
                   SELECT record_json, first_seen_us, epoch_us,
                          ROW_NUMBER() OVER (PARTITION BY norad ORDER BY epoch_us DESC, source_rank ASC) AS rn
                   FROM elsets WHERE first_seen_us <= ?1) WHERE rn = 1"
            }
            TimePolicy::NearestEpoch => {
                "SELECT record_json, first_seen_us, epoch_us FROM (
                   SELECT record_json, first_seen_us, epoch_us,
                          ROW_NUMBER() OVER (PARTITION BY norad ORDER BY ABS(epoch_us - ?1) ASC, source_rank ASC) AS rn
                   FROM elsets) WHERE rn = 1"
            }
            TimePolicy::Latest => {
                "SELECT record_json, first_seen_us, epoch_us FROM (
                   SELECT record_json, first_seen_us, epoch_us,
                          ROW_NUMBER() OVER (PARTITION BY norad ORDER BY epoch_us DESC, source_rank ASC) AS rn
                   FROM elsets WHERE ?1 = ?1) WHERE rn = 1"
            }
        };
        let conn = self.lock();
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map(params![us(t)], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?))
        })?;
        let mut records = Vec::new();
        let mut prov = Provenance {
            policy,
            data_time: t,
            records: 0,
            first_seen_min: None,
            first_seen_max: None,
            epoch_min: None,
            epoch_max: None,
        };
        let (mut smin, mut smax, mut emin, mut emax) = (i64::MAX, i64::MIN, i64::MAX, i64::MIN);
        let mut bad = 0usize;
        for row in rows {
            let (json, seen, epoch) = row?;
            match serde_json::from_str::<GpRecord>(&json) {
                Ok(rec) => {
                    records.push(rec);
                    smin = smin.min(seen);
                    smax = smax.max(seen);
                    emin = emin.min(epoch);
                    emax = emax.max(epoch);
                }
                Err(_) => bad += 1,
            }
        }
        if bad > 0 {
            tracing::warn!(bad, "历史库中有无法解析的记录,已跳过");
        }
        if !records.is_empty() {
            prov.first_seen_min = Some(from_us(smin));
            prov.first_seen_max = Some(from_us(smax));
            prov.epoch_min = Some(from_us(emin));
            prov.epoch_max = Some(from_us(emax));
        }
        prov.records = records.len();
        Ok((records, prov))
    }

    /// 最近的获取记录(新的在前)。
    pub fn recent_fetches(&self, limit: usize) -> Result<Vec<FetchRow>, ArchiveError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT fetched_at_us, source, resource, origin, bytes FROM fetches ORDER BY fetched_at_us DESC, id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], |r| {
            Ok(FetchRow {
                fetched_at: from_us(r.get(0)?),
                source: r.get(1)?,
                resource: r.get(2)?,
                origin: r.get(3)?,
                bytes: r.get::<_, i64>(4)?.max(0) as u64,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    fn db_bytes(conn: &Connection) -> Result<u64, ArchiveError> {
        let page_size: i64 = conn.query_row("PRAGMA page_size", [], |r| r.get(0))?;
        let pages: i64 = conn.query_row("PRAGMA page_count", [], |r| r.get(0))?;
        let free: i64 = conn.query_row("PRAGMA freelist_count", [], |r| r.get(0))?;
        Ok(((pages - free).max(0) * page_size) as u64)
    }

    pub fn usage(&self) -> Result<HistoryUsage, ArchiveError> {
        let conn = self.lock();
        let (elsets, objects, smin, smax): (i64, i64, Option<i64>, Option<i64>) = conn.query_row(
            "SELECT COUNT(*), COUNT(DISTINCT norad), MIN(first_seen_us), MAX(first_seen_us) FROM elsets",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )?;
        let fetches: i64 = conn.query_row("SELECT COUNT(*) FROM fetches", [], |r| r.get(0))?;
        Ok(HistoryUsage {
            elsets: elsets as u64,
            objects: objects as u64,
            fetches: fetches as u64,
            bytes: Self::db_bytes(&conn)?,
            first_seen_min: smin.map(from_us),
            first_seen_max: smax.map(from_us),
        })
    }

    /// 按保留时段与空间上限清理。每个物体历元最新的一条永不删除。返回删除的根数条数。
    pub fn prune(&self, now: DateTime<Utc>) -> Result<u64, ArchiveError> {
        // 可删除的行:不是本物体历元最新的那条
        const DELETABLE: &str = "id NOT IN (SELECT id FROM (
              SELECT id, ROW_NUMBER() OVER (PARTITION BY norad ORDER BY epoch_us DESC, source_rank ASC) AS rn
              FROM elsets) WHERE rn = 1)";
        let conn = self.lock();
        let mut deleted: u64 = 0;
        if self.cfg.retain_days > 0 {
            let cutoff = us(now - chrono::Duration::days(i64::from(self.cfg.retain_days)));
            deleted += conn.execute(
                &format!("DELETE FROM elsets WHERE first_seen_us < ?1 AND {DELETABLE}"),
                params![cutoff],
            )? as u64;
            conn.execute("DELETE FROM fetches WHERE fetched_at_us < ?1", params![cutoff])?;
        }
        if self.cfg.max_total_mb > 0 {
            let limit = self.cfg.max_total_mb * 1024 * 1024;
            for _ in 0..50 {
                if Self::db_bytes(&conn)? <= limit {
                    break;
                }
                let total: i64 = conn.query_row("SELECT COUNT(*) FROM elsets", [], |r| r.get(0))?;
                let batch = (total / 20).max(500);
                let n = conn.execute(
                    &format!(
                        "DELETE FROM elsets WHERE id IN (SELECT id FROM elsets WHERE {DELETABLE}
                         ORDER BY first_seen_us ASC LIMIT ?1)"
                    ),
                    params![batch],
                )? as u64;
                deleted += n;
                conn.execute_batch("PRAGMA incremental_vacuum;")?;
                if n == 0 {
                    tracing::warn!("历史库已无可删除的根数,但仍超过空间上限");
                    break;
                }
            }
        }
        if deleted > 0 {
            conn.execute_batch("PRAGMA incremental_vacuum;")?;
            tracing::info!(deleted, "已清理旧根数");
        }
        Ok(deleted)
    }

    /// 某物体的全部历史根数(历元从新到旧)。
    pub fn object_history(&self, norad: u32, limit: usize) -> Result<Vec<ElsetInfo>, ArchiveError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT epoch_us, first_seen_us, source FROM elsets WHERE norad = ?1 ORDER BY epoch_us DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![i64::from(norad), limit as i64], |r| {
            Ok(ElsetInfo {
                epoch: from_us(r.get(0)?),
                first_seen: from_us(r.get(1)?),
                source: r.get(2)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// 最早的获取时间(用于提示"该时刻之前没有数据")。
    pub fn earliest_first_seen(&self) -> Result<Option<DateTime<Utc>>, ArchiveError> {
        let conn = self.lock();
        let v: Option<i64> = conn
            .query_row("SELECT MIN(first_seen_us) FROM elsets", [], |r| r.get(0))
            .optional()?
            .flatten();
        Ok(v.map(from_us))
    }
}
