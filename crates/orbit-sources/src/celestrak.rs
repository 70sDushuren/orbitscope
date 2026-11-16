//! CelesTrak 数据源(无需账号)。
//!
//! - GP:`{base}/NORAD/elements/gp.php?GROUP={group}&FORMAT=json`
//! - SATCAT:`{base}{satcat_path}`(默认 `/pub/satcat.csv`)
//!
//! CelesTrak 的公开分组不覆盖全部碎片;需要全量编目时启用 Space-Track。
//! CelesTrak 按 IP 统计请求,重复下载未更新的数据或大量错误请求会被封禁,
//! 因此刷新间隔有硬下限 [`MIN_REFRESH_MINUTES`],且只重试 1 次。

use crate::cache::Cache;
use crate::error::SourceError;
use crate::http::{RetryPolicy, cached_fetch, send_text};
use crate::{FetchMode, SourceBatch};
use chrono::{DateTime, Utc};
use orbit_core::{DataSource, parse_omm_json};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// GP 刷新间隔硬下限(分钟)。
pub const MIN_REFRESH_MINUTES: u64 = 60;
/// SATCAT 刷新间隔硬下限(分钟)。
pub const MIN_SATCAT_REFRESH_MINUTES: u64 = 720;

/// 默认分组:在轨活跃物体 + 几次主要解体事件的碎片。
pub const DEFAULT_GROUPS: &[&str] = &[
    "active",
    "cosmos-1408-debris",
    "fengyun-1c-debris",
    "iridium-33-debris",
    "cosmos-2251-debris",
];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CelesTrakConfig {
    pub enabled: bool,
    pub base_url: String,
    pub groups: Vec<String>,
    pub refresh_minutes: u64,
    pub satcat: bool,
    pub satcat_path: String,
    pub satcat_refresh_minutes: u64,
}

impl Default for CelesTrakConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            base_url: "https://celestrak.org".into(),
            groups: DEFAULT_GROUPS.iter().map(|s| s.to_string()).collect(),
            refresh_minutes: 120,
            satcat: true,
            satcat_path: "/pub/satcat.csv".into(),
            satcat_refresh_minutes: 1_440,
        }
    }
}

fn is_valid_group(g: &str) -> bool {
    !g.is_empty()
        && g.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

#[derive(Debug)]
pub struct CelesTrakSource {
    cfg: CelesTrakConfig,
    client: reqwest::Client,
    cache: Cache,
    retry: RetryPolicy,
}

impl CelesTrakSource {
    pub fn new(cfg: CelesTrakConfig, cache: Cache) -> Result<Self, SourceError> {
        if let Some(bad) = cfg.groups.iter().find(|g| !is_valid_group(g)) {
            return Err(SourceError::Config(format!("非法的 CelesTrak 分组名: {bad:?}")));
        }
        if cfg.groups.is_empty() && !cfg.satcat {
            return Err(SourceError::Config("CelesTrak 未配置任何分组".into()));
        }
        Ok(Self {
            cfg,
            client: crate::http::build_client(false)?,
            cache,
            retry: RetryPolicy::default(),
        })
    }

    /// 覆盖重试策略(测试用)。
    pub fn with_retry(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }

    pub fn config(&self) -> &CelesTrakConfig {
        &self.cfg
    }

    fn base(&self) -> &str {
        self.cfg.base_url.trim_end_matches('/')
    }

    fn refresh(&self) -> Duration {
        Duration::from_secs(60 * self.cfg.refresh_minutes.max(MIN_REFRESH_MINUTES))
    }

    async fn get(
        &self,
        key: &str,
        url: &str,
        refresh: Duration,
        mode: FetchMode,
        now: DateTime<Utc>,
    ) -> Result<(String, crate::ResourceStatus), SourceError> {
        cached_fetch(&self.cache, key, url, refresh, mode, now, |cond| {
            let client = self.client.clone();
            let url = url.to_string();
            let retry = self.retry;
            async move { send_text(&url, || client.get(&url), cond.as_ref(), retry).await }
        })
        .await
    }

    /// 获取全部分组的 GP 与(可选)SATCAT。单个分组失败只记入 notes;全部失败才返回错误。
    pub async fn fetch(&self, mode: FetchMode, now: DateTime<Utc>) -> Result<SourceBatch, SourceError> {
        let mut batch = SourceBatch::new(DataSource::CelesTrak, "celestrak");
        let mut last_err = None;

        for group in &self.cfg.groups {
            let url = format!("{}/NORAD/elements/gp.php?GROUP={group}&FORMAT=json", self.base());
            let key = format!("celestrak/gp-{group}");
            match self.get(&key, &url, self.refresh(), mode, now).await {
                Ok((body, status)) => {
                    batch.resources.push(status);
                    // CelesTrak 对未知分组等情况返回纯文本(如 "No GP data found")
                    match parse_omm_json(&body, DataSource::CelesTrak) {
                        Ok(out) => {
                            batch.record_errors += out.errors.len();
                            batch.records.extend(out.records);
                        }
                        Err(e) => {
                            let snippet: String = body.chars().take(80).collect();
                            batch
                                .notes
                                .push(format!("分组 {group}: 返回内容不是 GP JSON ({e}): {snippet:?}"));
                        }
                    }
                }
                Err(e) => {
                    batch.notes.push(format!("分组 {group}: {e}"));
                    last_err = Some(e);
                }
            }
        }

        if self.cfg.satcat {
            let url = format!("{}{}", self.base(), self.cfg.satcat_path);
            let refresh =
                Duration::from_secs(60 * self.cfg.satcat_refresh_minutes.max(MIN_SATCAT_REFRESH_MINUTES));
            match self.get("celestrak/satcat.csv", &url, refresh, mode, now).await {
                Ok((body, status)) => {
                    batch.resources.push(status);
                    match crate::satcat::parse_satcat_csv(&body) {
                        Ok(out) => {
                            if out.bad_rows > 0 {
                                batch.notes.push(format!("SATCAT: {} 行无法解析", out.bad_rows));
                            }
                            batch.extra_meta = out.meta;
                        }
                        Err(e) => batch.notes.push(format!("SATCAT 解析失败: {e}")),
                    }
                }
                Err(e) => batch.notes.push(format!("SATCAT: {e}")),
            }
        }

        if batch.records.is_empty()
            && let Some(e) = last_err
        {
            return Err(e);
        }
        Ok(batch)
    }
}
