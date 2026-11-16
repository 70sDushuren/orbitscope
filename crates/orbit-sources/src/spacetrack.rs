//! Space-Track 数据源(需免费账号)。
//!
//! 依据 Space-Track 官方 API 使用规定(2026-11 核实):
//! - 全局限速:< 30 次/分钟 且 < 300 次/小时(本实现默认 20 / 200 留余量);
//! - GP:**每小时最多 1 次**,在非整点/半点的随机分钟发起(调度在服务端实现);
//!   查询需带 `/decay_date/null-val/epoch/%3Enow-10/`,只取可外推的在轨物体;
//! - 不要逐个物体查询;一次查询取全量。
//!
//! 认证:`POST /ajaxauth/login`(表单字段 `identity`、`password`),之后用会话 cookie。

use crate::cache::Cache;
use crate::error::SourceError;
use crate::http::{RetryPolicy, cached_fetch, send_text};
use crate::ratelimit::RateLimiter;
use crate::{FetchMode, SourceBatch};
use chrono::{DateTime, Utc};
use orbit_core::{DataSource, parse_omm_json};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::time::Duration;
use tokio::sync::Mutex;

/// GP 刷新间隔硬下限(分钟),来自官方规定。
pub const MIN_REFRESH_MINUTES: u64 = 60;
/// 凭据环境变量名。
pub const ENV_USER: &str = "ORBITSCOPE_SPACETRACK_USER";
pub const ENV_PASSWORD: &str = "ORBITSCOPE_SPACETRACK_PASSWORD";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SpaceTrackConfig {
    pub enabled: bool,
    pub base_url: String,
    pub refresh_minutes: u64,
    /// 服务端预筛:只取近地点高度低于该值(km)的物体,减少下载量。`None` 表示全量
    pub max_periapsis_km: Option<f64>,
    /// 只取 epoch 在最近 N 天内的根数(官方建议 10)
    pub max_epoch_age_days: u32,
    pub requests_per_minute: usize,
    pub requests_per_hour: usize,
}

impl Default for SpaceTrackConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            base_url: "https://www.space-track.org".into(),
            refresh_minutes: 60,
            max_periapsis_km: Some(2_000.0),
            max_epoch_age_days: 10,
            requests_per_minute: 20,
            requests_per_hour: 200,
        }
    }
}

/// 账号凭据。`Debug` / `Display` 不会输出密码。
#[derive(Clone)]
pub struct Credentials {
    identity: String,
    password: String,
}

impl Credentials {
    pub fn new(identity: impl Into<String>, password: impl Into<String>) -> Self {
        Self {
            identity: identity.into(),
            password: password.into(),
        }
    }

    /// 从环境变量 [`ENV_USER`] / [`ENV_PASSWORD`] 读取;任一缺失返回 `None`。
    pub fn from_env() -> Option<Self> {
        let user = std::env::var(ENV_USER).ok().filter(|s| !s.is_empty())?;
        let pass = std::env::var(ENV_PASSWORD).ok().filter(|s| !s.is_empty())?;
        Some(Self::new(user, pass))
    }
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credentials")
            .field("identity", &self.identity)
            .field("password", &"***")
            .finish()
    }
}

#[derive(Debug)]
pub struct SpaceTrackSource {
    cfg: SpaceTrackConfig,
    creds: Credentials,
    client: reqwest::Client,
    cache: Cache,
    limiter: RateLimiter,
    logged_in: Mutex<bool>,
    retry: RetryPolicy,
}

impl SpaceTrackSource {
    pub fn new(cfg: SpaceTrackConfig, creds: Credentials, cache: Cache) -> Result<Self, SourceError> {
        if cfg.max_periapsis_km.is_some_and(|v| !v.is_finite() || v <= 0.0) {
            return Err(SourceError::Config("max_periapsis_km 必须为正数".into()));
        }
        // 限速不得高于官方上限
        let per_min = cfg.requests_per_minute.clamp(1, 29);
        let per_hour = cfg.requests_per_hour.clamp(1, 299);
        Ok(Self {
            limiter: RateLimiter::per_minute_and_hour(per_min, per_hour),
            client: crate::http::build_client(true)?,
            cfg,
            creds,
            cache,
            logged_in: Mutex::new(false),
            retry: RetryPolicy::default(),
        })
    }

    pub fn with_retry(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }

    fn base(&self) -> &str {
        self.cfg.base_url.trim_end_matches('/')
    }

    /// 全量 GP 查询 URL。
    pub fn gp_url(&self) -> String {
        let mut q = format!(
            "{}/basicspacedata/query/class/gp/decay_date/null-val/epoch/%3Enow-{}/",
            self.base(),
            self.cfg.max_epoch_age_days.max(1)
        );
        if let Some(p) = self.cfg.max_periapsis_km {
            q.push_str(&format!("PERIAPSIS/%3C{p}/"));
        }
        q.push_str("orderby/norad_cat_id/format/json");
        q
    }

    async fn login(&self) -> Result<(), SourceError> {
        let url = format!("{}/ajaxauth/login", self.base());
        self.limiter.acquire().await;
        let form = [
            ("identity", self.creds.identity.as_str()),
            ("password", self.creds.password.as_str()),
        ];
        let resp = self
            .client
            .post(&url)
            .form(&form)
            .send()
            .await
            .map_err(|e| SourceError::Login(format!("请求失败: {}", e.without_url())))?;
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        // 成功时返回 ""(空 JSON 字符串);失败时返回 {"Login":"Failed"}
        if !status.is_success() || body.contains("Failed") {
            return Err(SourceError::Login(format!(
                "HTTP {}; 请检查账号密码",
                status.as_u16()
            )));
        }
        tracing::info!("Space-Track 登录成功");
        Ok(())
    }

    async fn ensure_login(&self) -> Result<(), SourceError> {
        let mut logged = self.logged_in.lock().await;
        if !*logged {
            self.login().await?;
            *logged = true;
        }
        Ok(())
    }

    async fn query(&self, url: &str) -> Result<crate::http::Fetched, SourceError> {
        for attempt in 0..2 {
            self.ensure_login().await?;
            self.limiter.acquire().await;
            let client = self.client.clone();
            let r = send_text(url, || client.get(url), None, self.retry).await;
            match r {
                // 会话失效:重新登录一次
                Err(SourceError::Status { status: 401, .. }) if attempt == 0 => {
                    *self.logged_in.lock().await = false;
                    continue;
                }
                Ok(crate::http::Fetched::Body { body, .. })
                    if attempt == 0 && body.trim_start().starts_with('{') && body.contains("\"error\"") =>
                {
                    tracing::warn!("Space-Track 返回错误对象,重新登录后重试");
                    *self.logged_in.lock().await = false;
                    continue;
                }
                other => return other,
            }
        }
        Err(SourceError::Login("重新登录后仍无法查询".into()))
    }

    /// 获取 GP 全量(按配置预筛)。刷新间隔 < 1 小时时使用缓存,绝不提前联网。
    pub async fn fetch(&self, mode: FetchMode, now: DateTime<Utc>) -> Result<SourceBatch, SourceError> {
        let url = self.gp_url();
        let refresh = Duration::from_secs(60 * self.cfg.refresh_minutes.max(MIN_REFRESH_MINUTES));
        let (body, status) = cached_fetch(&self.cache, "spacetrack/gp", &url, refresh, mode, now, |_| {
            self.query(&url)
        })
        .await?;

        let mut batch = SourceBatch::new(DataSource::SpaceTrack, "space-track");
        let fetched_at = status.fetched_at;
        batch.resources.push(status);
        let out = parse_omm_json(&body, DataSource::SpaceTrack).map_err(|e| SourceError::Parse {
            what: "space-track gp".into(),
            message: e.to_string(),
        })?;
        batch.record_errors = out.errors.len();
        batch.records = out.records;
        for r in &mut batch.records {
            r.fetched_at = Some(fetched_at);
        }
        Ok(batch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_hides_password() {
        let c = Credentials::new("user@example.com", "hunter2");
        let s = format!("{c:?}");
        assert!(!s.contains("hunter2"));
        assert!(s.contains("***"));
    }

    #[test]
    fn gp_url_format() {
        let dir = tempfile::tempdir().unwrap();
        let src = SpaceTrackSource::new(
            SpaceTrackConfig::default(),
            Credentials::new("u", "p"),
            Cache::new(dir.path()),
        )
        .unwrap();
        assert_eq!(
            src.gp_url(),
            "https://www.space-track.org/basicspacedata/query/class/gp/decay_date/null-val/epoch/%3Enow-10/PERIAPSIS/%3C2000/orderby/norad_cat_id/format/json"
        );
    }
}
