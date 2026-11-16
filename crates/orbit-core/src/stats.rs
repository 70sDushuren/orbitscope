//! 统计:按类型/国家计数、高度直方图。

use crate::model::SpaceObject;
use serde::Serialize;
use std::collections::BTreeMap;

/// 统计结果。
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Stats {
    pub total: usize,
    pub by_type: BTreeMap<String, usize>,
    pub by_country: BTreeMap<String, usize>,
    /// 高度直方图分箱宽度(km)
    pub bin_km: f64,
    /// (分箱下界 km, 数量),按下界升序;高度取 (近地点 + 远地点) / 2
    pub altitude_histogram: Vec<(f64, usize)>,
}

impl Stats {
    /// 对给定物体集合统计。`bin_km` ≤ 0 时取 50 km。
    pub fn compute<'a>(objects: impl IntoIterator<Item = &'a SpaceObject>, bin_km: f64) -> Self {
        let bin_km = if bin_km > 0.0 && bin_km.is_finite() {
            bin_km
        } else {
            50.0
        };
        let mut s = Stats {
            bin_km,
            ..Default::default()
        };
        let mut hist: BTreeMap<i64, usize> = BTreeMap::new();
        for o in objects {
            s.total += 1;
            *s.by_type
                .entry(o.meta.object_type.as_str().to_string())
                .or_default() += 1;
            let country = o.meta.country.clone().unwrap_or_else(|| "UNKNOWN".into());
            *s.by_country.entry(country).or_default() += 1;
            let mean_alt = 0.5 * (o.derived.perigee_alt_km + o.derived.apogee_alt_km);
            if mean_alt.is_finite() {
                *hist.entry((mean_alt / bin_km).floor() as i64).or_default() += 1;
            }
        }
        s.altitude_histogram = hist.into_iter().map(|(k, n)| (k as f64 * bin_km, n)).collect();
        s
    }
}
