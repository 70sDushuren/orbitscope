//! # orbit-core
//!
//! OrbitScope 的核心计算库,**不做任何 IO**(网络、磁盘、async 均不在此 crate)。
//!
//! 数据流:
//!
//! ```text
//! 文本 (OMM JSON / TLE) ──omm/tle──▶ GpRecord ──Catalog::build──▶ Catalog
//!                                                                 │
//!                         Filter ──Catalog::select──▶ 下标集合 ───┤
//!                                                                 ▼
//!                                   Catalog::snapshot / propagate / track
//!                                      (SGP4 → TEME → ECEF → WGS84)
//! ```
//!
//! 坐标系约定:SGP4 输出 TEME;经 GMST 旋转得到 ECEF(PEF,忽略极移);再转换为 WGS84 大地坐标。

pub mod catalog;
pub mod elements;
pub mod error;
pub mod filter;
pub mod frames;
pub mod model;
pub mod omm;
pub mod propagate;
pub mod size;
pub mod stats;
pub mod summary;
pub mod time;
pub mod tle;

pub use catalog::{BuildReport, Catalog, Snapshot, TrackPoint, apply_meta};
pub use elements::DerivedOrbit;
pub use error::{CoreError, FilterIssue, RecordError};
pub use filter::{AltitudeMode, Filter, UnknownSizePolicy};
pub use frames::Geodetic;
pub use model::{DataSource, GpRecord, NoradId, ObjectMeta, ObjectType, RcsCategory, RcsInfo, SpaceObject};
pub use omm::parse_omm_json;
pub use propagate::StateVector;
pub use size::{SizeEstimate, SizeSource, SizeVerdict};
pub use stats::Stats;
pub use summary::ObjectSummary;
pub use tle::parse_tle_text;

/// 解析结果:成功的记录 + 逐条错误(单条失败不影响整体)。
#[derive(Debug, Default)]
pub struct ParseOutcome {
    pub records: Vec<GpRecord>,
    pub errors: Vec<RecordError>,
}
