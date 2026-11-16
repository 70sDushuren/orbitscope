//! HTTP 工具:统一 User-Agent、超时、条件请求、有限次重试,以及"缓存优先"的获取流程。

use crate::cache::{Cache, CacheMeta};
use crate::error::SourceError;
use chrono::{DateTime, Utc};
use reqwest::header::{ETAG, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED, RETRY_AFTER};
use reqwest::{RequestBuilder, StatusCode};
use serde::Serialize;
use std::future::Future;
use std::time::Duration;

/// 统一 User-Agent。不包含任何个人信息。
pub const USER_AGENT: &str = concat!("orbitscope/", env!("CARGO_PKG_VERSION"));

/// 构建 HTTP 客户端。`cookies` 为 true 时启用会话 cookie(Space-Track 登录需要)。
pub fn build_client(cookies: bool) -> Result<reqwest::Client, SourceError> {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(20))
        .timeout(Duration::from_secs(180))
        .cookie_store(cookies)
        .build()
        .map_err(|e| SourceError::Config(format!("创建 HTTP 客户端失败: {e}")))
}

/// 重试策略。数据源政策会惩罚大量错误请求,因此默认只重试 1 次。
#[derive(Debug, Clone, Copy)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub base_delay: Duration,
    /// `Retry-After` 的上限
    pub max_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 2,
            base_delay: Duration::from_secs(5),
            max_delay: Duration::from_secs(300),
        }
    }
}

impl RetryPolicy {
    /// 不重试、无延迟(测试用)。
    pub fn none() -> Self {
        Self {
            max_attempts: 1,
            base_delay: Duration::ZERO,
            max_delay: Duration::ZERO,
        }
    }
}

/// 一次网络获取的结果。
#[derive(Debug)]
pub enum Fetched {
    Body {
        body: String,
        etag: Option<String>,
        last_modified: Option<String>,
    },
    NotModified,
}

/// 发送请求(由 `make` 每次重新构造),处理条件请求头、状态码与重试。
pub async fn send_text(
    url: &str,
    make: impl Fn() -> RequestBuilder,
    cond: Option<&CacheMeta>,
    retry: RetryPolicy,
) -> Result<Fetched, SourceError> {
    let mut attempt = 0;
    loop {
        attempt += 1;
        let mut req = make();
        if let Some(m) = cond {
            if let Some(etag) = &m.etag {
                req = req.header(IF_NONE_MATCH, etag);
            }
            if let Some(lm) = &m.last_modified {
                req = req.header(IF_MODIFIED_SINCE, lm);
            }
        }

        let (err, retry_after) = match req.send().await {
            Err(e) => (SourceError::http(url, e), None),
            Ok(resp) => {
                let status = resp.status();
                let header = |name| {
                    resp.headers()
                        .get(name)
                        .and_then(|v| v.to_str().ok())
                        .map(String::from)
                };
                let etag = header(ETAG);
                let last_modified = header(LAST_MODIFIED);
                let retry_after = header(RETRY_AFTER).and_then(|s| s.trim().parse::<u64>().ok());
                if status == StatusCode::NOT_MODIFIED {
                    return Ok(Fetched::NotModified);
                }
                if status.is_success() {
                    match resp.text().await {
                        Ok(body) => {
                            return Ok(Fetched::Body {
                                body,
                                etag,
                                last_modified,
                            });
                        }
                        Err(e) => (SourceError::http(url, e), None),
                    }
                } else {
                    (
                        SourceError::Status {
                            url: url.to_string(),
                            status: status.as_u16(),
                        },
                        retry_after,
                    )
                }
            }
        };

        if attempt >= retry.max_attempts || !err.is_transient() {
            return Err(err);
        }
        let backoff = retry.base_delay * 2u32.saturating_pow(attempt - 1);
        let delay = retry_after
            .map_or(backoff, Duration::from_secs)
            .min(retry.max_delay);
        tracing::warn!(url, attempt, ?delay, error = %err, "请求失败,稍后重试");
        tokio::time::sleep(delay).await;
    }
}

/// 获取模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum FetchMode {
    /// 缓存未过期则用缓存,否则联网(失败时回退到旧缓存)
    #[default]
    Normal,
    /// 只用缓存,不联网
    CacheOnly,
}

/// 数据来自哪里。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Origin {
    /// 本次从网络下载
    Network,
    /// 服务器返回 304,沿用缓存
    NotModified,
    /// 缓存仍在刷新间隔内,未联网
    Cache { age_secs: i64 },
    /// 联网失败,回退到旧缓存
    StaleCache { age_secs: i64, error: String },
}

/// 单个资源(URL)的获取结果。
#[derive(Debug, Clone, Serialize)]
pub struct ResourceStatus {
    pub key: String,
    pub url: String,
    pub origin: Origin,
    pub fetched_at: DateTime<Utc>,
    pub bytes: u64,
}

/// 缓存优先的获取流程。
///
/// - `CacheOnly`:有缓存则返回,否则 `NoCache`;
/// - `Normal`:缓存年龄 < `refresh` 时直接返回缓存;否则调用 `network(cond)` 联网
///   (带 ETag / Last-Modified 条件头),成功则写缓存;失败且有旧缓存时回退。
pub async fn cached_fetch<F, Fut>(
    cache: &Cache,
    key: &str,
    url: &str,
    refresh: Duration,
    mode: FetchMode,
    now: DateTime<Utc>,
    network: F,
) -> Result<(String, ResourceStatus), SourceError>
where
    F: FnOnce(Option<CacheMeta>) -> Fut,
    Fut: Future<Output = Result<Fetched, SourceError>>,
{
    let cached = cache.read(key).await?;
    let status = |origin: Origin, meta: &CacheMeta| ResourceStatus {
        key: key.to_string(),
        url: url.to_string(),
        origin,
        fetched_at: meta.fetched_at,
        bytes: meta.bytes,
    };

    if let Some((meta, body)) = &cached {
        let age = meta.age_secs(now);
        let fresh = age >= 0 && (age as u64) < refresh.as_secs() && meta.url == url;
        if mode == FetchMode::CacheOnly || fresh {
            tracing::debug!(key, age, "使用缓存");
            return Ok((body.clone(), status(Origin::Cache { age_secs: age }, meta)));
        }
    } else if mode == FetchMode::CacheOnly {
        return Err(SourceError::NoCache(key.to_string()));
    }

    // 只有 URL 未变时才发条件请求
    let cond = cached
        .as_ref()
        .filter(|(m, _)| m.url == url)
        .map(|(m, _)| m.clone());
    match network(cond).await {
        Ok(Fetched::Body {
            body,
            etag,
            last_modified,
        }) => {
            let meta = CacheMeta {
                url: url.to_string(),
                fetched_at: now,
                etag,
                last_modified,
                bytes: body.len() as u64,
            };
            if let Err(e) = cache.write(key, &meta, &body).await {
                tracing::warn!(key, error = %e, "写缓存失败");
            }
            tracing::info!(key, bytes = meta.bytes, "已下载");
            Ok((body, status(Origin::Network, &meta)))
        }
        Ok(Fetched::NotModified) => match cached {
            Some((mut meta, body)) => {
                meta.fetched_at = now;
                if let Err(e) = cache.write_meta(key, &meta).await {
                    tracing::warn!(key, error = %e, "写缓存元数据失败");
                }
                Ok((body, status(Origin::NotModified, &meta)))
            }
            None => Err(SourceError::Parse {
                what: key.to_string(),
                message: "收到 304 但没有缓存".into(),
            }),
        },
        Err(e) => match cached {
            Some((meta, body)) => {
                let age = meta.age_secs(now);
                tracing::warn!(key, age, error = %e, "联网失败,使用旧缓存");
                Ok((
                    body,
                    status(
                        Origin::StaleCache {
                            age_secs: age,
                            error: e.to_string(),
                        },
                        &meta,
                    ),
                ))
            }
            None => Err(e),
        },
    }
}
