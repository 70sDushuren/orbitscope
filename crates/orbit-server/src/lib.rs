//! # orbit-server
//!
//! OrbitScope 服务端:
//! - 启动时加载编目,之后每小时在随机分钟触发刷新(真正联网与否由数据源缓存决定);
//! - REST API(`/api/v1/...`,见 [`api`]);
//! - WebSocket 实时位置推送(`/api/v1/stream`,见 [`ws`] 与 [`frame`])。

pub mod api;
pub mod frame;
pub mod query;
pub mod scheduler;
pub mod state;
pub mod web;
pub mod ws;

pub use state::{
    AppState, DEFAULT_CESIUM_URL, PersistedSettings, RefreshKind, RuntimeSettings, ServerOptions,
};

use orbit_sources::SourceSet;
use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::mpsc;

#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    #[error("无法监听 {addr}: {source}")]
    Bind {
        addr: SocketAddr,
        source: std::io::Error,
    },
    #[error("服务运行失败: {0}")]
    Io(#[from] std::io::Error),
}

/// 绑定 `options.bind` 并运行,直到 `shutdown` 完成。
pub async fn serve(
    options: ServerOptions,
    sources: SourceSet,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<(), ServerError> {
    let addr = options.bind;
    let listener = TcpListener::bind(addr)
        .await
        .map_err(|source| ServerError::Bind { addr, source })?;
    serve_with_listener(listener, options, sources, shutdown).await
}

/// 使用已绑定的监听器运行(测试中可绑定端口 0)。
pub async fn serve_with_listener(
    listener: TcpListener,
    options: ServerOptions,
    sources: SourceSet,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<(), ServerError> {
    let local = listener.local_addr()?;
    if !local.ip().is_loopback() && options.api_token.is_none() {
        tracing::warn!(%local, "监听非本机地址且未设置 API token:写操作接口将拒绝非本机请求");
    }

    let (refresh_tx, refresh_rx) = mpsc::channel(1);
    let state = Arc::new(AppState::new(options, refresh_tx));
    let minute = state.options.refresh_minute.unwrap_or_else(|| {
        scheduler::pick_refresh_minute(std::hash::BuildHasher::hash_one(
            &std::collections::hash_map::RandomState::new(),
            local.port(),
        ))
    });
    state.update_schedule(|s| s.refresh_minute = minute % 60);

    let sources = Arc::new(sources);
    // 首次加载(新鲜缓存直接使用,否则联网)
    scheduler::refresh_once(&state, &sources, RefreshKind::Collect).await;
    tracing::info!(objects = state.catalog.load().catalog.len(), %local, "OrbitScope 服务已启动");

    let sched = tokio::spawn(scheduler::run_scheduler(state.clone(), sources, refresh_rx));
    let ticker = tokio::spawn(scheduler::run_ticker(state.clone()));

    let app = api::router(state.clone());
    let st = state.clone();
    let result = axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
        .with_graceful_shutdown(async move {
            shutdown.await;
            tracing::info!("正在关闭…");
            st.shutdown.send_replace(true);
        })
        .await;

    state.shutdown.send_replace(true);
    sched.abort();
    ticker.abort();
    result.map_err(ServerError::from)
}
