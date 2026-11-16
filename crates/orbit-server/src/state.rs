//! 服务端共享状态。

use arc_swap::ArcSwap;
use chrono::{DateTime, Utc};
use orbit_core::{BuildReport, Catalog, Filter};
use orbit_sources::SourceStatus;
use serde::Serialize;
use std::net::SocketAddr;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;
use tokio::sync::{mpsc, watch};

/// 服务端选项。
#[derive(Debug, Clone)]
pub struct ServerOptions {
    pub bind: SocketAddr,
    /// 位置推送周期
    pub tick: Duration,
    pub max_ws_clients: usize,
    /// 写操作(修改默认过滤、手动刷新)的 Bearer token;为 `None` 时只允许本机地址执行写操作
    pub api_token: Option<String>,
    /// 默认过滤条件(客户端未指定时使用)
    pub default_filter: Filter,
    /// 服务端"当前时间"相对系统时间的偏移(用于模拟/回放与测试)
    pub time_offset: chrono::Duration,
    /// 每小时的刷新分钟(0–59);`None` 时随机选取,避开整点与半点
    pub refresh_minute: Option<u32>,
}

impl Default for ServerOptions {
    fn default() -> Self {
        Self {
            bind: SocketAddr::from(([127, 0, 0, 1], 8080)),
            tick: Duration::from_secs(1),
            max_ws_clients: 64,
            api_token: None,
            default_filter: Filter::default(),
            time_offset: chrono::Duration::zero(),
            refresh_minute: None,
        }
    }
}

/// 一个编目快照及其元信息。
#[derive(Debug)]
pub struct CatalogState {
    pub catalog: Catalog,
    /// 每次替换编目加 1;WS 帧与元数据用它对齐下标
    pub generation: u64,
    pub loaded_at: DateTime<Utc>,
    pub report: BuildReport,
}

impl CatalogState {
    pub fn empty() -> Self {
        let (catalog, report) = Catalog::build(Vec::new(), Utc::now());
        Self {
            catalog,
            generation: 0,
            loaded_at: Utc::now(),
            report,
        }
    }
}

/// 某时刻全部物体的位置(下标与 `generation` 对应的编目一致)。外推失败的物体为 NaN。
#[derive(Debug)]
pub struct PositionFrame {
    pub generation: u64,
    pub time: DateTime<Utc>,
    pub ecef_km: Vec<[f32; 3]>,
    pub alt_km: Vec<f32>,
}

/// 状态接口中的调度信息。
#[derive(Debug, Clone, Default, Serialize)]
pub struct ScheduleInfo {
    pub refresh_minute: u32,
    pub next_refresh: Option<DateTime<Utc>>,
    pub last_refresh: Option<DateTime<Utc>>,
    pub refreshing: bool,
}

pub struct AppState {
    pub options: ServerOptions,
    pub catalog: ArcSwap<CatalogState>,
    pub default_filter: RwLock<Filter>,
    /// 最新位置帧;只有存在订阅者时 ticker 才计算
    pub positions: watch::Sender<Option<Arc<PositionFrame>>>,
    /// 编目代数变化通知
    pub catalog_gen: watch::Sender<u64>,
    /// 手动刷新请求
    pub refresh_tx: mpsc::Sender<()>,
    pub schedule: Mutex<ScheduleInfo>,
    /// 最近一次获取时各数据源的状态
    pub source_status: Mutex<Vec<SourceStatus>>,
    pub ws_clients: AtomicUsize,
    pub started_at: DateTime<Utc>,
    /// 置为 true 时所有后台任务与 WS 连接退出
    pub shutdown: watch::Sender<bool>,
}

impl AppState {
    pub fn new(options: ServerOptions, refresh_tx: mpsc::Sender<()>) -> Self {
        let default_filter = RwLock::new(options.default_filter.clone());
        Self {
            options,
            catalog: ArcSwap::from_pointee(CatalogState::empty()),
            default_filter,
            positions: watch::channel(None).0,
            catalog_gen: watch::channel(0).0,
            refresh_tx,
            schedule: Mutex::new(ScheduleInfo::default()),
            source_status: Mutex::new(Vec::new()),
            ws_clients: AtomicUsize::new(0),
            started_at: Utc::now(),
            shutdown: watch::channel(false).0,
        }
    }

    /// 服务端"当前时间"(系统时间 + 偏移)。
    pub fn now(&self) -> DateTime<Utc> {
        Utc::now() + self.options.time_offset
    }

    pub fn default_filter(&self) -> Filter {
        self.default_filter
            .read()
            .map(|f| f.clone())
            .unwrap_or_else(|p| p.into_inner().clone())
    }

    pub fn set_default_filter(&self, f: Filter) {
        match self.default_filter.write() {
            Ok(mut g) => *g = f,
            Err(p) => *p.into_inner() = f,
        }
    }

    pub fn schedule_info(&self) -> ScheduleInfo {
        self.schedule
            .lock()
            .map(|s| s.clone())
            .unwrap_or_else(|p| p.into_inner().clone())
    }

    pub fn source_status(&self) -> Vec<SourceStatus> {
        self.source_status
            .lock()
            .map(|s| s.clone())
            .unwrap_or_else(|p| p.into_inner().clone())
    }

    pub fn set_source_status(&self, v: Vec<SourceStatus>) {
        match self.source_status.lock() {
            Ok(mut g) => *g = v,
            Err(p) => *p.into_inner() = v,
        }
    }

    pub fn update_schedule(&self, f: impl FnOnce(&mut ScheduleInfo)) {
        match self.schedule.lock() {
            Ok(mut g) => f(&mut g),
            Err(p) => f(&mut p.into_inner()),
        }
    }
}
