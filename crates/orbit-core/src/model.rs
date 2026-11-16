//! 数据模型。

use crate::elements::DerivedOrbit;
use crate::size::SizeEstimate;
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;

/// NORAD 编目号。使用 `u32` 以支持 9 位编号。
pub type NoradId = u32;

/// 物体类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum ObjectType {
    #[serde(rename = "PAYLOAD", alias = "PAY")]
    Payload,
    #[serde(rename = "ROCKET BODY", alias = "R/B", alias = "ROCKET_BODY")]
    RocketBody,
    #[serde(rename = "DEBRIS", alias = "DEB")]
    Debris,
    #[serde(rename = "UNKNOWN", alias = "UNK", alias = "TBA")]
    #[default]
    Unknown,
}

impl ObjectType {
    pub const ALL: [ObjectType; 4] = [
        ObjectType::Payload,
        ObjectType::RocketBody,
        ObjectType::Debris,
        ObjectType::Unknown,
    ];

    /// 宽松解析:兼容 Space-Track(`PAYLOAD` / `ROCKET BODY` / `DEBRIS` / `UNKNOWN`)
    /// 与 CelesTrak SATCAT(`PAY` / `R/B` / `DEB` / `UNK`)写法;无法识别时为 `Unknown`。
    pub fn parse_lenient(s: &str) -> Self {
        match s.trim().to_ascii_uppercase().as_str() {
            "PAYLOAD" | "PAY" => Self::Payload,
            "ROCKET BODY" | "ROCKET_BODY" | "R/B" | "RB" => Self::RocketBody,
            "DEBRIS" | "DEB" => Self::Debris,
            _ => Self::Unknown,
        }
    }

    /// 根据名称后缀推测类型(仅在无元数据时使用,例如纯 TLE 输入)。
    pub fn guess_from_name(name: &str) -> Self {
        let n = name.to_ascii_uppercase();
        if n.contains(" DEB") || n.ends_with("DEB") {
            Self::Debris
        } else if n.contains(" R/B") || n.ends_with("R/B") {
            Self::RocketBody
        } else {
            Self::Unknown
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Payload => "PAYLOAD",
            Self::RocketBody => "ROCKET BODY",
            Self::Debris => "DEBRIS",
            Self::Unknown => "UNKNOWN",
        }
    }
}

impl fmt::Display for ObjectType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Space-Track 的 RCS 分档。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum RcsCategory {
    /// RCS < 0.1 m²
    Small,
    /// 0.1 m² ≤ RCS < 1 m²
    Medium,
    /// RCS ≥ 1 m²
    Large,
}

impl RcsCategory {
    pub fn parse_lenient(s: &str) -> Option<Self> {
        match s.trim().to_ascii_uppercase().as_str() {
            "SMALL" => Some(Self::Small),
            "MEDIUM" => Some(Self::Medium),
            "LARGE" => Some(Self::Large),
            _ => None,
        }
    }

    /// 分档对应的 RCS 区间(m²)。
    pub fn rcs_range_m2(self) -> (Option<f64>, Option<f64>) {
        match self {
            Self::Small => (None, Some(0.1)),
            Self::Medium => (Some(0.1), Some(1.0)),
            Self::Large => (Some(1.0), None),
        }
    }
}

/// 雷达截面积信息。
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RcsInfo {
    /// 数值 RCS(m²),例如来自 CelesTrak SATCAT。
    Numeric { m2: f64 },
    /// 分档 RCS,例如来自 Space-Track `RCS_SIZE`。
    Category { category: RcsCategory },
    #[default]
    Unknown,
}

impl RcsInfo {
    /// 信息量排序:Numeric > Category > Unknown。
    fn rank(&self) -> u8 {
        match self {
            Self::Numeric { m2 } if m2.is_finite() && *m2 > 0.0 => 2,
            Self::Numeric { .. } => 0,
            Self::Category { .. } => 1,
            Self::Unknown => 0,
        }
    }

    /// 返回信息量更大的那个(相同时保留 `self`)。
    pub fn better(self, other: Self) -> Self {
        if other.rank() > self.rank() { other } else { self }
    }
}

/// 数据来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataSource {
    SpaceTrack,
    CelesTrak,
    File,
}

impl fmt::Display for DataSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::SpaceTrack => "space-track",
            Self::CelesTrak => "celestrak",
            Self::File => "file",
        })
    }
}

/// 物体元数据(与轨道根数无关的部分)。
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ObjectMeta {
    pub name: Option<String>,
    /// 国际编号(COSPAR ID),如 `1998-067A`
    pub intl_designator: Option<String>,
    pub object_type: ObjectType,
    /// 国家/所有者代码(Space-Track `COUNTRY_CODE` 或 SATCAT `OWNER`)
    pub country: Option<String>,
    pub launch_date: Option<NaiveDate>,
    pub decay_date: Option<NaiveDate>,
    pub rcs: RcsInfo,
}

impl ObjectMeta {
    /// 用 `other` 补全本对象缺失的字段(已有值不覆盖;RCS 取信息量更大者)。
    pub fn fill_missing_from(&mut self, other: &ObjectMeta) {
        if self.name.is_none() {
            self.name.clone_from(&other.name);
        }
        if self.intl_designator.is_none() {
            self.intl_designator.clone_from(&other.intl_designator);
        }
        if self.object_type == ObjectType::Unknown {
            self.object_type = other.object_type;
        }
        if self.country.is_none() {
            self.country.clone_from(&other.country);
        }
        if self.launch_date.is_none() {
            self.launch_date = other.launch_date;
        }
        if self.decay_date.is_none() {
            self.decay_date = other.decay_date;
        }
        self.rcs = self.rcs.better(other.rcs);
    }
}

/// 一条 GP 记录:平根数 + 元数据 + 来源。是各数据源解析后的统一中间格式。
#[derive(Debug, Clone)]
pub struct GpRecord {
    pub elements: sgp4::Elements,
    pub meta: ObjectMeta,
    pub source: DataSource,
}

impl GpRecord {
    pub fn norad_id(&self) -> NoradId {
        // 解析阶段已保证编号在 u32 范围内
        NoradId::try_from(self.elements.norad_id).unwrap_or(NoradId::MAX)
    }

    pub fn epoch(&self) -> DateTime<Utc> {
        self.elements.datetime.and_utc()
    }
}

/// 编目中的一个空间物体(构建 [`crate::Catalog`] 后得到)。
#[derive(Debug, Clone, Serialize)]
pub struct SpaceObject {
    pub norad_id: NoradId,
    pub name: String,
    #[serde(flatten)]
    pub meta: ObjectMeta,
    pub epoch: DateTime<Utc>,
    pub derived: DerivedOrbit,
    pub size: SizeEstimate,
    pub source: DataSource,
    /// 原始平根数(OMM 字段名)
    pub elements: sgp4::Elements,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_type_lenient() {
        assert_eq!(ObjectType::parse_lenient("PAY"), ObjectType::Payload);
        assert_eq!(ObjectType::parse_lenient("rocket body"), ObjectType::RocketBody);
        assert_eq!(ObjectType::parse_lenient("R/B"), ObjectType::RocketBody);
        assert_eq!(ObjectType::parse_lenient("DEB"), ObjectType::Debris);
        assert_eq!(ObjectType::parse_lenient("???"), ObjectType::Unknown);
        assert_eq!(ObjectType::guess_from_name("FENGYUN 1C DEB"), ObjectType::Debris);
        assert_eq!(ObjectType::guess_from_name("CZ-4C R/B"), ObjectType::RocketBody);
        assert_eq!(ObjectType::guess_from_name("ISS (ZARYA)"), ObjectType::Unknown);
    }

    #[test]
    fn object_type_serde_aliases() {
        let v: Vec<ObjectType> =
            serde_json::from_str(r#"["PAYLOAD","R/B","DEB","ROCKET BODY","UNK"]"#).unwrap();
        assert_eq!(
            v,
            vec![
                ObjectType::Payload,
                ObjectType::RocketBody,
                ObjectType::Debris,
                ObjectType::RocketBody,
                ObjectType::Unknown
            ]
        );
    }

    #[test]
    fn meta_fill_missing() {
        let mut a = ObjectMeta {
            name: Some("A".into()),
            rcs: RcsInfo::Category {
                category: RcsCategory::Small,
            },
            ..Default::default()
        };
        let b = ObjectMeta {
            name: Some("B".into()),
            country: Some("PRC".into()),
            object_type: ObjectType::Debris,
            rcs: RcsInfo::Numeric { m2: 0.02 },
            ..Default::default()
        };
        a.fill_missing_from(&b);
        assert_eq!(a.name.as_deref(), Some("A"));
        assert_eq!(a.country.as_deref(), Some("PRC"));
        assert_eq!(a.object_type, ObjectType::Debris);
        assert_eq!(a.rcs, RcsInfo::Numeric { m2: 0.02 });
    }
}
