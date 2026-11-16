//! 完整快照:每次网络下载的原始内容以 gzip 保存,带时间戳,不覆盖;按文件数与总空间上限淘汰最旧的。
//!
//! 布局:`<dir>/<资源键>/<YYYYmmddTHHMMSSZ>[-n].body.gz`

use crate::config::SnapshotConfig;
use crate::error::ArchiveError;
use chrono::{DateTime, NaiveDateTime, Utc};
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use serde::Serialize;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const SUFFIX: &str = ".body.gz";
const TS_FORMAT: &str = "%Y%m%dT%H%M%SZ";

/// 一个快照文件。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SnapshotFile {
    pub resource: String,
    pub fetched_at: DateTime<Utc>,
    pub bytes: u64,
    #[serde(skip)]
    pub path: PathBuf,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct SnapshotUsage {
    pub files: usize,
    pub bytes: u64,
    pub oldest: Option<DateTime<Utc>>,
    pub newest: Option<DateTime<Utc>>,
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

fn io(path: &Path, e: impl std::fmt::Display) -> ArchiveError {
    ArchiveError::Io {
        path: path.display().to_string(),
        message: e.to_string(),
    }
}

#[derive(Debug, Clone)]
pub struct SnapshotStore {
    dir: PathBuf,
    cfg: SnapshotConfig,
}

impl SnapshotStore {
    pub fn new(dir: impl Into<PathBuf>, cfg: SnapshotConfig) -> Self {
        Self { dir: dir.into(), cfg }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// 保存一份快照,返回文件路径。
    pub fn save(
        &self,
        resource: &str,
        fetched_at: DateTime<Utc>,
        body: &[u8],
    ) -> Result<PathBuf, ArchiveError> {
        let sub = self.dir.join(sanitize(resource));
        std::fs::create_dir_all(&sub).map_err(|e| io(&sub, e))?;
        let ts = fetched_at.format(TS_FORMAT).to_string();
        let mut path = sub.join(format!("{ts}{SUFFIX}"));
        let mut n = 1;
        while path.exists() {
            path = sub.join(format!("{ts}-{n}{SUFFIX}"));
            n += 1;
        }
        let tmp = path.with_extension("tmp");
        {
            let f = std::fs::File::create(&tmp).map_err(|e| io(&tmp, e))?;
            let mut enc = GzEncoder::new(f, Compression::default());
            enc.write_all(body).map_err(|e| io(&tmp, e))?;
            enc.finish().map_err(|e| io(&tmp, e))?;
        }
        std::fs::rename(&tmp, &path).map_err(|e| io(&path, e))?;
        Ok(path)
    }

    /// 列出全部快照,按获取时间从旧到新。
    pub fn list(&self) -> Result<Vec<SnapshotFile>, ArchiveError> {
        let mut out = Vec::new();
        let Ok(dirs) = std::fs::read_dir(&self.dir) else {
            return Ok(out);
        };
        for d in dirs.flatten() {
            if !d.path().is_dir() {
                continue;
            }
            let resource = d.file_name().to_string_lossy().to_string();
            for f in std::fs::read_dir(d.path())
                .map_err(|e| io(&d.path(), e))?
                .flatten()
            {
                let name = f.file_name().to_string_lossy().to_string();
                let Some(stem) = name.strip_suffix(SUFFIX) else {
                    continue;
                };
                let ts = stem.split('-').next().unwrap_or(stem);
                let Ok(t) = NaiveDateTime::parse_from_str(ts, TS_FORMAT) else {
                    continue;
                };
                let bytes = f.metadata().map(|m| m.len()).unwrap_or(0);
                out.push(SnapshotFile {
                    resource: resource.clone(),
                    fetched_at: t.and_utc(),
                    bytes,
                    path: f.path(),
                });
            }
        }
        out.sort_by(|a, b| a.fetched_at.cmp(&b.fetched_at).then_with(|| a.path.cmp(&b.path)));
        Ok(out)
    }

    pub fn usage(&self) -> Result<SnapshotUsage, ArchiveError> {
        let files = self.list()?;
        Ok(SnapshotUsage {
            files: files.len(),
            bytes: files.iter().map(|f| f.bytes).sum(),
            oldest: files.first().map(|f| f.fetched_at),
            newest: files.last().map(|f| f.fetched_at),
        })
    }

    /// 读取快照内容(解压)。
    pub fn read(&self, file: &SnapshotFile) -> Result<String, ArchiveError> {
        let f = std::fs::File::open(&file.path).map_err(|e| io(&file.path, e))?;
        let mut s = String::new();
        GzDecoder::new(f)
            .read_to_string(&mut s)
            .map_err(|e| io(&file.path, e))?;
        Ok(s)
    }

    /// 按配置淘汰最旧的快照,返回删除的文件数与字节数。
    pub fn prune(&self) -> Result<(usize, u64), ArchiveError> {
        let files = self.list()?;
        let max_files = if self.cfg.max_files == 0 {
            usize::MAX
        } else {
            self.cfg.max_files
        };
        let max_bytes = if self.cfg.max_total_mb == 0 {
            u64::MAX
        } else {
            self.cfg.max_total_mb * 1024 * 1024
        };
        let mut count = files.len();
        let mut total: u64 = files.iter().map(|f| f.bytes).sum();
        let (mut del_n, mut del_b) = (0, 0);
        for f in &files {
            if count <= max_files && total <= max_bytes {
                break;
            }
            std::fs::remove_file(&f.path).map_err(|e| io(&f.path, e))?;
            count -= 1;
            total -= f.bytes;
            del_n += 1;
            del_b += f.bytes;
            // 删除空目录
            if let Some(parent) = f.path.parent()
                && std::fs::read_dir(parent)
                    .map(|mut d| d.next().is_none())
                    .unwrap_or(false)
            {
                let _ = std::fs::remove_dir(parent);
            }
        }
        if del_n > 0 {
            tracing::info!(files = del_n, bytes = del_b, "已清理旧快照");
        }
        Ok((del_n, del_b))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn t(h: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 16, h, 0, 0).unwrap()
    }

    #[test]
    fn save_list_read_and_prune_by_count() {
        let dir = tempfile::tempdir().unwrap();
        let s = SnapshotStore::new(
            dir.path(),
            SnapshotConfig {
                enabled: true,
                max_files: 3,
                max_total_mb: 0,
            },
        );
        for h in [3, 1, 2, 4, 5] {
            s.save("celestrak/gp-active", t(h), format!("body {h}").as_bytes())
                .unwrap();
        }
        s.save("celestrak/gp-active", t(5), b"same second").unwrap();
        let all = s.list().unwrap();
        assert_eq!(all.len(), 6);
        assert_eq!(all[0].fetched_at, t(1));
        assert_eq!(s.read(&all[0]).unwrap(), "body 1");

        let (n, _) = s.prune().unwrap();
        assert_eq!(n, 3);
        let left: Vec<_> = s.list().unwrap().iter().map(|f| f.fetched_at).collect();
        assert_eq!(left, vec![t(4), t(5), t(5)], "保留最新的 3 个");
    }

    #[test]
    fn prune_by_size_across_resources() {
        let dir = tempfile::tempdir().unwrap();
        let s = SnapshotStore::new(
            dir.path(),
            SnapshotConfig {
                enabled: true,
                max_files: 0,
                max_total_mb: 1,
            },
        );
        // 不可压缩的数据,每份约 0.6 MB
        let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
        let noise: Vec<u8> = (0..600_000)
            .map(|_| {
                // xorshift64:不可压缩的伪随机数据
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                (x >> 24) as u8
            })
            .collect();
        s.save("a", t(1), &noise).unwrap();
        s.save("b", t(2), &noise).unwrap();
        s.save("a", t(3), &noise).unwrap();
        s.prune().unwrap();
        let u = s.usage().unwrap();
        assert_eq!(u.files, 1);
        assert!(u.bytes <= 1024 * 1024);
        assert_eq!(u.newest, Some(t(3)));
        // 资源 b 的目录已被删除
        assert!(!dir.path().join("b").exists());
    }
}
