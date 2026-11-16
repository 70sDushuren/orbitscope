//! 坐标系转换:TEME → ECEF(PEF)→ WGS84 大地坐标。
//!
//! TEME → PEF 仅绕 Z 轴旋转 GMST;忽略极移(PEF 与 ITRF 相差约 10 m 级,远小于 SGP4 本身的 km 级误差)。

use serde::{Deserialize, Serialize};

/// WGS84 长半轴(km)。
pub const WGS84_A_KM: f64 = 6_378.137;
/// WGS84 扁率。
pub const WGS84_F: f64 = 1.0 / 298.257_223_563;
/// WGS84 第一偏心率平方。
pub const WGS84_E2: f64 = WGS84_F * (2.0 - WGS84_F);
/// 地球自转角速度(rad/s),IAU-82 / Vallado 取值。
pub const EARTH_ROTATION_RAD_S: f64 = 7.292_115_146_706_979e-5;

/// WGS84 大地坐标。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Geodetic {
    /// 大地纬度,[-90, 90]
    pub lat_deg: f64,
    /// 经度,(-180, 180]
    pub lon_deg: f64,
    /// 椭球高(km)
    pub alt_km: f64,
}

/// TEME 位置 → ECEF(PEF)位置。`gmst_rad` 见 [`crate::time::gmst_rad`]。
#[inline]
pub fn teme_to_ecef(r_teme: [f64; 3], gmst_rad: f64) -> [f64; 3] {
    let (s, c) = gmst_rad.sin_cos();
    [
        c * r_teme[0] + s * r_teme[1],
        -s * r_teme[0] + c * r_teme[1],
        r_teme[2],
    ]
}

/// TEME 速度 → ECEF(PEF)速度(扣除地球自转引起的牵连速度 ω × r)。
#[inline]
pub fn teme_to_ecef_velocity(r_teme: [f64; 3], v_teme: [f64; 3], gmst_rad: f64) -> [f64; 3] {
    let r = teme_to_ecef(r_teme, gmst_rad);
    let v = teme_to_ecef(v_teme, gmst_rad);
    let w = EARTH_ROTATION_RAD_S;
    [v[0] + w * r[1], v[1] - w * r[0], v[2]]
}

/// ECEF(km)→ WGS84 大地坐标。迭代法,6 次迭代后误差远小于 1 mm(LEO 到 GEO 范围)。
pub fn ecef_to_geodetic(r: [f64; 3]) -> Geodetic {
    let [x, y, z] = r;
    let p = x.hypot(y);
    let lon = y.atan2(x);

    // 初值:球面近似修正
    let mut lat = z.atan2(p * (1.0 - WGS84_E2));
    let mut n = WGS84_A_KM;
    for _ in 0..6 {
        let sin_lat = lat.sin();
        n = WGS84_A_KM / (1.0 - WGS84_E2 * sin_lat * sin_lat).sqrt();
        lat = (z + WGS84_E2 * n * sin_lat).atan2(p);
    }
    let (sin_lat, cos_lat) = lat.sin_cos();
    // 对极区与赤道均数值稳定的高程公式
    let alt = p * cos_lat + (z + WGS84_E2 * n * sin_lat) * sin_lat - n;

    Geodetic {
        lat_deg: lat.to_degrees(),
        lon_deg: lon.to_degrees(),
        alt_km: alt,
    }
}

/// WGS84 大地坐标 → ECEF(km)。
pub fn geodetic_to_ecef(g: Geodetic) -> [f64; 3] {
    let lat = g.lat_deg.to_radians();
    let lon = g.lon_deg.to_radians();
    let (sin_lat, cos_lat) = lat.sin_cos();
    let n = WGS84_A_KM / (1.0 - WGS84_E2 * sin_lat * sin_lat).sqrt();
    [
        (n + g.alt_km) * cos_lat * lon.cos(),
        (n + g.alt_km) * cos_lat * lon.sin(),
        (n * (1.0 - WGS84_E2) + g.alt_km) * sin_lat,
    ]
}

/// 向量模长。
#[inline]
pub fn norm(v: [f64; 3]) -> f64 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

#[cfg(test)]
// 参考值按原文献格式书写,便于核对
#[allow(clippy::inconsistent_digit_grouping, clippy::needless_range_loop)]
mod tests {
    use super::*;
    use crate::time::gmst_rad;
    use chrono::{TimeZone, Utc};

    fn assert_vec_close(a: [f64; 3], b: [f64; 3], tol: f64) {
        for i in 0..3 {
            assert!((a[i] - b[i]).abs() < tol, "{a:?} vs {b:?} (tol {tol})");
        }
    }

    /// Vallado et al. 2006 (Revisiting Spacetrack Report #3) 例:
    /// 2004-04-06 07:51:28.386009 UTC,TEME r = (5094.18016210, 6127.64465950, 6380.34453270) km,
    /// PEF r = (-1033.47503130, 7901.30558560, 6380.34453270) km(用 UT1,ΔUT1 = -0.4399619 s)。
    /// 本实现用 UTC 近似 UT1,地面位置差约 0.26 km,容差取 0.5 km。
    #[test]
    fn teme_to_pef_vallado() {
        let t = Utc
            .with_ymd_and_hms(2004, 4, 6, 7, 51, 28)
            .unwrap()
            .checked_add_signed(chrono::Duration::nanoseconds(386_009_000))
            .unwrap();
        let r_teme = [5094.180_162_10, 6127.644_659_50, 6380.344_532_70];
        let r_pef = teme_to_ecef(r_teme, gmst_rad(t));
        assert_vec_close(r_pef, [-1033.475_031_30, 7901.305_585_60, 6380.344_532_70], 0.5);
        // 旋转不改变模长
        assert!((norm(r_pef) - norm(r_teme)).abs() < 1e-9);
    }

    #[test]
    fn geodetic_known_points() {
        // 赤道、本初子午线、海平面
        let g = ecef_to_geodetic([WGS84_A_KM, 0.0, 0.0]);
        assert!(g.lat_deg.abs() < 1e-12 && g.lon_deg.abs() < 1e-12 && g.alt_km.abs() < 1e-9);

        // 北极点上空 100 km:极半径 b = a(1-f)
        let b = WGS84_A_KM * (1.0 - WGS84_F);
        let g = ecef_to_geodetic([0.0, 0.0, b + 100.0]);
        assert!((g.lat_deg - 90.0).abs() < 1e-9);
        assert!((g.alt_km - 100.0).abs() < 1e-6, "{g:?}");
    }

    #[test]
    fn geodetic_round_trip() {
        for &lat in &[-89.9, -60.0, -23.5, 0.0, 12.3, 45.0, 80.0, 89.99] {
            for &lon in &[-179.0, -90.0, 0.0, 33.3, 120.0, 180.0] {
                for &alt in &[0.0, 100.0, 408.0, 1000.0, 35_786.0] {
                    let g = Geodetic {
                        lat_deg: lat,
                        lon_deg: lon,
                        alt_km: alt,
                    };
                    let back = ecef_to_geodetic(geodetic_to_ecef(g));
                    assert!((back.lat_deg - lat).abs() < 1e-9, "{g:?} -> {back:?}");
                    assert!((back.alt_km - alt).abs() < 1e-6, "{g:?} -> {back:?}");
                    let dlon = (back.lon_deg - lon).rem_euclid(360.0);
                    assert!(dlon < 1e-9 || (360.0 - dlon) < 1e-9, "{g:?} -> {back:?}");
                }
            }
        }
    }

    #[test]
    fn ecef_velocity_removes_earth_rotation() {
        // 一个与地球同步旋转的赤道点,ECEF 速度应为 0
        let r = [42_164.0, 0.0, 0.0];
        let v = [0.0, EARTH_ROTATION_RAD_S * 42_164.0, 0.0];
        let v_ecef = teme_to_ecef_velocity(r, v, 0.0);
        assert_vec_close(v_ecef, [0.0, 0.0, 0.0], 1e-12);
    }
}
