//! REST API(前缀 `/api/v1`)。

use crate::errors::Coded;
use crate::query::{ListQuery, merge_filter};
use crate::state::{AppState, Scope};
use crate::ws::{ClientGuard, handle_socket};
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Duration, Utc};
use orbit_core::time::parse_time_arg;
use orbit_core::{ObjectSummary, SpaceObject, StateVector, Stats, TrackPoint};
use serde::Serialize;
use serde_json::json;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::Ordering;

pub const DEFAULT_PER_PAGE: usize = 100;
pub const MAX_PER_PAGE: usize = 5_000;

/// API 错误:JSON `{"error": "...", "code": "...", "params": {...}}`。
///
/// `error` 为中文说明;`code` 与 `params` 供界面显示所选语言的说明(见 [`crate::errors`])。
#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    message: String,
    code: Option<&'static str>,
    params: serde_json::Map<String, serde_json::Value>,
}

impl ApiError {
    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
            code: None,
            params: serde_json::Map::new(),
        }
    }
    fn coded(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code: Some(code),
            ..Self::new(status, message)
        }
    }
    fn bad(code: &'static str, message: impl Into<String>) -> Self {
        Self::coded(StatusCode::BAD_REQUEST, code, message)
    }
    fn param(mut self, name: &str, value: impl Into<serde_json::Value>) -> Self {
        self.params.insert(name.to_string(), value.into());
        self
    }
    fn internal(e: impl std::fmt::Display) -> Self {
        Self::coded(StatusCode::INTERNAL_SERVER_ERROR, "internal", e.to_string())
            .param("detail", e.to_string())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut body = serde_json::Map::new();
        body.insert("error".into(), self.message.into());
        if let Some(code) = self.code {
            body.insert("code".into(), code.into());
            if !self.params.is_empty() {
                body.insert("params".into(), self.params.into());
            }
        }
        (self.status, Json(serde_json::Value::Object(body))).into_response()
    }
}

impl From<Coded> for ApiError {
    fn from(e: Coded) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: e.message,
            code: Some(e.code),
            params: e.params,
        }
    }
}

impl From<orbit_core::CoreError> for ApiError {
    fn from(e: orbit_core::CoreError) -> Self {
        Coded::from(e).into()
    }
}

type ApiResult<T> = Result<Json<T>, ApiError>;

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/v1/status", get(status))
        .route(
            "/api/v1/filters/default",
            get(get_default_filter).put(put_default_filter),
        )
        .route("/api/v1/objects", get(list_objects))
        .route("/api/v1/objects/{id}", get(get_object))
        .route("/api/v1/objects/{id}/track", get(get_track))
        .route("/api/v1/stats", get(stats))
        .route("/api/v1/refresh", post(refresh))
        .route("/api/v1/stream", get(stream))
        .route("/api/v1/client-config", get(client_config))
        .route("/api/v1/settings", get(get_settings).put(put_settings))
        .merge(crate::web::routes(&state.options))
        .with_state(state)
}

/// 请求所用的编目与时钟:带 `view=<编号>` 时用该连接的"指定时刻"视图,否则用服务端实时的。
fn scope_of(state: &AppState, view: Option<&str>) -> Result<Scope, ApiError> {
    state.scope(view).ok_or_else(|| {
        ApiError::coded(
            StatusCode::NOT_FOUND,
            "view_gone",
            "指定时刻的视图已不存在(对应的连接已断开或已回到实时)",
        )
    })
}

fn parse_at(scope: &Scope, at: Option<&str>) -> Result<DateTime<Utc>, ApiError> {
    match at {
        None => Ok(scope.now),
        Some(s) => Ok(parse_time_arg(s, scope.now)?),
    }
}

/// 反向代理转发请求时附带的请求头。带有其中任何一个的请求不是直接来自本机的浏览器或命令行。
const FORWARDED_HEADERS: [&str; 6] = [
    "forwarded",
    "x-forwarded-for",
    "x-forwarded-host",
    "x-forwarded-proto",
    "x-real-ip",
    "via",
];

/// 请求是否经过了反向代理(此时连接来源是代理的地址,不能代表真正的访问者)。
fn is_forwarded(headers: &HeaderMap) -> bool {
    FORWARDED_HEADERS.iter().any(|h| headers.contains_key(*h))
}

/// 写操作鉴权:配置了 token 时要求 `Authorization: Bearer <token>`;
/// 否则只允许本机直接发起的请求(来源为本机地址,且没有经过反向代理)。
fn check_write(state: &AppState, headers: &HeaderMap, peer: SocketAddr) -> Result<(), ApiError> {
    match &state.options.api_token {
        Some(token) => {
            let given = headers
                .get(header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("Bearer "))
                .unwrap_or("");
            // 长度相同时逐字节比较全部内容,避免提前返回泄露信息
            let ok = given.len() == token.len()
                && given
                    .bytes()
                    .zip(token.bytes())
                    .fold(0u8, |acc, (a, b)| acc | (a ^ b))
                    == 0;
            if ok {
                Ok(())
            } else {
                Err(ApiError::coded(
                    StatusCode::UNAUTHORIZED,
                    "token_required",
                    "需要有效的 API token",
                ))
            }
        }
        // 经反向代理转发的请求,连接来源是代理(常为本机地址),不能当作本机请求放行
        None if is_forwarded(headers) => Err(ApiError::coded(
            StatusCode::FORBIDDEN,
            "local_only",
            "未配置 API token 时,写操作不接受经反向代理转发的请求;请设置 ORBITSCOPE_API_TOKEN",
        )),
        None if peer.ip().is_loopback() => Ok(()),
        None => Err(ApiError::coded(
            StatusCode::FORBIDDEN,
            "local_only",
            "未配置 API token 时,写操作只允许本机访问;请设置 ORBITSCOPE_API_TOKEN",
        )),
    }
}

// ------------------------------------------------------------------ handlers

#[derive(Serialize)]
struct CesiumIntegrity {
    js: &'static str,
    css: &'static str,
}

#[derive(Serialize)]
struct ClientConfig {
    version: &'static str,
    cesium_base_url: String,
    /// 从默认 CDN 地址加载时的完整性校验值;本地目录或自定义地址时为 null
    cesium_integrity: Option<CesiumIntegrity>,
    tick_ms: u64,
    max_rate_hz: f64,
}

async fn client_config(State(state): State<Arc<AppState>>) -> Json<ClientConfig> {
    let cesium_base_url = if state.options.cesium_dir.is_some() {
        "/cesium/".to_string()
    } else {
        state.options.cesium_base_url.clone()
    };
    let cesium_integrity = (cesium_base_url == crate::state::DEFAULT_CESIUM_URL).then_some(CesiumIntegrity {
        js: crate::state::DEFAULT_CESIUM_JS_INTEGRITY,
        css: crate::state::DEFAULT_CESIUM_CSS_INTEGRITY,
    });
    Json(ClientConfig {
        version: env!("CARGO_PKG_VERSION"),
        cesium_base_url,
        cesium_integrity,
        tick_ms: state.options.tick.as_millis() as u64,
        max_rate_hz: crate::ws::MAX_RATE_HZ,
    })
}

#[derive(Serialize)]
struct StatusResponse {
    version: &'static str,
    started_at: DateTime<Utc>,
    server_time: DateTime<Utc>,
    time_offset_secs: i64,
    generation: u64,
    objects: usize,
    catalog_loaded_at: DateTime<Utc>,
    report: orbit_core::BuildReport,
    sources: Vec<orbit_sources::SourceStatus>,
    schedule: crate::state::ScheduleInfo,
    ws_clients: usize,
    tick_ms: u64,
    settings: SettingsResponse,
    /// 当前编目的数据依据(来自历史库时)
    provenance: Option<orbit_archive::Provenance>,
    /// 历史库中最近的采集记录
    recent_fetches: Vec<orbit_archive::FetchRow>,
    /// "live"(实时)或 "fixed"(请求带了 `view`:该连接的指定时刻)
    time_mode: &'static str,
    /// 请求所用视图的编号;实时为 null
    view: Option<String>,
}

/// 只带 `view` 的查询参数。
#[derive(serde::Deserialize, Default)]
#[serde(default)]
struct ViewQuery {
    view: Option<String>,
}

async fn status(State(state): State<Arc<AppState>>, Query(q): Query<ViewQuery>) -> ApiResult<StatusResponse> {
    let scope = scope_of(&state, q.view.as_deref())?;
    let cs = scope.catalog.clone();
    let recent_fetches = match state.options.archive.clone() {
        Some(a) => tokio::task::spawn_blocking(move || a.history().map(|h| h.recent_fetches(10)))
            .await
            .ok()
            .flatten()
            .and_then(Result::ok)
            .unwrap_or_default(),
        None => Vec::new(),
    };
    Ok(Json(StatusResponse {
        version: env!("CARGO_PKG_VERSION"),
        started_at: state.started_at,
        server_time: scope.now,
        time_offset_secs: scope
            .view
            .as_ref()
            .map_or(state.settings().time_offset, |v| v.offset)
            .num_seconds(),
        generation: cs.generation,
        objects: cs.catalog.len(),
        catalog_loaded_at: cs.loaded_at,
        report: cs.report.clone(),
        sources: state.source_status(),
        schedule: state.schedule_info(),
        ws_clients: state.ws_clients.load(Ordering::SeqCst),
        tick_ms: state.options.tick.as_millis() as u64,
        settings: settings_response(&state),
        provenance: cs.provenance.clone(),
        recent_fetches,
        time_mode: if scope.view.is_some() { "fixed" } else { "live" },
        view: scope.view.as_ref().map(|v| v.id.clone()),
    }))
}

// ------------------------------------------------------------------ settings

#[derive(Serialize)]
struct Choice {
    id: &'static str,
    label: &'static str,
}

#[derive(Serialize)]
struct SettingsResponse {
    time_policy: orbit_archive::TimePolicy,
    collect_mode: orbit_archive::CollectMode,
    history_enabled: bool,
    /// 设置是否会保存到磁盘(否则只在本次运行有效)
    persisted: bool,
    /// 写操作是否需要 API token(界面据此显示 token 输入框)
    token_required: bool,
    policies: Vec<Choice>,
    collect_modes: Vec<Choice>,
}

fn settings_response(state: &AppState) -> SettingsResponse {
    let s = state.settings();
    SettingsResponse {
        time_policy: s.time_policy,
        collect_mode: s.collect_mode,
        history_enabled: state
            .options
            .archive
            .as_ref()
            .is_some_and(|a| a.history().is_some()),
        persisted: state.options.settings_path.is_some(),
        token_required: state.options.api_token.is_some(),
        policies: orbit_archive::TimePolicy::ALL
            .iter()
            .map(|p| Choice {
                id: p.as_str(),
                label: p.label(),
            })
            .collect(),
        collect_modes: orbit_archive::CollectMode::ALL
            .iter()
            .map(|m| Choice {
                id: m.as_str(),
                label: m.label(),
            })
            .collect(),
    }
}

async fn get_settings(State(state): State<Arc<AppState>>) -> Json<SettingsResponse> {
    Json(settings_response(&state))
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SettingsPatch {
    time_policy: Option<String>,
    collect_mode: Option<String>,
}

async fn put_settings(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(patch): Json<SettingsPatch>,
) -> ApiResult<SettingsResponse> {
    check_write(&state, &headers, peer)?;
    let policy = patch
        .time_policy
        .as_deref()
        .map(str::parse::<orbit_archive::TimePolicy>)
        .transpose()
        .map_err(|m| {
            ApiError::bad("unknown_policy", m).param("value", patch.time_policy.clone().unwrap_or_default())
        })?;
    let mode = patch
        .collect_mode
        .as_deref()
        .map(str::parse::<orbit_archive::CollectMode>)
        .transpose()
        .map_err(|m| {
            ApiError::bad("unknown_collect_mode", m)
                .param("value", patch.collect_mode.clone().unwrap_or_default())
        })?;
    let new = state.update_settings(|s| {
        if let Some(p) = policy {
            s.time_policy = p;
        }
        if let Some(m) = mode {
            s.collect_mode = m;
        }
    });
    if let Some(path) = &state.options.settings_path {
        let persisted = crate::state::PersistedSettings {
            time_policy: new.time_policy,
            collect_mode: new.collect_mode,
        };
        if let Err(e) = persisted.save(path) {
            tracing::warn!(error = %e, "保存界面设置失败");
        }
    }
    tracing::info!(policy = %new.time_policy, mode = %new.collect_mode, "设置已更新");
    // 立即按新设置重建编目(切换到 serve 采集时同时联网采集一次)
    let kind = if mode == Some(orbit_archive::CollectMode::Serve) {
        crate::state::RefreshKind::Collect
    } else {
        crate::state::RefreshKind::Reload
    };
    let _ = state.refresh_tx.try_send(kind);
    Ok(Json(settings_response(&state)))
}

async fn get_default_filter(State(state): State<Arc<AppState>>) -> Json<orbit_core::Filter> {
    Json(state.default_filter())
}

async fn put_default_filter(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(patch): Json<serde_json::Value>,
) -> ApiResult<orbit_core::Filter> {
    check_write(&state, &headers, peer)?;
    let f = merge_filter(&state.default_filter(), &patch)?;
    state.set_default_filter(f.clone());
    tracing::info!("默认过滤条件已更新");
    Ok(Json(f))
}

#[derive(Serialize)]
struct ListResponse {
    time: DateTime<Utc>,
    generation: u64,
    total: usize,
    page: usize,
    per_page: usize,
    failed: usize,
    items: Vec<ObjectSummary>,
}

type RowCmp = Box<dyn Fn(&ObjectSummary, &ObjectSummary) -> std::cmp::Ordering>;

fn sort_rows(rows: &mut [ObjectSummary], key: &str, desc: bool) -> Result<(), ApiError> {
    use std::cmp::Ordering as O;
    let f = |a: Option<f64>, b: Option<f64>| {
        a.unwrap_or(f64::NAN)
            .partial_cmp(&b.unwrap_or(f64::NAN))
            .unwrap_or(O::Equal)
    };
    let size = |r: &ObjectSummary| r.size_max_m.or(r.size_min_m).or(Some(-1.0));
    let cmp: RowCmp = match key {
        "id" => Box::new(|a, b| a.norad_id.cmp(&b.norad_id)),
        "name" => Box::new(|a, b| a.name.cmp(&b.name)),
        "alt" => Box::new(move |a, b| f(a.alt_km, b.alt_km)),
        "perigee" => Box::new(move |a, b| f(Some(a.perigee_km), Some(b.perigee_km))),
        "apogee" => Box::new(move |a, b| f(Some(a.apogee_km), Some(b.apogee_km))),
        "inc" => Box::new(move |a, b| f(Some(a.inclination_deg), Some(b.inclination_deg))),
        "age" => Box::new(move |a, b| f(Some(a.epoch_age_days), Some(b.epoch_age_days))),
        "size" => Box::new(move |a, b| f(size(a), size(b))),
        other => {
            return Err(ApiError::bad(
                "unknown_sort",
                format!("未知的排序字段 {other:?}(可选 id, name, alt, perigee, apogee, inc, age, size)"),
            )
            .param("value", other));
        }
    };
    rows.sort_by(|a, b| if desc { cmp(b, a) } else { cmp(a, b) });
    Ok(())
}

async fn list_objects(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ListQuery>,
) -> ApiResult<ListResponse> {
    let filter = q.apply(&state.default_filter())?;
    let scope = scope_of(&state, q.view.as_deref())?;
    let at = parse_at(&scope, q.at.as_deref())?;
    let per_page = q.per_page.unwrap_or(DEFAULT_PER_PAGE).clamp(1, MAX_PER_PAGE);
    let page = q.page.unwrap_or(1).max(1);
    let sort = q.sort.clone().unwrap_or_else(|| "id".into());
    let desc = q.desc.unwrap_or(false);

    let cs = scope.catalog;
    let resp = tokio::task::spawn_blocking(move || -> Result<ListResponse, ApiError> {
        let cat = &cs.catalog;
        let idx = cat.select(&filter, at);
        let snap = cat.snapshot(&idx, &filter, at);
        let mut rows: Vec<ObjectSummary> = snap
            .states
            .iter()
            .map(|(i, s)| ObjectSummary::new(&cat.objects()[*i], Some(s), at))
            .collect();
        if !filter.needs_instantaneous() {
            rows.extend(
                snap.failed
                    .iter()
                    .map(|(i, _)| ObjectSummary::new(&cat.objects()[*i], None, at)),
            );
        }
        sort_rows(&mut rows, &sort, desc)?;
        let total = rows.len();
        let items: Vec<ObjectSummary> = rows
            .into_iter()
            .skip((page - 1) * per_page)
            .take(per_page)
            .collect();
        Ok(ListResponse {
            time: at,
            generation: cs.generation,
            total,
            page,
            per_page,
            failed: snap.failed.len(),
            items,
        })
    })
    .await
    .map_err(ApiError::internal)??;
    Ok(Json(resp))
}

#[derive(Serialize)]
struct ObjectResponse {
    time: DateTime<Utc>,
    object: SpaceObject,
    summary: ObjectSummary,
    state: Option<StateVector>,
    error: Option<String>,
}

#[derive(serde::Deserialize, Default)]
#[serde(default)]
struct AtQuery {
    at: Option<String>,
    /// 使用某个连接的"指定时刻"视图
    view: Option<String>,
}

fn find(scope: Scope, id: u32) -> Result<(Arc<crate::state::CatalogState>, usize), ApiError> {
    let cs = scope.catalog;
    let idx = cs.catalog.index_of(id).ok_or_else(|| {
        ApiError::coded(
            StatusCode::NOT_FOUND,
            "object_not_found",
            format!("编目中没有 NORAD {id}"),
        )
        .param("id", id)
    })?;
    Ok((cs, idx))
}

async fn get_object(
    State(state): State<Arc<AppState>>,
    Path(id): Path<u32>,
    Query(q): Query<AtQuery>,
) -> ApiResult<ObjectResponse> {
    let scope = scope_of(&state, q.view.as_deref())?;
    let at = parse_at(&scope, q.at.as_deref())?;
    let (cs, idx) = find(scope, id)?;
    let o = &cs.catalog.objects()[idx];
    let (st, error) = match cs.catalog.propagate(idx, at) {
        Ok(s) => (Some(s), None),
        Err(e) => (None, Some(e.to_string())),
    };
    Ok(Json(ObjectResponse {
        time: at,
        summary: ObjectSummary::new(o, st.as_ref(), at),
        object: o.clone(),
        state: st,
        error,
    }))
}

#[derive(serde::Deserialize, Default)]
#[serde(default)]
struct TrackQuery {
    at: Option<String>,
    /// 向前(过去)分钟数,默认 0
    before_min: Option<f64>,
    /// 向后(未来)分钟数,默认一个轨道周期
    after_min: Option<f64>,
    /// 步长(秒),默认 30
    step_s: Option<f64>,
    /// 使用某个连接的"指定时刻"视图
    view: Option<String>,
}

#[derive(Serialize)]
struct TrackResponse {
    norad_id: u32,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    step_s: f64,
    points: Vec<TrackPoint>,
}

async fn get_track(
    State(state): State<Arc<AppState>>,
    Path(id): Path<u32>,
    Query(q): Query<TrackQuery>,
) -> ApiResult<TrackResponse> {
    let scope = scope_of(&state, q.view.as_deref())?;
    let at = parse_at(&scope, q.at.as_deref())?;
    let (cs, idx) = find(scope, id)?;
    let period = cs.catalog.objects()[idx].derived.period_min;
    let before = q.before_min.unwrap_or(0.0);
    let after = q
        .after_min
        .unwrap_or(if period.is_finite() { period } else { 90.0 });
    let step_s = q.step_s.unwrap_or(30.0);
    for (name, v) in [("before_min", before), ("after_min", after)] {
        if !v.is_finite() || !(0.0..=10_080.0).contains(&v) {
            return Err(
                ApiError::bad("track_range", format!("{name} 应在 0–10080 分钟之间")).param("name", name),
            );
        }
    }
    if !step_s.is_finite() || step_s < 1.0 {
        return Err(ApiError::bad("track_step", "step_s 不能小于 1"));
    }
    let start = at - Duration::milliseconds((before * 60_000.0) as i64);
    let end = at + Duration::milliseconds((after * 60_000.0) as i64);
    let step = Duration::milliseconds((step_s * 1000.0) as i64);
    // 逐点外推可达数万次,放到阻塞线程池,不占用异步工作线程
    let points = tokio::task::spawn_blocking(move || cs.catalog.track(idx, start, end, step))
        .await
        .map_err(ApiError::internal)??;
    Ok(Json(TrackResponse {
        norad_id: id,
        start,
        end,
        step_s,
        points,
    }))
}

async fn stats(State(state): State<Arc<AppState>>, Query(q): Query<ListQuery>) -> ApiResult<Stats> {
    let filter = q.apply(&state.default_filter())?;
    let scope = scope_of(&state, q.view.as_deref())?;
    let at = parse_at(&scope, q.at.as_deref())?;
    let bin = q.bin_km.unwrap_or(50.0);
    let cs = scope.catalog;
    let s = tokio::task::spawn_blocking(move || {
        let cat = &cs.catalog;
        let idx = cat.select(&filter, at);
        if filter.needs_instantaneous() {
            let snap = cat.snapshot(&idx, &filter, at);
            Stats::compute(snap.states.iter().map(|(i, _)| &cat.objects()[*i]), bin)
        } else {
            Stats::compute(idx.iter().map(|&i| &cat.objects()[i]), bin)
        }
    })
    .await
    .map_err(ApiError::internal)?;
    Ok(Json(s))
}

async fn refresh(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    check_write(&state, &headers, peer)?;
    let queued = state
        .refresh_tx
        .try_send(crate::state::RefreshKind::Collect)
        .is_ok();
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({
            "queued": queued,
            "note": "刷新会遵守各数据源的最短刷新间隔,间隔内将直接使用缓存"
        })),
    ))
}

async fn stream(State(state): State<Arc<AppState>>, ws: WebSocketUpgrade) -> Response {
    let max = state.options.max_ws_clients;
    let prev = state.ws_clients.fetch_add(1, Ordering::SeqCst);
    let guard = ClientGuard(state.clone());
    if prev >= max {
        drop(guard);
        return ApiError::coded(
            StatusCode::SERVICE_UNAVAILABLE,
            "ws_limit",
            format!("WebSocket 连接数已达上限 {max}"),
        )
        .param("max", max)
        .into_response();
    }
    ws.max_message_size(64 * 1024)
        .on_upgrade(move |socket| handle_socket(socket, state, guard))
}
