//! WebSocket 实时位置推送(`/api/v1/stream`)。
//!
//! 客户端 → 服务端(JSON 文本):
//! - `{"type":"subscribe","filter":{...局部过滤条件...},"rate_hz":1,"time":...}`:订阅(可重复发送以更换条件)。
//!   `time` 只对本连接生效:RFC3339 时刻 = 从该时刻起回放(时钟继续走),`"live"` = 回到实时,
//!   省略 = 保持本连接当前的时间模式
//! - `{"type":"ping"}`
//!
//! 服务端 → 客户端:
//! - JSON `{"type":"hello",...}`:连接建立后立即发送
//! - JSON `{"type":"catalog",...}`:订阅时与编目更新时发送物体元数据(`i` 为帧中使用的编目下标);
//!   `view` 为本连接"指定时刻"视图的编号(实时为 null),REST 请求带上 `view=<编号>` 即使用同一时刻与数据
//! - 二进制位置帧(格式见 [`crate::frame`])
//! - JSON `{"type":"error","message":...,"code":...,"params":...}` / `{"type":"pong"}`

use crate::errors::Coded;
use crate::frame;
use crate::query::merge_filter;
use crate::state::{AppState, CatalogState, View};
use axum::extract::ws::{Message, WebSocket};
use chrono::{DateTime, Utc};
use orbit_core::Filter;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

/// 允许的推送频率范围(Hz)。实际频率不超过服务端 tick。
pub const MIN_RATE_HZ: f64 = 0.05;
pub const MAX_RATE_HZ: f64 = 10.0;

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientMsg {
    Subscribe {
        #[serde(default)]
        filter: serde_json::Value,
        #[serde(default)]
        rate_hz: Option<f64>,
        /// 本连接的时间:RFC3339 时刻、`"live"`,或省略(保持不变)
        #[serde(default)]
        time: Option<String>,
    },
    Ping,
}

/// WS 中的物体元数据(字段名简短以减小体积)。
#[derive(Debug, Serialize)]
pub struct WsObject {
    /// 编目下标(位置帧中使用)
    pub i: u32,
    pub id: u32,
    pub name: String,
    #[serde(rename = "type")]
    pub object_type: &'static str,
    pub country: Option<String>,
    pub perigee_km: f32,
    pub apogee_km: f32,
    pub inc_deg: f32,
    /// 代表尺寸(m),未知为 null
    pub size_m: Option<f32>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ServerMsg<'a> {
    Hello {
        version: &'static str,
        tick_ms: u64,
        generation: u64,
        frame_format: &'static str,
    },
    Catalog {
        generation: u64,
        total: usize,
        filter: &'a Filter,
        rate_hz: f64,
        /// 本连接"指定时刻"视图的编号;实时为 null
        view: Option<&'a str>,
        /// "live"(实时)或 "fixed"(指定时刻,时钟从该时刻继续走)
        time_mode: &'static str,
        /// 本连接的当前时间
        time: DateTime<Utc>,
        objects: Vec<WsObject>,
    },
    Error {
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        code: Option<&'static str>,
        #[serde(skip_serializing_if = "serde_json::Map::is_empty")]
        params: serde_json::Map<String, serde_json::Value>,
    },
    Pong,
}

struct Subscription {
    filter: Filter,
    indices: Vec<usize>,
    generation: u64,
    min_interval: Duration,
    last_sent: Option<Instant>,
}

/// 连接计数守卫。
pub struct ClientGuard(pub Arc<AppState>);

impl Drop for ClientGuard {
    fn drop(&mut self) {
        self.0.ws_clients.fetch_sub(1, Ordering::SeqCst);
    }
}

/// 本连接持有的"指定时刻"视图;丢弃时(回到实时、换时刻、连接断开)从登记表移除。
struct ViewHandle {
    state: Arc<AppState>,
    view: Arc<View>,
}

impl Drop for ViewHandle {
    fn drop(&mut self) {
        self.state.remove_view(&self.view.id);
    }
}

fn error_msg(e: Coded) -> ServerMsg<'static> {
    ServerMsg::Error {
        message: e.message,
        code: Some(e.code),
        params: e.params,
    }
}

async fn send_json(socket: &mut WebSocket, msg: &ServerMsg<'_>) -> bool {
    match serde_json::to_string(msg) {
        Ok(text) => socket.send(Message::Text(text.into())).await.is_ok(),
        Err(_) => false,
    }
}

fn catalog_objects(cs: &CatalogState, indices: &[usize]) -> Vec<WsObject> {
    indices
        .iter()
        .filter_map(|&i| cs.catalog.get(i).map(|o| (i, o)))
        .map(|(i, o)| WsObject {
            i: i as u32,
            id: o.norad_id,
            name: o.name.clone(),
            object_type: o.meta.object_type.as_str(),
            country: o.meta.country.clone(),
            perigee_km: o.derived.perigee_alt_km as f32,
            apogee_km: o.derived.apogee_alt_km as f32,
            inc_deg: o.derived.inclination_deg as f32,
            size_m: o.size.representative_m().map(|v| v as f32),
        })
        .collect()
}

/// 为订阅计算下标集合并发送元数据(用本连接视图的编目与时钟,没有视图时用实时的)。
async fn send_catalog(
    socket: &mut WebSocket,
    state: &AppState,
    view: Option<&ViewHandle>,
    sub: &mut Subscription,
) -> bool {
    let (cs, now) = match view {
        Some(v) => (v.view.catalog.load_full(), v.view.now()),
        None => (state.catalog.load_full(), state.now()),
    };
    sub.indices = cs.catalog.select(&sub.filter, now);
    sub.generation = cs.generation;
    let rate_hz = 1.0 / sub.min_interval.as_secs_f64();
    let msg = ServerMsg::Catalog {
        generation: cs.generation,
        total: sub.indices.len(),
        filter: &sub.filter,
        rate_hz,
        view: view.map(|v| v.view.id.as_str()),
        time_mode: if view.is_some() { "fixed" } else { "live" },
        time: now,
        objects: catalog_objects(&cs, &sub.indices),
    };
    send_json(socket, &msg).await
}

/// 按视图的当前时刻重建其编目;内容变化时向客户端重发元数据。
async fn refresh_view(
    socket: &mut WebSocket,
    state: &AppState,
    view: &ViewHandle,
    sub: &mut Option<Subscription>,
) -> bool {
    let prev = view.view.catalog.load_full();
    let next = crate::view::build_catalog(state, view.view.now(), Some(&prev)).await;
    let changed = next.generation != prev.generation;
    view.view.catalog.store(next);
    match sub.as_mut() {
        Some(s) if changed => send_catalog(socket, state, Some(view), s).await,
        _ => true,
    }
}

pub async fn handle_socket(mut socket: WebSocket, state: Arc<AppState>, _guard: ClientGuard) {
    // 实时位置帧:只有处于实时模式时才订阅(没有实时订阅者时服务端不计算实时帧)
    let mut pos_rx = Some(state.positions.subscribe());
    let mut gen_rx = state.catalog_gen.subscribe();
    let mut shutdown = state.shutdown.subscribe();
    let mut sub: Option<Subscription> = None;
    // 本连接的"指定时刻"视图;None = 实时
    let mut view: Option<ViewHandle> = None;
    let mut view_tick = tokio::time::interval(state.options.tick);
    view_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let reload = Duration::from_secs(60 * state.options.collect.reload_minutes.clamp(1, 1_440));
    let mut view_reload = tokio::time::interval_at(tokio::time::Instant::now() + reload, reload);
    view_reload.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let hello = ServerMsg::Hello {
        version: env!("CARGO_PKG_VERSION"),
        tick_ms: state.options.tick.as_millis() as u64,
        generation: state.catalog.load().generation,
        frame_format: "OSF1",
    };
    if !send_json(&mut socket, &hello).await {
        return;
    }

    loop {
        tokio::select! {
            msg = socket.recv() => {
                let Some(Ok(msg)) = msg else { break };
                let text = match msg {
                    Message::Text(t) => t,
                    Message::Close(_) => break,
                    _ => continue,
                };
                match serde_json::from_str::<ClientMsg>(&text) {
                    Ok(ClientMsg::Ping) => {
                        if !send_json(&mut socket, &ServerMsg::Pong).await { break; }
                    }
                    Ok(ClientMsg::Subscribe { filter, rate_hz, time }) => {
                        let base = state.default_filter();
                        let filter = match merge_filter(&base, &filter) {
                            Ok(f) => f,
                            Err(e) => {
                                if !send_json(&mut socket, &error_msg(e)).await { break; }
                                continue;
                            }
                        };
                        let rate = rate_hz.unwrap_or(1.0);
                        if !rate.is_finite() {
                            let e = Coded::new("bad_rate", "rate_hz 无效");
                            if !send_json(&mut socket, &error_msg(e)).await { break; }
                            continue;
                        }
                        let rate = rate.clamp(MIN_RATE_HZ, MAX_RATE_HZ);
                        // 本连接的时间:省略时保持不变
                        match time.as_deref().map(str::trim) {
                            None => {}
                            Some(t) if t.eq_ignore_ascii_case("live") => view = None,
                            Some(t) => match DateTime::parse_from_rfc3339(t) {
                                Ok(at) => {
                                    let at = at.with_timezone(&Utc);
                                    let catalog = crate::view::build_catalog(&state, at, None).await;
                                    // 先登记新视图再丢弃旧的
                                    view = Some(ViewHandle {
                                        state: state.clone(),
                                        view: state.register_view(at, catalog),
                                    });
                                }
                                Err(e) => {
                                    let e = Coded::new("bad_time", format!("无法解析时刻 {t:?}: {e}"))
                                        .param("value", t);
                                    if !send_json(&mut socket, &error_msg(e)).await { break; }
                                    continue;
                                }
                            },
                        }
                        pos_rx = match (&view, pos_rx.take()) {
                            (Some(_), _) => None,
                            (None, rx) => Some(rx.unwrap_or_else(|| state.positions.subscribe())),
                        };
                        let mut s = Subscription {
                            filter,
                            indices: Vec::new(),
                            generation: 0,
                            min_interval: Duration::from_secs_f64(1.0 / rate),
                            last_sent: None,
                        };
                        if !send_catalog(&mut socket, &state, view.as_ref(), &mut s).await { break; }
                        sub = Some(s);
                    }
                    Err(e) => {
                        let e = Coded::new("bad_message", format!("无法解析消息: {e}"))
                            .param("detail", e.to_string());
                        if !send_json(&mut socket, &error_msg(e)).await { break; }
                    }
                }
            }
            changed = gen_rx.changed() => {
                if changed.is_err() { break; }
                match view.as_ref() {
                    // 实时编目变化(新数据入库或时刻策略改变):视图也按自己的时刻重新选取
                    Some(v) => {
                        if !refresh_view(&mut socket, &state, v, &mut sub).await { break; }
                    }
                    None => {
                        if let Some(s) = sub.as_mut()
                            && !send_catalog(&mut socket, &state, None, s).await
                        {
                            break;
                        }
                    }
                }
            }
            changed = async {
                match pos_rx.as_mut() {
                    Some(rx) => rx.changed().await,
                    None => std::future::pending().await,
                }
            } => {
                if changed.is_err() { break; }
                let Some(s) = sub.as_mut() else { continue };
                let Some(frame) = pos_rx.as_mut().and_then(|rx| rx.borrow_and_update().clone()) else { continue };
                // 帧与订阅的编目代数不一致时跳过(等待 catalog 消息先送达)
                if frame.generation != s.generation {
                    continue;
                }
                // 帧周期与订阅周期接近时留 10% 余量,避免因抖动而隔帧发送
                if s.last_sent.is_some_and(|t| t.elapsed() < s.min_interval.mul_f64(0.9)) {
                    continue;
                }
                let bytes = frame::encode(&frame, &s.indices, &s.filter);
                if socket.send(Message::Binary(bytes.into())).await.is_err() { break; }
                s.last_sent = Some(Instant::now());
            }
            // 指定时刻的连接:按自己的时钟计算位置帧
            _ = view_tick.tick(), if view.is_some() && sub.is_some() => {
                let (Some(s), Some(v)) = (sub.as_mut(), view.as_ref()) else { continue };
                if s.last_sent.is_some_and(|t| t.elapsed() < s.min_interval.mul_f64(0.9)) {
                    continue;
                }
                let cs = v.view.catalog.load_full();
                if cs.generation != s.generation {
                    continue;
                }
                let t = v.view.now();
                let Ok(frame) = tokio::task::spawn_blocking(move || {
                    crate::scheduler::compute_frame(&cs, t)
                })
                .await
                else {
                    continue;
                };
                let bytes = frame::encode(&frame, &s.indices, &s.filter);
                if socket.send(Message::Binary(bytes.into())).await.is_err() { break; }
                s.last_sent = Some(Instant::now());
            }
            // 视图时钟在走:定期按新的时刻重新选取数据
            _ = view_reload.tick(), if view.is_some() => {
                if let Some(v) = view.as_ref()
                    && !refresh_view(&mut socket, &state, v, &mut sub).await
                {
                    break;
                }
            }
            _ = shutdown.changed() => {
                let _ = socket.send(Message::Close(None)).await;
                break;
            }
        }
    }
    tracing::debug!("WS 连接关闭");
}
