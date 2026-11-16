//! 时间相关计算。时间统一使用 `DateTime<Utc>`;UT1 近似为 UTC(误差 < 0.9 s,对应地面位置 < 0.4 km)。

use chrono::{DateTime, NaiveDateTime, Utc};
use std::f64::consts::TAU;

/// Unix 纪元(1970-01-01T00:00:00Z)对应的儒略日。
pub const JD_UNIX_EPOCH: f64 = 2_440_587.5;
/// J2000.0 对应的儒略日。
pub const JD_J2000: f64 = 2_451_545.0;

/// 计算儒略日(UTC 时标)。
pub fn julian_date(t: DateTime<Utc>) -> f64 {
    let secs = t.timestamp() as f64 + f64::from(t.timestamp_subsec_nanos()) * 1e-9;
    JD_UNIX_EPOCH + secs / 86_400.0
}

/// 格林尼治平恒星时(IAU-1982 模型,与 SGP4/TEME 配套),单位弧度,范围 [0, 2π)。
///
/// 参考 Vallado, *Fundamentals of Astrodynamics and Applications*, Eq. 3-47。
pub fn gmst_rad(t: DateTime<Utc>) -> f64 {
    let t_ut1 = (julian_date(t) - JD_J2000) / 36_525.0;
    let gmst_s =
        67_310.548_41 + (876_600.0 * 3_600.0 + 8_640_184.812_866) * t_ut1 + 0.093_104 * t_ut1 * t_ut1
            - 6.2e-6 * t_ut1 * t_ut1 * t_ut1;
    // 1 秒恒星时 = 1/240 度
    (gmst_s.rem_euclid(86_400.0) / 240.0).to_radians().rem_euclid(TAU)
}

/// 把 GP 根数的 epoch(无时区,约定为 UTC)转换为 `DateTime<Utc>`。
pub fn epoch_to_utc(epoch: NaiveDateTime) -> DateTime<Utc> {
    epoch.and_utc()
}

/// `t` 相对 `epoch` 的分钟数(可为负)。
pub fn minutes_since(epoch: DateTime<Utc>, t: DateTime<Utc>) -> f64 {
    let d = t - epoch;
    match d.num_nanoseconds() {
        Some(ns) => ns as f64 / 60e9,
        // 超出 i64 纳秒范围(约 ±292 年)时退化为毫秒精度
        None => d.num_milliseconds() as f64 / 60e3,
    }
}

/// `t` 相对 `epoch` 的天数(可为负)。
pub fn days_since(epoch: DateTime<Utc>, t: DateTime<Utc>) -> f64 {
    minutes_since(epoch, t) / 1_440.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn julian_date_of_j2000() {
        let t = Utc.with_ymd_and_hms(2000, 1, 1, 12, 0, 0).unwrap();
        assert!((julian_date(t) - JD_J2000).abs() < 1e-9);
    }

    /// Vallado Example 3-5:1992-08-20 12:14:00 UT1 → GMST = 152.578787810°
    #[test]
    fn gmst_vallado_example_3_5() {
        let t = Utc.with_ymd_and_hms(1992, 8, 20, 12, 14, 0).unwrap();
        let gmst_deg = gmst_rad(t).to_degrees();
        assert!((gmst_deg - 152.578_787_810).abs() < 1e-6, "gmst = {gmst_deg}");
    }

    #[test]
    fn minutes_since_is_signed() {
        let a = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        let b = Utc.with_ymd_and_hms(2024, 1, 1, 1, 30, 0).unwrap();
        assert_eq!(minutes_since(a, b), 90.0);
        assert_eq!(minutes_since(b, a), -90.0);
        assert_eq!(days_since(a, b), 90.0 / 1440.0);
    }
}
