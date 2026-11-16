//! 由平根数推导轨道几何量(只依赖根数,不需要外推)。

use crate::frames::WGS84_A_KM;
use serde::{Deserialize, Serialize};
use std::f64::consts::TAU;

/// 地球引力常数 μ(km³/s²),WGS84。
pub const MU_EARTH_KM3_S2: f64 = 398_600.441_8;

/// 由根数推导的轨道参数。高度相对地球赤道半径(6378.137 km)。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DerivedOrbit {
    pub mean_motion_rev_day: f64,
    pub eccentricity: f64,
    pub inclination_deg: f64,
    pub semi_major_axis_km: f64,
    pub period_min: f64,
    pub perigee_alt_km: f64,
    pub apogee_alt_km: f64,
}

impl DerivedOrbit {
    /// `mean_motion_rev_day`:平均运动(圈/天);`eccentricity`;`inclination_deg`。
    ///
    /// 对 `mean_motion_rev_day <= 0` 的非法输入,几何量为 NaN(调用方负责剔除)。
    pub fn from_mean_elements(mean_motion_rev_day: f64, eccentricity: f64, inclination_deg: f64) -> Self {
        let n_rad_s = mean_motion_rev_day * TAU / 86_400.0;
        let a = if n_rad_s > 0.0 {
            (MU_EARTH_KM3_S2 / (n_rad_s * n_rad_s)).cbrt()
        } else {
            f64::NAN
        };
        let period_min = if mean_motion_rev_day > 0.0 {
            1_440.0 / mean_motion_rev_day
        } else {
            f64::NAN
        };
        Self {
            mean_motion_rev_day,
            eccentricity,
            inclination_deg,
            semi_major_axis_km: a,
            period_min,
            perigee_alt_km: a * (1.0 - eccentricity) - WGS84_A_KM,
            apogee_alt_km: a * (1.0 + eccentricity) - WGS84_A_KM,
        }
    }

    /// 由 sgp4 根数构造。
    pub fn from_elements(e: &sgp4::Elements) -> Self {
        Self::from_mean_elements(e.mean_motion, e.eccentricity, e.inclination)
    }

    /// 是否为深空轨道(周期 ≥ 225 分钟,SGP4 将使用 SDP4)。
    pub fn is_deep_space(&self) -> bool {
        self.period_min >= 225.0
    }

    /// 根数几何量是否有限(用于剔除非法记录)。
    pub fn is_valid(&self) -> bool {
        self.semi_major_axis_km.is_finite()
            && self.perigee_alt_km.is_finite()
            && self.apogee_alt_km.is_finite()
            && (0.0..1.0).contains(&self.eccentricity)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iss_like_orbit() {
        let d = DerivedOrbit::from_mean_elements(15.498_155_14, 0.000_670_3, 51.6416);
        // Space-Track 对同一组根数给出 PERIAPSIS ≈ 413 km、APOAPSIS ≈ 422 km
        assert!((d.perigee_alt_km - 413.0).abs() < 5.0, "{d:?}");
        assert!((d.apogee_alt_km - 422.0).abs() < 5.0, "{d:?}");
        assert!((d.period_min - 92.915).abs() < 0.01);
        assert!(!d.is_deep_space());
        assert!(d.is_valid());
    }

    #[test]
    fn geo_orbit() {
        let d = DerivedOrbit::from_mean_elements(1.002_737_8, 0.0, 0.0);
        assert!((d.semi_major_axis_km - 42_164.0).abs() < 2.0, "{d:?}");
        assert!(d.is_deep_space());
    }

    #[test]
    fn invalid_mean_motion() {
        let d = DerivedOrbit::from_mean_elements(0.0, 0.0, 0.0);
        assert!(!d.is_valid());
    }
}
