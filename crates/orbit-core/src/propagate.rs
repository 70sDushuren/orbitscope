//! 单物体外推:SGP4 → TEME → ECEF → WGS84。

use crate::error::CoreError;
use crate::frames::{Geodetic, ecef_to_geodetic, norm, teme_to_ecef, teme_to_ecef_velocity};
use crate::model::NoradId;
use crate::time::{gmst_rad, minutes_since};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// 某时刻的状态。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct StateVector {
    pub time: DateTime<Utc>,
    /// TEME 位置(km)
    pub teme_pos_km: [f64; 3],
    /// TEME 速度(km/s)
    pub teme_vel_kms: [f64; 3],
    /// ECEF 位置(km)
    pub ecef_pos_km: [f64; 3],
    /// ECEF 速度(km/s,相对旋转地球)
    pub ecef_vel_kms: [f64; 3],
    /// WGS84 大地坐标
    pub geodetic: Geodetic,
    /// 惯性系速度大小(km/s)
    pub speed_kms: f64,
}

/// 在给定时刻外推。`gmst` 为该时刻的格林尼治平恒星时(批量外推时可共享,避免重复计算)。
pub fn propagate_with_gmst(
    constants: &sgp4::Constants,
    norad_id: NoradId,
    epoch: DateTime<Utc>,
    t: DateTime<Utc>,
    gmst: f64,
) -> Result<StateVector, CoreError> {
    let minutes = minutes_since(epoch, t);
    let p = constants
        .propagate(sgp4::MinutesSinceEpoch(minutes))
        .map_err(|e| CoreError::Propagation {
            norad_id,
            message: e.to_string(),
        })?;
    let r = p.position;
    let v = p.velocity;
    if !(r.iter().all(|x| x.is_finite()) && v.iter().all(|x| x.is_finite())) {
        return Err(CoreError::Propagation {
            norad_id,
            message: "结果非有限值".into(),
        });
    }
    let ecef = teme_to_ecef(r, gmst);
    Ok(StateVector {
        time: t,
        teme_pos_km: r,
        teme_vel_kms: v,
        ecef_pos_km: ecef,
        ecef_vel_kms: teme_to_ecef_velocity(r, v, gmst),
        geodetic: ecef_to_geodetic(ecef),
        speed_kms: norm(v),
    })
}

/// 在给定时刻外推(内部计算 GMST)。
pub fn propagate(
    constants: &sgp4::Constants,
    norad_id: NoradId,
    epoch: DateTime<Utc>,
    t: DateTime<Utc>,
) -> Result<StateVector, CoreError> {
    propagate_with_gmst(constants, norad_id, epoch, t, gmst_rad(t))
}

#[cfg(test)]
// 参考值按原文献格式书写,便于核对
#[allow(clippy::inconsistent_digit_grouping, clippy::needless_range_loop)]
mod tests {
    use super::*;

    /// sgp4 crate 自带的 Vallado 测试向量(00005,t = 0 与 360 min),验证分钟换算与 TEME 输出。
    ///
    /// 参考值由 AFSPC 兼容模式生成;本项目使用 sgp4 crate 推荐的默认(改进)模式,
    /// 两者差异为数十米量级(远小于 SGP4 本身的 km 级误差),因此容差取 0.1 km。
    #[test]
    fn vallado_00005() {
        let e = sgp4::Elements::from_tle(
            None,
            b"1 00005U 58002B   00179.78495062  .00000023  00000-0  28098-4 0  4753",
            b"2 00005  34.2682 348.7242 1859667 331.7664  19.3264 10.82419157413667",
        )
        .unwrap();
        let c = sgp4::Constants::from_elements(&e).unwrap();
        let epoch = e.datetime.and_utc();

        let s0 = propagate(&c, 5, epoch, epoch).unwrap();
        let expect0 = [7022.465_292_66, -1400.082_967_55, 0.039_951_55];
        for i in 0..3 {
            assert!(
                (s0.teme_pos_km[i] - expect0[i]).abs() < 0.1,
                "{:?}",
                s0.teme_pos_km
            );
        }

        let t = epoch + chrono::Duration::minutes(360);
        let s1 = propagate(&c, 5, epoch, t).unwrap();
        let expect1 = [-7154.031_202_02, -3783.176_825_04, -3536.194_122_94];
        for i in 0..3 {
            assert!(
                (s1.teme_pos_km[i] - expect1[i]).abs() < 0.1,
                "{:?}",
                s1.teme_pos_km
            );
        }
        // 大地高与 ECEF 模长一致性
        assert!(s1.geodetic.alt_km > 0.0);
    }
}
