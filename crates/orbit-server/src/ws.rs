//! WebSocket 实时位置推送(`/api/v1/stream`)。
//!
//! 客户端 → 服务端(JSON 文本):
//! - `{"type":"subscribe","filter":{...局部过滤条件...},"rate_hz":1}`:订阅(可重复发送以更换条件)
//! - `{"type":"ping"}`
//!
//! 服务端 → 客户端:
//! - JSON `{"type":"hello",...}`:连接建立后立即发送
//! - JSON `{"type":"catalog",...}`:订阅时与编目更新时发送物体元数据(`i` 为帧中使用的编目下标)
//! - 二进制位置帧(格式见 [`crate::frame`])
//! - JSON `{"type":"error","message":...}` / `{"type":"pong"}`

use crate::frame;
use crate::query::merge_filter;
use crate::state::{AppState, CatalogState};
use axum::extract::ws::{Message, WebSocket};
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
        objects: Vec<WsObject>,
    },
    Error {
        message: String,
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

/// 为订阅计算下标集合并发送元数据。
async fn send_catalog(socket: &mut WebSocket, state: &AppState, sub: &mut Subscription) -> bool {
    let cs = state.catalog.load_full();
    sub.indices = cs.catalog.select(&sub.filter, state.now());
    sub.generation = cs.generation;
    let rate_hz = 1.0 / sub.min_interval.as_secs_f64();
    let msg = ServerMsg::Catalog {
        generation: cs.generation,
        total: sub.indices.len(),
        filter: &sub.filter,
        rate_hz,
        objects: catalog_objects(&cs, &sub.indices),
    };
    send_json(socket, &msg).await
}

pub async fn handle_socket(mut socket: WebSocket, state: Arc<AppState>, _guard: ClientGuard) {
    let mut pos_rx = state.positions.subscribe();
    let mut gen_rx = state.catalog_gen.subscribe();
    let mut shutdown = state.shutdown.subscribe();
    let mut sub: Option<Subscription> = None;

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
                    Ok(ClientMsg::Subscribe { filter, rate_hz }) => {
                        let base = state.default_filter();
                        let filter = match merge_filter(&base, &filter) {
                            Ok(f) => f,
                            Err(message) => {
                                if !send_json(&mut socket, &ServerMsg::Error { message }).await { break; }
                                continue;
                            }
                        };
                        let rate = rate_hz.unwrap_or(1.0);
                        if !rate.is_finite() {
                            if !send_json(&mut socket, &ServerMsg::Error { message: "rate_hz 无效".into() }).await { break; }
                            continue;
                        }
                        let rate = rate.clamp(MIN_RATE_HZ, MAX_RATE_HZ);
                        let mut s = Subscription {
                            filter,
                            indices: Vec::new(),
                            generation: 0,
                            min_interval: Duration::from_secs_f64(1.0 / rate),
                            last_sent: None,
                        };
                        if !send_catalog(&mut socket, &state, &mut s).await { break; }
                        sub = Some(s);
                    }
                    Err(e) => {
                        let message = format!("无法解析消息: {e}");
                        if !send_json(&mut socket, &ServerMsg::Error { message }).await { break; }
                    }
                }
            }
            changed = gen_rx.changed() => {
                if changed.is_err() { break; }
                if let Some(s) = sub.as_mut()
                    && !send_catalog(&mut socket, &state, s).await
                {
                    break;
                }
            }
            changed = pos_rx.changed() => {
                if changed.is_err() { break; }
                let Some(s) = sub.as_mut() else { continue };
                let Some(frame) = pos_rx.borrow_and_update().clone() else { continue };
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
            _ = shutdown.changed() => {
                let _ = socket.send(Message::Close(None)).await;
                break;
            }
        }
    }
    tracing::debug!("WS 连接关闭");
}
