//! 编目快照:去重合并、预初始化 SGP4、批量过滤与外推。
//!
//! [`Catalog`] 构建后不可变;数据刷新时构建新快照整体替换(服务端用 ArcSwap)。

use crate::elements::DerivedOrbit;
use crate::error::CoreError;
use crate::filter::Filter;
use crate::frames::Geodetic;
use crate::model::{GpRecord, NoradId, ObjectMeta, SpaceObject};
use crate::propagate::{StateVector, propagate_with_gmst};
use crate::size::SizeEstimate;
use crate::time::gmst_rad;
use chrono::{DateTime, Duration, Utc};
use rayon::prelude::*;
use serde::Serialize;
use std::collections::HashMap;

/// 单条轨迹最多点数,防止误用产生超大数组。
pub const MAX_TRACK_POINTS: usize = 20_000;

/// 构建报告。
#[derive(Debug, Clone, Default, Serialize)]
pub struct BuildReport {
    /// 输入记录数
    pub input: usize,
    /// 收录物体数
    pub accepted: usize,
    /// 同一编号的重复记录数(保留 epoch 最新者)
    pub duplicates: usize,
    /// 已再入而剔除的数量
    pub decayed: usize,
    /// 根数非法或 SGP4 初始化失败而剔除的物体
    pub rejected: Vec<(NoradId, String)>,
}

/// 编目快照。
#[derive(Debug, Clone)]
pub struct Catalog {
    objects: Vec<SpaceObject>,
    constants: Vec<sgp4::Constants>,
    index: HashMap<NoradId, usize>,
    built_at: DateTime<Utc>,
}

/// 一次批量外推的结果。
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub time: DateTime<Utc>,
    /// (物体下标, 状态),按下标升序
    pub states: Vec<(usize, StateVector)>,
    /// 外推失败的物体(通常是已衰减)
    pub failed: Vec<(usize, CoreError)>,
    /// `instantaneous` 模式下因当前高度不在范围内而被排除的数量
    pub excluded_by_altitude: usize,
}

/// 轨迹点。
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct TrackPoint {
    pub time: DateTime<Utc>,
    pub ecef_pos_km: [f64; 3],
    pub geodetic: Geodetic,
}

/// 用补充元数据(如 CelesTrak SATCAT)补全记录中缺失的字段。
pub fn apply_meta(records: &mut [GpRecord], extra: &HashMap<NoradId, ObjectMeta>) {
    for r in records {
        if let Some(m) = extra.get(&r.norad_id()) {
            r.meta.fill_missing_from(m);
        }
    }
}

impl Catalog {
    /// 由 GP 记录构建快照。
    ///
    /// - 同一 NORAD 编号保留 epoch 最新的根数,元数据互相补全;
    /// - 剔除 `decay_date <= now` 的物体;
    /// - 剔除根数非法或 SGP4 初始化失败的物体(记入报告)。
    pub fn build(records: Vec<GpRecord>, now: DateTime<Utc>) -> (Self, BuildReport) {
        let mut report = BuildReport {
            input: records.len(),
            ..Default::default()
        };

        let mut by_id: HashMap<NoradId, GpRecord> = HashMap::with_capacity(records.len());
        for rec in records {
            let id = rec.norad_id();
            match by_id.get_mut(&id) {
                None => {
                    by_id.insert(id, rec);
                }
                Some(existing) => {
                    report.duplicates += 1;
                    if rec.elements.datetime > existing.elements.datetime {
                        let mut rec = rec;
                        rec.meta.fill_missing_from(&existing.meta);
                        *existing = rec;
                    } else {
                        existing.meta.fill_missing_from(&rec.meta);
                    }
                }
            }
        }

        let today = now.date_naive();
        let mut recs: Vec<GpRecord> = by_id
            .into_values()
            .filter(|r| {
                let decayed = r.meta.decay_date.is_some_and(|d| d <= today);
                report.decayed += usize::from(decayed);
                !decayed
            })
            .collect();
        recs.sort_unstable_by_key(GpRecord::norad_id);

        let built: Vec<Result<(SpaceObject, sgp4::Constants), (NoradId, String)>> =
            recs.into_par_iter().map(build_object).collect();

        let mut objects = Vec::with_capacity(built.len());
        let mut constants = Vec::with_capacity(built.len());
        for b in built {
            match b {
                Ok((o, c)) => {
                    objects.push(o);
                    constants.push(c);
                }
                Err(e) => report.rejected.push(e),
            }
        }
        let index = objects.iter().enumerate().map(|(i, o)| (o.norad_id, i)).collect();
        report.accepted = objects.len();
        (
            Self {
                objects,
                constants,
                index,
                built_at: now,
            },
            report,
        )
    }

    pub fn len(&self) -> usize {
        self.objects.len()
    }

    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }

    pub fn built_at(&self) -> DateTime<Utc> {
        self.built_at
    }

    pub fn objects(&self) -> &[SpaceObject] {
        &self.objects
    }

    pub fn get(&self, idx: usize) -> Option<&SpaceObject> {
        self.objects.get(idx)
    }

    /// 按 NORAD 编号查下标。
    pub fn index_of(&self, norad_id: NoradId) -> Option<usize> {
        self.index.get(&norad_id).copied()
    }

    pub fn by_norad(&self, norad_id: NoradId) -> Option<&SpaceObject> {
        self.index_of(norad_id).map(|i| &self.objects[i])
    }

    /// 静态过滤,返回升序下标。`now` 用于根数龄期判断。
    pub fn select(&self, filter: &Filter, now: DateTime<Utc>) -> Vec<usize> {
        self.objects
            .par_iter()
            .enumerate()
            .filter(|(_, o)| filter.matches_static(o, now))
            .map(|(i, _)| i)
            .collect()
    }

    /// 外推单个物体。
    pub fn propagate(&self, idx: usize, t: DateTime<Utc>) -> Result<StateVector, CoreError> {
        self.propagate_gmst(idx, t, gmst_rad(t))
    }

    fn propagate_gmst(&self, idx: usize, t: DateTime<Utc>, gmst: f64) -> Result<StateVector, CoreError> {
        let o = self.objects.get(idx).ok_or(CoreError::IndexOutOfRange(idx))?;
        propagate_with_gmst(&self.constants[idx], o.norad_id, o.epoch, t, gmst)
    }

    /// 对给定下标集合并行外推,并按 `filter` 的瞬时高度条件筛选。
    pub fn snapshot(&self, indices: &[usize], filter: &Filter, t: DateTime<Utc>) -> Snapshot {
        let gmst = gmst_rad(t);
        let results: Vec<(usize, Result<StateVector, CoreError>)> = indices
            .par_iter()
            .map(|&i| (i, self.propagate_gmst(i, t, gmst)))
            .collect();

        let mut snap = Snapshot {
            time: t,
            states: Vec::with_capacity(results.len()),
            failed: Vec::new(),
            excluded_by_altitude: 0,
        };
        for (i, r) in results {
            match r {
                Ok(s) if filter.matches_instantaneous(s.geodetic.alt_km) => snap.states.push((i, s)),
                Ok(_) => snap.excluded_by_altitude += 1,
                Err(e) => snap.failed.push((i, e)),
            }
        }
        snap
    }

    /// `select` + `snapshot` 的便捷组合(龄期以 `t` 计)。
    pub fn snapshot_filtered(&self, filter: &Filter, t: DateTime<Utc>) -> Snapshot {
        let idx = self.select(filter, t);
        self.snapshot(&idx, filter, t)
    }

    /// 计算 `[start, end]` 区间、步长 `step` 的轨迹。外推失败的点被跳过;若全部失败则返回首个错误。
    pub fn track(
        &self,
        idx: usize,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        step: Duration,
    ) -> Result<Vec<TrackPoint>, CoreError> {
        if idx >= self.objects.len() {
            return Err(CoreError::IndexOutOfRange(idx));
        }
        let step_ms = step.num_milliseconds();
        let span_ms = (end - start).num_milliseconds();
        if step_ms <= 0 || span_ms < 0 {
            return Err(CoreError::InvalidArgument("step 必须为正且 end ≥ start".into()));
        }
        let n = usize::try_from(span_ms / step_ms)
            .unwrap_or(usize::MAX)
            .saturating_add(1);
        if n > MAX_TRACK_POINTS {
            return Err(CoreError::InvalidArgument(format!(
                "轨迹点数 {n} 超过上限 {MAX_TRACK_POINTS}"
            )));
        }

        let mut points = Vec::with_capacity(n);
        let mut first_err = None;
        for k in 0..n {
            let t = start + Duration::milliseconds(step_ms * k as i64);
            match self.propagate(idx, t) {
                Ok(s) => points.push(TrackPoint {
                    time: t,
                    ecef_pos_km: s.ecef_pos_km,
                    geodetic: s.geodetic,
                }),
                Err(e) => {
                    first_err.get_or_insert(e);
                }
            }
        }
        match (points.is_empty(), first_err) {
            (true, Some(e)) => Err(e),
            _ => Ok(points),
        }
    }
}

fn build_object(rec: GpRecord) -> Result<(SpaceObject, sgp4::Constants), (NoradId, String)> {
    let id = rec.norad_id();
    let derived = DerivedOrbit::from_elements(&rec.elements);
    if !derived.is_valid() {
        return Err((
            id,
            format!(
                "根数非法: n={} e={}",
                derived.mean_motion_rev_day, derived.eccentricity
            ),
        ));
    }
    let constants =
        sgp4::Constants::from_elements(&rec.elements).map_err(|e| (id, format!("SGP4 初始化失败: {e}")))?;
    let name = rec
        .meta
        .name
        .clone()
        .or_else(|| rec.elements.object_name.clone())
        .unwrap_or_else(|| format!("NORAD {id}"));
    let obj = SpaceObject {
        norad_id: id,
        name,
        size: SizeEstimate::from_rcs(&rec.meta.rcs),
        epoch: rec.epoch(),
        derived,
        source: rec.source,
        meta: rec.meta,
        elements: rec.elements,
    };
    Ok((obj, constants))
}
