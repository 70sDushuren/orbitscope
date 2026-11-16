//! 服务端共享状态。

use arc_swap::ArcSwap;
use chrono::{DateTime, Utc};
use orbit_archive::{Archive, CollectConfig, CollectMode, Provenance, TimePolicy};
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
    /// 前端加载 CesiumJS 的基础 URL(以 `/` 结尾);设置了 `cesium_dir` 时被忽略
    pub cesium_base_url: String,
    /// 本地 CesiumJS `Build/Cesium` 目录;设置后以 `/cesium/` 提供,前端可完全离线
    pub cesium_dir: Option<std::path::PathBuf>,
    /// 历史归档;为 `None` 时只使用数据源的最新数据(v0.1 行为)
    pub archive: Option<Arc<Archive>>,
    /// 初始时刻策略
    pub time_policy: TimePolicy,
    /// 采集策略与本地重新加载间隔
    pub collect: CollectConfig,
    /// 离线:采集时只读缓存,不联网
    pub offline: bool,
    /// Web 界面所做设置的保存位置(`None` 时不保存,只在本次运行有效)
    pub settings_path: Option<std::path::PathBuf>,
}

/// 默认从 jsDelivr 加载的 CesiumJS 版本。
pub const DEFAULT_CESIUM_URL: &str = "https://cdn.jsdelivr.net/npm/cesium@1.146.0/Build/Cesium/";

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
            cesium_base_url: DEFAULT_CESIUM_URL.to_string(),
            cesium_dir: None,
            archive: None,
            time_policy: TimePolicy::default(),
            collect: CollectConfig::default(),
            offline: false,
            settings_path: None,
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
    /// 数据依据(来自历史库时)
    pub provenance: Option<Provenance>,
    /// 内容签名(各物体编号与历元),用于判断编目是否真正变化
    pub signature: u64,
}

impl CatalogState {
    pub fn empty() -> Self {
        let (catalog, report) = Catalog::build(Vec::new(), Utc::now());
        Self {
            catalog,
            generation: 0,
            loaded_at: Utc::now(),
            report,
            provenance: None,
            signature: 0,
        }
    }
}

/// 计算编目内容签名。
pub fn catalog_signature(cat: &Catalog) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    cat.len().hash(&mut h);
    for o in cat.objects() {
        o.norad_id.hash(&mut h);
        o.epoch.timestamp_micros().hash(&mut h);
        o.source.hash(&mut h);
    }
    h.finish()
}

/// 运行期可在 Web 界面切换的设置。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RuntimeSettings {
    pub time_policy: TimePolicy,
    pub collect_mode: CollectMode,
    /// 服务端时钟相对系统时间的偏移
    #[serde(skip)]
    pub time_offset: chrono::Duration,
}

/// 保存到磁盘的设置(时间不保存:重启后回到实时或 `--sim-time`)。
#[derive(Debug, Clone, PartialEq, Serialize, serde::Deserialize)]
pub struct PersistedSettings {
    pub time_policy: TimePolicy,
    pub collect_mode: CollectMode,
}

impl PersistedSettings {
    pub fn load(path: &std::path::Path) -> Option<Self> {
        let text = std::fs::read_to_string(path).ok()?;
        match serde_json::from_str(&text) {
            Ok(s) => Some(s),
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "忽略无法解析的界面设置文件");
                None
            }
        }
    }

    pub fn save(&self, path: &std::path::Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(
            &tmp,
            serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?,
        )?;
        std::fs::rename(tmp, path)
    }
}

/// 调度器的唤醒原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshKind {
    /// 联网采集(仅 serve 采集模式下真正联网)并重建编目
    Collect,
    /// 只从本地历史 / 缓存重建编目
    Reload,
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
    /// 下次联网采集(仅 serve 采集模式)
    pub next_refresh: Option<DateTime<Utc>>,
    /// 下次从本地重新加载
    pub next_reload: Option<DateTime<Utc>>,
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
    pub refresh_tx: mpsc::Sender<RefreshKind>,
    pub settings: RwLock<RuntimeSettings>,
    pub schedule: Mutex<ScheduleInfo>,
    /// 最近一次获取时各数据源的状态
    pub source_status: Mutex<Vec<SourceStatus>>,
    pub ws_clients: AtomicUsize,
    pub started_at: DateTime<Utc>,
    /// 置为 true 时所有后台任务与 WS 连接退出
    pub shutdown: watch::Sender<bool>,
}

impl AppState {
    pub fn new(options: ServerOptions, refresh_tx: mpsc::Sender<RefreshKind>) -> Self {
        let default_filter = RwLock::new(options.default_filter.clone());
        let settings = RwLock::new(RuntimeSettings {
            time_policy: options.time_policy,
            collect_mode: options.collect.mode,
            time_offset: options.time_offset,
        });
        Self {
            settings,
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
        Utc::now() + self.settings().time_offset
    }

    pub fn settings(&self) -> RuntimeSettings {
        self.settings
            .read()
            .map(|s| s.clone())
            .unwrap_or_else(|p| p.into_inner().clone())
    }

    pub fn update_settings(&self, f: impl FnOnce(&mut RuntimeSettings)) -> RuntimeSettings {
        let mut g = self.settings.write().unwrap_or_else(|p| p.into_inner());
        f(&mut g);
        g.clone()
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
