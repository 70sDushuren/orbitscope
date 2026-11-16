//! REST API(前缀 `/api/v1`)。

use crate::query::{ListQuery, merge_filter};
use crate::state::AppState;
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

/// API 错误:JSON `{"error": "..."}`。
#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }
    fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({ "error": self.message }))).into_response()
    }
}

impl From<orbit_core::CoreError> for ApiError {
    fn from(e: orbit_core::CoreError) -> Self {
        Self::bad_request(e.to_string())
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
        .merge(crate::web::routes(&state.options))
        .with_state(state)
}

fn parse_at(state: &AppState, at: Option<&str>) -> Result<DateTime<Utc>, ApiError> {
    let now = state.now();
    match at {
        None => Ok(now),
        Some(s) => Ok(parse_time_arg(s, now)?),
    }
}

/// 写操作鉴权:配置了 token 时要求 `Authorization: Bearer <token>`;否则只允许本机地址。
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
                Err(ApiError::new(StatusCode::UNAUTHORIZED, "需要有效的 API token"))
            }
        }
        None if peer.ip().is_loopback() => Ok(()),
        None => Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "未配置 API token 时,写操作只允许本机访问",
        )),
    }
}

// ------------------------------------------------------------------ handlers

#[derive(Serialize)]
struct ClientConfig {
    version: &'static str,
    cesium_base_url: String,
    tick_ms: u64,
    max_rate_hz: f64,
}

async fn client_config(State(state): State<Arc<AppState>>) -> Json<ClientConfig> {
    let cesium_base_url = if state.options.cesium_dir.is_some() {
        "/cesium/".to_string()
    } else {
        state.options.cesium_base_url.clone()
    };
    Json(ClientConfig {
        version: env!("CARGO_PKG_VERSION"),
        cesium_base_url,
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
}

async fn status(State(state): State<Arc<AppState>>) -> Json<StatusResponse> {
    let cs = state.catalog.load_full();
    Json(StatusResponse {
        version: env!("CARGO_PKG_VERSION"),
        started_at: state.started_at,
        server_time: state.now(),
        time_offset_secs: state.options.time_offset.num_seconds(),
        generation: cs.generation,
        objects: cs.catalog.len(),
        catalog_loaded_at: cs.loaded_at,
        report: cs.report.clone(),
        sources: state.source_status(),
        schedule: state.schedule_info(),
        ws_clients: state.ws_clients.load(Ordering::SeqCst),
        tick_ms: state.options.tick.as_millis() as u64,
    })
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
    let f = merge_filter(&state.default_filter(), &patch).map_err(ApiError::bad_request)?;
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
            return Err(ApiError::bad_request(format!(
                "未知的排序字段 {other:?}(可选 id, name, alt, perigee, apogee, inc, age, size)"
            )));
        }
    };
    rows.sort_by(|a, b| if desc { cmp(b, a) } else { cmp(a, b) });
    Ok(())
}

async fn list_objects(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ListQuery>,
) -> ApiResult<ListResponse> {
    let filter = q.apply(&state.default_filter()).map_err(ApiError::bad_request)?;
    let at = parse_at(&state, q.at.as_deref())?;
    let per_page = q.per_page.unwrap_or(DEFAULT_PER_PAGE).clamp(1, MAX_PER_PAGE);
    let page = q.page.unwrap_or(1).max(1);
    let sort = q.sort.clone().unwrap_or_else(|| "id".into());
    let desc = q.desc.unwrap_or(false);

    let cs = state.catalog.load_full();
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
    .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))??;
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
}

fn find(state: &AppState, id: u32) -> Result<(Arc<crate::state::CatalogState>, usize), ApiError> {
    let cs = state.catalog.load_full();
    let idx = cs
        .catalog
        .index_of(id)
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, format!("编目中没有 NORAD {id}")))?;
    Ok((cs, idx))
}

async fn get_object(
    State(state): State<Arc<AppState>>,
    Path(id): Path<u32>,
    Query(q): Query<AtQuery>,
) -> ApiResult<ObjectResponse> {
    let at = parse_at(&state, q.at.as_deref())?;
    let (cs, idx) = find(&state, id)?;
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
    let at = parse_at(&state, q.at.as_deref())?;
    let (cs, idx) = find(&state, id)?;
    let period = cs.catalog.objects()[idx].derived.period_min;
    let before = q.before_min.unwrap_or(0.0);
    let after = q
        .after_min
        .unwrap_or(if period.is_finite() { period } else { 90.0 });
    let step_s = q.step_s.unwrap_or(30.0);
    for (name, v) in [("before_min", before), ("after_min", after)] {
        if !v.is_finite() || !(0.0..=10_080.0).contains(&v) {
            return Err(ApiError::bad_request(format!("{name} 应在 0–10080 分钟之间")));
        }
    }
    if !step_s.is_finite() || step_s < 1.0 {
        return Err(ApiError::bad_request("step_s 不能小于 1"));
    }
    let start = at - Duration::milliseconds((before * 60_000.0) as i64);
    let end = at + Duration::milliseconds((after * 60_000.0) as i64);
    let step = Duration::milliseconds((step_s * 1000.0) as i64);
    let points = cs.catalog.track(idx, start, end, step)?;
    Ok(Json(TrackResponse {
        norad_id: id,
        start,
        end,
        step_s,
        points,
    }))
}

async fn stats(State(state): State<Arc<AppState>>, Query(q): Query<ListQuery>) -> ApiResult<Stats> {
    let filter = q.apply(&state.default_filter()).map_err(ApiError::bad_request)?;
    let at = parse_at(&state, q.at.as_deref())?;
    let bin = q.bin_km.unwrap_or(50.0);
    let cs = state.catalog.load_full();
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
    .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(s))
}

async fn refresh(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    check_write(&state, &headers, peer)?;
    let queued = state.refresh_tx.try_send(()).is_ok();
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
        return ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            format!("WebSocket 连接数已达上限 {max}"),
        )
        .into_response();
    }
    ws.max_message_size(64 * 1024)
        .on_upgrade(move |socket| handle_socket(socket, state, guard))
}
