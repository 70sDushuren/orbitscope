//! WebSocket 二进制位置帧(小端)。
//!
//! ```text
//! 偏移  长度  内容
//! 0     4     magic "OSF1"
//! 4     4     u32 版本号(=1)
//! 8     4     u32 编目代数(低 32 位)
//! 12    8     f64 Unix 时间(秒)
//! 20    4     u32 条目数 N
//! 24    16×N  { u32 编目下标; f32 x_km; f32 y_km; f32 z_km }   // ECEF
//! ```
//!
//! 前端用 ECEF 直接给 Cesium(Cartesian3 单位 m,×1000)。f32 在 7000 km 量级精度约 0.5 m。

use crate::state::PositionFrame;
use chrono::{DateTime, Utc};
use orbit_core::Filter;

pub const MAGIC: [u8; 4] = *b"OSF1";
pub const VERSION: u32 = 1;
pub const HEADER_LEN: usize = 24;
pub const ENTRY_LEN: usize = 16;

/// 按订阅的下标集合编码一帧;外推失败(NaN)或不满足瞬时高度条件的物体跳过。
pub fn encode(frame: &PositionFrame, indices: &[usize], filter: &Filter) -> Vec<u8> {
    let mut buf = Vec::with_capacity(HEADER_LEN + ENTRY_LEN * indices.len());
    buf.extend_from_slice(&MAGIC);
    buf.extend_from_slice(&VERSION.to_le_bytes());
    buf.extend_from_slice(&(frame.generation as u32).to_le_bytes());
    let t = frame.time.timestamp() as f64 + f64::from(frame.time.timestamp_subsec_nanos()) * 1e-9;
    buf.extend_from_slice(&t.to_le_bytes());
    let count_pos = buf.len();
    buf.extend_from_slice(&0u32.to_le_bytes());

    let mut count: u32 = 0;
    for &i in indices {
        let (Some(p), Some(&alt)) = (frame.ecef_km.get(i), frame.alt_km.get(i)) else {
            continue;
        };
        if !alt.is_finite() || !filter.matches_instantaneous(f64::from(alt)) {
            continue;
        }
        buf.extend_from_slice(&(i as u32).to_le_bytes());
        for c in p {
            buf.extend_from_slice(&c.to_le_bytes());
        }
        count += 1;
    }
    buf[count_pos..count_pos + 4].copy_from_slice(&count.to_le_bytes());
    buf
}

/// 解码结果(供测试与其他 Rust 客户端使用)。
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedFrame {
    pub generation: u32,
    pub time: DateTime<Utc>,
    pub entries: Vec<(u32, [f32; 3])>,
}

pub fn decode(b: &[u8]) -> Result<DecodedFrame, String> {
    let u32_at = |o: usize| -> Result<u32, String> {
        b.get(o..o + 4)
            .and_then(|s| s.try_into().ok())
            .map(u32::from_le_bytes)
            .ok_or_else(|| format!("帧长度不足(偏移 {o})"))
    };
    let f32_at = |o: usize| u32_at(o).map(f32::from_bits);
    if b.len() < HEADER_LEN || b[..4] != MAGIC {
        return Err("不是 OSF1 帧".into());
    }
    if u32_at(4)? != VERSION {
        return Err(format!("不支持的版本 {}", u32_at(4)?));
    }
    let generation = u32_at(8)?;
    let secs = f64::from_le_bytes(b[12..20].try_into().map_err(|_| "时间字段错误")?);
    let time = DateTime::from_timestamp_millis((secs * 1000.0).round() as i64).ok_or("时间超出范围")?;
    let n = u32_at(20)? as usize;
    if b.len() != HEADER_LEN + n * ENTRY_LEN {
        return Err(format!("帧长度 {} 与条目数 {n} 不符", b.len()));
    }
    let mut entries = Vec::with_capacity(n);
    for k in 0..n {
        let o = HEADER_LEN + k * ENTRY_LEN;
        entries.push((u32_at(o)?, [f32_at(o + 4)?, f32_at(o + 8)?, f32_at(o + 12)?]));
    }
    Ok(DecodedFrame {
        generation,
        time,
        entries,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbit_core::AltitudeMode;

    fn frame() -> PositionFrame {
        PositionFrame {
            generation: 7,
            time: DateTime::from_timestamp(1_704_112_200, 500_000_000).unwrap(),
            ecef_km: vec![[1.0, 2.0, 3.0], [f32::NAN; 3], [7000.5, -1.25, 0.0]],
            alt_km: vec![420.0, f32::NAN, 900.0],
        }
    }

    #[test]
    fn round_trip_skips_failed() {
        let f = frame();
        let bytes = encode(&f, &[0, 1, 2], &Filter::pass_all());
        assert_eq!(bytes.len(), HEADER_LEN + 2 * ENTRY_LEN);
        let d = decode(&bytes).unwrap();
        assert_eq!(d.generation, 7);
        assert_eq!(d.time, f.time);
        assert_eq!(d.entries, vec![(0, [1.0, 2.0, 3.0]), (2, [7000.5, -1.25, 0.0])]);
    }

    #[test]
    fn instantaneous_filter_applies() {
        let filter = Filter {
            altitude_mode: AltitudeMode::Instantaneous,
            alt_min_km: 800.0,
            alt_max_km: 1000.0,
            ..Filter::pass_all()
        };
        let d = decode(&encode(&frame(), &[0, 2], &filter)).unwrap();
        assert_eq!(d.entries.len(), 1);
        assert_eq!(d.entries[0].0, 2);
    }

    #[test]
    fn rejects_garbage() {
        assert!(decode(b"nope").is_err());
        let mut b = encode(&frame(), &[0], &Filter::pass_all());
        b.pop();
        assert!(decode(&b).is_err());
    }
}
