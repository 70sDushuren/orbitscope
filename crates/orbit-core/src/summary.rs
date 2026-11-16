//! 物体在某时刻的扁平化摘要(CLI 的 CSV/JSON 输出与 Web API 共用)。

use crate::model::SpaceObject;
use crate::propagate::StateVector;
use crate::size::SizeSource;
use crate::time::days_since;
use chrono::{DateTime, SecondsFormat, Utc};
use serde::Serialize;

/// 一个物体在某时刻的扁平化输出行(CSV / JSON / 表格共用)。
#[derive(Debug, Clone, Serialize)]
pub struct ObjectSummary {
    pub norad_id: u32,
    pub name: String,
    pub object_type: &'static str,
    pub country: Option<String>,
    pub intl_designator: Option<String>,
    pub source: String,
    pub epoch: String,
    pub epoch_age_days: f64,
    /// 根数的获取时间
    pub fetched_at: Option<String>,
    pub perigee_km: f64,
    pub apogee_km: f64,
    pub inclination_deg: f64,
    pub period_min: f64,
    pub size_min_m: Option<f64>,
    pub size_max_m: Option<f64>,
    pub size_source: SizeSource,
    pub time: String,
    pub lat_deg: Option<f64>,
    pub lon_deg: Option<f64>,
    pub alt_km: Option<f64>,
    pub speed_kms: Option<f64>,
}

fn round(v: f64, digits: i32) -> f64 {
    let p = 10f64.powi(digits);
    (v * p).round() / p
}

pub fn fmt_time(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Secs, true)
}

impl ObjectSummary {
    pub fn new(o: &SpaceObject, state: Option<&StateVector>, t: DateTime<Utc>) -> Self {
        Self {
            norad_id: o.norad_id,
            name: o.name.clone(),
            object_type: o.meta.object_type.as_str(),
            country: o.meta.country.clone(),
            intl_designator: o.meta.intl_designator.clone(),
            source: o.source.to_string(),
            epoch: fmt_time(o.epoch),
            epoch_age_days: round(days_since(o.epoch, t), 3),
            fetched_at: o.fetched_at.map(fmt_time),
            perigee_km: round(o.derived.perigee_alt_km, 1),
            apogee_km: round(o.derived.apogee_alt_km, 1),
            inclination_deg: round(o.derived.inclination_deg, 4),
            period_min: round(o.derived.period_min, 3),
            size_min_m: o.size.min_diameter_m.map(|v| round(v, 3)),
            size_max_m: o.size.max_diameter_m.map(|v| round(v, 3)),
            size_source: o.size.source,
            time: fmt_time(t),
            lat_deg: state.map(|s| round(s.geodetic.lat_deg, 4)),
            lon_deg: state.map(|s| round(s.geodetic.lon_deg, 4)),
            alt_km: state.map(|s| round(s.geodetic.alt_km, 2)),
            speed_kms: state.map(|s| round(s.speed_kms, 4)),
        }
    }
}
