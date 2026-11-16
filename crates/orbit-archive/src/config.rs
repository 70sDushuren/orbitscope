//! 归档配置与时刻策略。

use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

/// 指定时刻 T 时如何为每个物体选取根数。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TimePolicy {
    /// T 时刻已获取到的最新根数(`first_seen ≤ T` 中历元最新者),结果可复现
    #[default]
    KnownAt,
    /// 历元最接近 T 的根数(可能是 T 之后才获取到的)
    NearestEpoch,
    /// 历元最新的根数(不考虑 T)
    Latest,
}

impl TimePolicy {
    pub const ALL: [TimePolicy; 3] = [TimePolicy::KnownAt, TimePolicy::NearestEpoch, TimePolicy::Latest];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::KnownAt => "known-at",
            Self::NearestEpoch => "nearest-epoch",
            Self::Latest => "latest",
        }
    }

    /// 中文说明。
    pub fn label(self) -> &'static str {
        match self {
            Self::KnownAt => "该时刻已获取到的数据",
            Self::NearestEpoch => "历元最接近该时刻的数据",
            Self::Latest => "最新数据",
        }
    }
}

impl fmt::Display for TimePolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for TimePolicy {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|p| p.as_str().eq_ignore_ascii_case(s.trim()))
            .ok_or_else(|| format!("未知的时刻策略 {s:?}(可选 known-at, nearest-epoch, latest)"))
    }
}

/// 完整快照的容量控制。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SnapshotConfig {
    /// 是否保存每次网络下载的原始文件
    pub enabled: bool,
    /// 最多保留的快照文件数(0 = 不限)
    pub max_files: usize,
    /// 快照总空间上限(MB,0 = 不限)
    pub max_total_mb: u64,
}

impl Default for SnapshotConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_files: 200,
            max_total_mb: 1_024,
        }
    }
}

/// 根数历史库的保留控制。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HistoryConfig {
    /// 是否记录根数历史(关闭后只使用缓存中的最新数据,即 v0.1 行为)
    pub enabled: bool,
    /// 保留最近多少天内获取的根数(0 = 不限)
    pub retain_days: u32,
    /// 历史库空间上限(MB,0 = 不限)
    pub max_total_mb: u64,
}

impl Default for HistoryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            retain_days: 0,
            max_total_mb: 4_096,
        }
    }
}

/// `[archive]` 配置段。
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ArchiveConfig {
    /// 归档目录;默认为用户数据目录下的 `orbitscope`
    pub dir: Option<PathBuf>,
    /// 默认时刻策略
    pub time_policy: TimePolicy,
    pub snapshots: SnapshotConfig,
    pub history: HistoryConfig,
}

impl ArchiveConfig {
    /// 默认归档目录。
    pub fn default_dir() -> Option<PathBuf> {
        dirs::data_dir().map(|d| d.join("orbitscope"))
    }

    pub fn resolved_dir(&self) -> Option<PathBuf> {
        self.dir.clone().or_else(Self::default_dir)
    }
}

/// 采集策略(`[collect] mode`)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CollectMode {
    /// Web 服务进程负责采集
    #[default]
    Serve,
    /// 独立常驻采集进程 `orbitscope collect`
    Daemon,
    /// 系统定时任务调用 `orbitscope fetch`
    External,
}

impl CollectMode {
    pub const ALL: [CollectMode; 3] = [CollectMode::Serve, CollectMode::Daemon, CollectMode::External];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Serve => "serve",
            Self::Daemon => "daemon",
            Self::External => "external",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Serve => "由 Web 服务采集",
            Self::Daemon => "由常驻采集进程(orbitscope collect)采集",
            Self::External => "由系统定时任务(orbitscope fetch)采集",
        }
    }

    /// Web 服务是否应联网采集。
    pub fn serve_collects(self) -> bool {
        self == Self::Serve
    }
}

impl fmt::Display for CollectMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for CollectMode {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|m| m.as_str().eq_ignore_ascii_case(s.trim()))
            .ok_or_else(|| format!("未知的采集策略 {s:?}(可选 serve, daemon, external)"))
    }
}

/// `[collect]` 配置段。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CollectConfig {
    pub mode: CollectMode,
    /// 非 serve 模式下,Web 服务从本地历史重新加载的间隔(分钟)
    pub reload_minutes: u64,
}

impl Default for CollectConfig {
    fn default() -> Self {
        Self {
            mode: CollectMode::Serve,
            reload_minutes: 5,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_display() {
        for p in TimePolicy::ALL {
            assert_eq!(p.as_str().parse::<TimePolicy>().unwrap(), p);
        }
        for m in CollectMode::ALL {
            assert_eq!(m.as_str().parse::<CollectMode>().unwrap(), m);
        }
        assert!("nope".parse::<TimePolicy>().is_err());
        assert!(CollectMode::Serve.serve_collects());
        assert!(!CollectMode::External.serve_collects());
        let c: ArchiveConfig =
            serde_json::from_str(r#"{"time_policy":"nearest-epoch","snapshots":{"max_files":3}}"#).unwrap();
        assert_eq!(c.time_policy, TimePolicy::NearestEpoch);
        assert_eq!(c.snapshots.max_files, 3);
        assert_eq!(c.snapshots.max_total_mb, 1_024);
    }
}
