//! 尺寸估算:公开编目没有物体物理尺寸,这里由 RCS 推算**等效直径区间**。
//!
//! 模型:光学区球体近似 `RCS ≈ π·d²/4` ⇒ `d = 2·√(RCS/π)`。
//! 10 cm 对应 RCS ≈ 0.00785 m²。后续可替换为 NASA 尺寸估算模型(SEM)。

use crate::model::{RcsCategory, RcsInfo};
use serde::{Deserialize, Serialize};
use std::f64::consts::PI;

/// 尺寸估算的依据。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SizeSource {
    NumericRcs,
    RcsCategory,
    Unknown,
}

/// 等效直径区间(m)。`None` 表示该侧无界。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SizeEstimate {
    pub min_diameter_m: Option<f64>,
    pub max_diameter_m: Option<f64>,
    pub source: SizeSource,
}

/// 尺寸门限判定结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SizeVerdict {
    /// 确定不小于门限
    Pass,
    /// 确定小于门限
    Fail,
    /// 无法判定(尺寸未知,或区间跨越门限)
    Undetermined,
}

/// RCS(m²)→ 等效直径(m)。
#[inline]
pub fn rcs_to_diameter_m(rcs_m2: f64) -> f64 {
    2.0 * (rcs_m2 / PI).sqrt()
}

/// 等效直径(m)→ RCS(m²)。
#[inline]
pub fn diameter_to_rcs_m2(d_m: f64) -> f64 {
    PI * d_m * d_m / 4.0
}

impl SizeEstimate {
    pub const UNKNOWN: Self = Self {
        min_diameter_m: None,
        max_diameter_m: None,
        source: SizeSource::Unknown,
    };

    pub fn from_rcs(rcs: &RcsInfo) -> Self {
        match *rcs {
            RcsInfo::Numeric { m2 } if m2.is_finite() && m2 > 0.0 => {
                let d = rcs_to_diameter_m(m2);
                Self {
                    min_diameter_m: Some(d),
                    max_diameter_m: Some(d),
                    source: SizeSource::NumericRcs,
                }
            }
            RcsInfo::Numeric { .. } | RcsInfo::Unknown => Self::UNKNOWN,
            RcsInfo::Category { category } => Self::from_category(category),
        }
    }

    fn from_category(c: RcsCategory) -> Self {
        let (lo, hi) = c.rcs_range_m2();
        Self {
            min_diameter_m: Some(lo.map_or(0.0, rcs_to_diameter_m)),
            max_diameter_m: hi.map(rcs_to_diameter_m),
            source: SizeSource::RcsCategory,
        }
    }

    /// 与最小直径门限 `min_diameter_m` 比较。门限 ≤ 0 时总是 `Pass`。
    pub fn check_min_diameter(&self, min_diameter_m: f64) -> SizeVerdict {
        if min_diameter_m <= 0.0 {
            return SizeVerdict::Pass;
        }
        if self.min_diameter_m.is_some_and(|lo| lo >= min_diameter_m) {
            return SizeVerdict::Pass;
        }
        if self.max_diameter_m.is_some_and(|hi| hi < min_diameter_m) {
            return SizeVerdict::Fail;
        }
        SizeVerdict::Undetermined
    }

    /// 代表值(m):数值 RCS 取其值;分档取区间几何中值或已知边界;未知为 `None`。
    pub fn representative_m(&self) -> Option<f64> {
        match (self.min_diameter_m, self.max_diameter_m) {
            (Some(lo), Some(hi)) if lo > 0.0 => Some((lo * hi).sqrt()),
            (_, Some(hi)) => Some(hi),
            (Some(lo), None) => Some(lo),
            (None, None) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ten_cm_rcs() {
        let rcs = diameter_to_rcs_m2(0.10);
        assert!((rcs - 0.007_853_98).abs() < 1e-8);
        assert!((rcs_to_diameter_m(rcs) - 0.10).abs() < 1e-12);
    }

    #[test]
    fn numeric_rcs_verdicts() {
        let s = SizeEstimate::from_rcs(&RcsInfo::Numeric { m2: 0.02 }); // d ≈ 0.16 m
        assert_eq!(s.source, SizeSource::NumericRcs);
        assert_eq!(s.check_min_diameter(0.10), SizeVerdict::Pass);
        assert_eq!(s.check_min_diameter(0.20), SizeVerdict::Fail);
        assert_eq!(s.check_min_diameter(0.0), SizeVerdict::Pass);
    }

    #[test]
    fn category_verdicts() {
        let small = SizeEstimate::from_rcs(&RcsInfo::Category {
            category: RcsCategory::Small,
        });
        // SMALL 区间 [0, ~0.357 m]:10 cm 落在区间内 → 无法判定
        assert_eq!(small.check_min_diameter(0.10), SizeVerdict::Undetermined);
        assert_eq!(small.check_min_diameter(0.50), SizeVerdict::Fail);

        let medium = SizeEstimate::from_rcs(&RcsInfo::Category {
            category: RcsCategory::Medium,
        });
        assert_eq!(medium.check_min_diameter(0.10), SizeVerdict::Pass);
        assert_eq!(medium.check_min_diameter(0.50), SizeVerdict::Undetermined);
        assert_eq!(medium.check_min_diameter(2.0), SizeVerdict::Fail);

        let large = SizeEstimate::from_rcs(&RcsInfo::Category {
            category: RcsCategory::Large,
        });
        assert_eq!(large.check_min_diameter(1.0), SizeVerdict::Pass);
        assert_eq!(large.check_min_diameter(5.0), SizeVerdict::Undetermined);
        assert!((large.min_diameter_m.unwrap() - 1.128).abs() < 1e-3);
    }

    #[test]
    fn unknown_and_bad_rcs() {
        for rcs in [
            RcsInfo::Unknown,
            RcsInfo::Numeric { m2: 0.0 },
            RcsInfo::Numeric { m2: f64::NAN },
        ] {
            let s = SizeEstimate::from_rcs(&rcs);
            assert_eq!(s, SizeEstimate::UNKNOWN);
            assert_eq!(s.check_min_diameter(0.1), SizeVerdict::Undetermined);
            assert_eq!(s.representative_m(), None);
        }
    }
}
