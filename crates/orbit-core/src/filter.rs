//! 过滤条件。
//!
//! 分两级:
//! - **静态过滤**([`Filter::matches_static`]):只依赖根数与元数据,在编目或过滤条件变化时计算一次;
//! - **瞬时过滤**([`Filter::matches_instantaneous`]):`instantaneous` 模式下每次外推后按当前高度判定。

use crate::error::CoreError;
use crate::model::{ObjectType, SpaceObject};
use crate::size::SizeVerdict;
use crate::time::days_since;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// `instantaneous` 模式下静态预筛的高度余量(km)。
///
/// 由平根数算出的近/远地点高度相对赤道半径,而瞬时高度为 WGS84 椭球高(两极相差约 21 km),
/// 再加上 SGP4 短周期项,二者可相差数十 km;预筛时放宽该余量以免漏选。
pub const INSTANTANEOUS_PREFILTER_MARGIN_KM: f64 = 50.0;

/// 高度判据模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AltitudeMode {
    /// 轨道高度区间 [近地点, 远地点] 与范围有交集(默认)
    #[default]
    OrbitOverlap,
    /// 轨道完全位于范围内(近地点 ≥ 下限 且 远地点 ≤ 上限)
    OrbitWithin,
    /// 当前瞬时高度位于范围内
    Instantaneous,
}

/// 尺寸无法判定时的处理策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UnknownSizePolicy {
    #[default]
    Include,
    Exclude,
}

/// 过滤条件。所有字段均有默认值,可从 TOML/JSON 部分指定。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Filter {
    pub alt_min_km: f64,
    pub alt_max_km: f64,
    pub altitude_mode: AltitudeMode,
    /// 最小等效直径(cm);≤ 0 表示不过滤尺寸
    pub min_size_cm: f64,
    pub unknown_size: UnknownSizePolicy,
    /// 允许的类型;空表示全部
    pub object_types: Vec<ObjectType>,
    /// 允许的国家/所有者代码(不区分大小写);空表示全部
    pub countries: Vec<String>,
    pub inclination_min_deg: Option<f64>,
    pub inclination_max_deg: Option<f64>,
    /// 名称(子串,不区分大小写)/ NORAD 编号(精确)/ 国际编号(前缀)
    pub query: Option<String>,
    /// 根数龄期上限(天);超过则视为过期剔除。`None` 表示不限
    pub max_epoch_age_days: Option<f64>,
}

impl Default for Filter {
    fn default() -> Self {
        Self {
            alt_min_km: 100.0,
            alt_max_km: 1_000.0,
            altitude_mode: AltitudeMode::OrbitOverlap,
            min_size_cm: 10.0,
            unknown_size: UnknownSizePolicy::Include,
            object_types: Vec::new(),
            countries: Vec::new(),
            inclination_min_deg: None,
            inclination_max_deg: None,
            query: None,
            max_epoch_age_days: Some(14.0),
        }
    }
}

fn contains_ignore_ascii_case(haystack: &str, needle: &str) -> bool {
    let (h, n) = (haystack.as_bytes(), needle.as_bytes());
    n.is_empty() || h.windows(n.len()).any(|w| w.eq_ignore_ascii_case(n))
}

impl Filter {
    /// 不过滤任何物体的条件(高度 0–∞、无尺寸门限、不限龄期)。
    pub fn pass_all() -> Self {
        Self {
            alt_min_km: f64::NEG_INFINITY,
            alt_max_km: f64::INFINITY,
            min_size_cm: 0.0,
            max_epoch_age_days: None,
            ..Self::default()
        }
    }

    /// 校验参数合法性。
    pub fn validate(&self) -> Result<(), CoreError> {
        use crate::error::FilterIssue;
        let bad = |issue: FilterIssue| Err(CoreError::InvalidFilter(issue));
        if self.alt_min_km.is_nan() || self.alt_max_km.is_nan() {
            return bad(FilterIssue::AltNan);
        }
        if self.alt_min_km > self.alt_max_km {
            return bad(FilterIssue::AltOrder);
        }
        if !self.min_size_cm.is_finite() || self.min_size_cm < 0.0 {
            return bad(FilterIssue::MinSize);
        }
        if let (Some(lo), Some(hi)) = (self.inclination_min_deg, self.inclination_max_deg)
            && lo > hi
        {
            return bad(FilterIssue::InclinationOrder);
        }
        if self.max_epoch_age_days.is_some_and(|d| d.is_nan() || d < 0.0) {
            return bad(FilterIssue::EpochAge);
        }
        Ok(())
    }

    /// 静态过滤:不依赖当前位置的全部条件。`now` 用于计算根数龄期。
    ///
    /// `instantaneous` 模式下,高度条件以 [`INSTANTANEOUS_PREFILTER_MARGIN_KM`] 余量做轨道区间预筛。
    pub fn matches_static(&self, obj: &SpaceObject, now: DateTime<Utc>) -> bool {
        self.altitude_static_ok(obj)
            && self.type_ok(obj)
            && self.size_ok(obj)
            && self.country_ok(obj)
            && self.inclination_ok(obj)
            && self.epoch_ok(obj, now)
            && self.query_ok(obj)
    }

    /// 是否需要逐帧按瞬时高度判定。
    pub fn needs_instantaneous(&self) -> bool {
        self.altitude_mode == AltitudeMode::Instantaneous
    }

    /// 瞬时高度判定(仅 `instantaneous` 模式有意义;其他模式恒为 `true`)。
    pub fn matches_instantaneous(&self, alt_km: f64) -> bool {
        !self.needs_instantaneous() || (self.alt_min_km..=self.alt_max_km).contains(&alt_km)
    }

    fn altitude_static_ok(&self, obj: &SpaceObject) -> bool {
        let (hp, ha) = (obj.derived.perigee_alt_km, obj.derived.apogee_alt_km);
        match self.altitude_mode {
            AltitudeMode::OrbitOverlap => hp <= self.alt_max_km && ha >= self.alt_min_km,
            AltitudeMode::OrbitWithin => hp >= self.alt_min_km && ha <= self.alt_max_km,
            AltitudeMode::Instantaneous => {
                let m = INSTANTANEOUS_PREFILTER_MARGIN_KM;
                hp <= self.alt_max_km + m && ha >= self.alt_min_km - m
            }
        }
    }

    fn type_ok(&self, obj: &SpaceObject) -> bool {
        self.object_types.is_empty() || self.object_types.contains(&obj.meta.object_type)
    }

    fn size_ok(&self, obj: &SpaceObject) -> bool {
        match obj.size.check_min_diameter(self.min_size_cm / 100.0) {
            SizeVerdict::Pass => true,
            SizeVerdict::Fail => false,
            SizeVerdict::Undetermined => self.unknown_size == UnknownSizePolicy::Include,
        }
    }

    fn country_ok(&self, obj: &SpaceObject) -> bool {
        self.countries.is_empty()
            || obj
                .meta
                .country
                .as_deref()
                .is_some_and(|c| self.countries.iter().any(|f| f.eq_ignore_ascii_case(c)))
    }

    fn inclination_ok(&self, obj: &SpaceObject) -> bool {
        let i = obj.derived.inclination_deg;
        self.inclination_min_deg.is_none_or(|lo| i >= lo) && self.inclination_max_deg.is_none_or(|hi| i <= hi)
    }

    fn epoch_ok(&self, obj: &SpaceObject, now: DateTime<Utc>) -> bool {
        self.max_epoch_age_days
            .is_none_or(|max| days_since(obj.epoch, now).abs() <= max)
    }

    fn query_ok(&self, obj: &SpaceObject) -> bool {
        let Some(q) = self.query.as_deref().map(str::trim).filter(|q| !q.is_empty()) else {
            return true;
        };
        if let Ok(id) = q.parse::<u32>()
            && id == obj.norad_id
        {
            return true;
        }
        contains_ignore_ascii_case(&obj.name, q)
            || obj.meta.intl_designator.as_deref().is_some_and(|d| {
                d.len() >= q.len() && d.as_bytes()[..q.len()].eq_ignore_ascii_case(q.as_bytes())
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_valid() {
        Filter::default().validate().unwrap();
        Filter::pass_all().validate().unwrap();
    }

    #[test]
    fn invalid_ranges() {
        let f = Filter {
            alt_min_km: 500.0,
            alt_max_km: 400.0,
            ..Default::default()
        };
        assert!(f.validate().is_err());
        let f = Filter {
            min_size_cm: -1.0,
            ..Default::default()
        };
        assert!(f.validate().is_err());
    }

    #[test]
    fn toml_like_partial_json() {
        let f: Filter = serde_json::from_str(
            r#"{"alt_max_km":600,"altitude_mode":"instantaneous","object_types":["DEB"]}"#,
        )
        .unwrap();
        assert_eq!(f.alt_min_km, 100.0);
        assert_eq!(f.alt_max_km, 600.0);
        assert_eq!(f.altitude_mode, AltitudeMode::Instantaneous);
        assert_eq!(f.object_types, vec![ObjectType::Debris]);
        assert!(serde_json::from_str::<Filter>(r#"{"typo":1}"#).is_err());
    }

    #[test]
    fn ignore_case_contains() {
        assert!(contains_ignore_ascii_case("ISS (ZARYA)", "zarya"));
        assert!(!contains_ignore_ascii_case("ISS", "zarya"));
        assert!(contains_ignore_ascii_case("x", ""));
    }
}
