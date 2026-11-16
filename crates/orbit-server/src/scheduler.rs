//! 后台任务:定时刷新编目、按周期计算位置帧。
//!
//! 刷新时刻:每小时一次,固定在启动时随机选定的分钟(避开整点与半点,符合 Space-Track 规定)。
//! 是否真正联网由各数据源的缓存与刷新间隔决定(见 orbit-sources),因此即使频繁触发也不会超频。

use crate::state::{AppState, CatalogState, PositionFrame, RefreshKind, catalog_signature};
use chrono::{DateTime, Duration as ChronoDuration, Timelike, Utc};
use orbit_core::Filter;
use orbit_sources::{FetchMode, SourceSet};
use std::hash::{BuildHasher, Hasher};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::time::MissedTickBehavior;

/// 随机数(无需额外依赖):利用 `RandomState` 的随机种子。
fn random_u64() -> u64 {
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(Utc::now().timestamp_nanos_opt().unwrap_or_default() as u128);
    h.finish()
}

/// 由随机数得到刷新分钟:落在 [2, 28] ∪ [32, 58],避开整点/半点前后各 2 分钟。
pub fn pick_refresh_minute(seed: u64) -> u32 {
    let r = (seed % 54) as u32;
    if r < 27 { 2 + r } else { 32 + (r - 27) }
}

/// `now` 之后下一个"分钟 = minute、秒 = second"的时刻。
pub fn next_wake(now: DateTime<Utc>, minute: u32, second: u32) -> DateTime<Utc> {
    let hour_start = now
        .with_minute(0)
        .and_then(|t| t.with_second(0))
        .and_then(|t| t.with_nanosecond(0))
        .unwrap_or(now);
    let candidate = hour_start
        + ChronoDuration::minutes(i64::from(minute % 60))
        + ChronoDuration::seconds(i64::from(second % 60));
    if candidate > now {
        candidate
    } else {
        candidate + ChronoDuration::hours(1)
    }
}

/// 构建编目所用的数据。
struct Built {
    catalog: orbit_core::Catalog,
    report: orbit_core::BuildReport,
    provenance: Option<orbit_archive::Provenance>,
}

/// 采集(按需)并重建编目。
///
/// - `Collect` 且采集策略为 `serve`:联网获取(离线时只读缓存)并写入归档;
/// - 其余情况不联网;
/// - 历史库可用时按时刻策略从历史库选取数据,否则使用数据源的最新数据;
/// - 编目内容无变化时不替换(避免 WS 客户端重复下载元数据)。
pub async fn refresh_once(state: &AppState, sources: &SourceSet, kind: RefreshKind) {
    state.update_schedule(|s| s.refreshing = true);
    let settings = state.settings();
    let collect = kind == RefreshKind::Collect && settings.collect_mode.serve_collects();
    let archive = state.options.archive.clone();
    let mut latest: Option<orbit_sources::FetchAllResult> = None;

    if collect {
        let mode = if state.options.offline {
            FetchMode::CacheOnly
        } else {
            FetchMode::Normal
        };
        let result = sources.fetch_all(mode, Utc::now()).await;
        state.set_source_status(result.statuses.clone());
        let result = match (&archive, result.any_ok()) {
            (Some(a), true) => {
                let a = a.clone();
                let cache = sources.cache().clone();
                match tokio::task::spawn_blocking(move || {
                    let r = a.record(&result, &cache, Utc::now());
                    (result, r)
                })
                .await
                {
                    Ok((result, Ok(rep))) => {
                        if let Some(h) = &rep.history {
                            tracing::info!(
                                new_elsets = h.new_elsets,
                                snapshots = rep.snapshots_saved,
                                "已写入历史归档"
                            );
                        }
                        Some(result)
                    }
                    Ok((result, Err(e))) => {
                        tracing::error!(error = %e, "写入历史归档失败");
                        Some(result)
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "归档任务失败");
                        None
                    }
                }
            }
            (_, ok) => ok.then_some(result),
        };
        latest = result;
    }

    let now = state.now();
    let built: Option<Built> = match archive.filter(|a| a.history().is_some()) {
        Some(a) => {
            let policy = settings.time_policy;
            match tokio::task::spawn_blocking(move || a.catalog_at(policy, now)).await {
                Ok(Ok(Some((catalog, report, prov)))) => Some(Built {
                    catalog,
                    report,
                    provenance: Some(prov),
                }),
                Ok(Ok(None)) => None,
                Ok(Err(e)) => {
                    tracing::error!(error = %e, "读取历史库失败");
                    None
                }
                Err(e) => {
                    tracing::error!(error = %e, "读取历史库任务失败");
                    None
                }
            }
        }
        None => {
            // 无历史库:使用本次获取结果;未采集时读缓存
            let result = match latest {
                Some(r) => Some(r),
                None => {
                    let r = sources.fetch_all(FetchMode::CacheOnly, Utc::now()).await;
                    if state.source_status().is_empty() {
                        state.set_source_status(r.statuses.clone());
                    }
                    r.any_ok().then_some(r)
                }
            };
            match result {
                Some(r) => tokio::task::spawn_blocking(move || {
                    let (catalog, report) = r.build_catalog(now);
                    Built {
                        catalog,
                        report,
                        provenance: None,
                    }
                })
                .await
                .ok(),
                None => None,
            }
        }
    };

    match built {
        Some(b) => {
            let signature =
                catalog_signature(&b.catalog) ^ (settings.time_policy as u64).wrapping_mul(0x9E37_79B9);
            let old = state.catalog.load_full();
            if signature != old.signature || old.generation == 0 {
                let generation = old.generation + 1;
                tracing::info!(generation, objects = b.catalog.len(), policy = %settings.time_policy, "编目已更新");
                state.catalog.store(Arc::new(CatalogState {
                    catalog: b.catalog,
                    generation,
                    loaded_at: Utc::now(),
                    report: b.report,
                    provenance: b.provenance,
                    signature,
                }));
                state.catalog_gen.send_replace(generation);
            } else if b.provenance != old.provenance {
                // 内容相同但依据说明变化(如数据时刻前进):只更新说明,不打扰客户端
                state.catalog.store(Arc::new(CatalogState {
                    catalog: old.catalog.clone(),
                    generation: old.generation,
                    loaded_at: old.loaded_at,
                    report: old.report.clone(),
                    provenance: b.provenance,
                    signature,
                }));
            }
        }
        None => tracing::warn!("没有可用数据,保留现有编目"),
    }
    state.update_schedule(|s| {
        s.refreshing = false;
        s.last_refresh = Some(Utc::now());
    });
}

/// 刷新调度循环:serve 采集模式下每小时随机分钟联网采集;另每隔 `reload_minutes` 从本地重新加载。
pub async fn run_scheduler(
    state: Arc<AppState>,
    sources: Arc<SourceSet>,
    mut refresh_rx: mpsc::Receiver<RefreshKind>,
) {
    let minute = state.schedule_info().refresh_minute;
    let second = (random_u64() % 60) as u32;
    let reload = ChronoDuration::minutes(state.options.collect.reload_minutes.clamp(1, 1_440) as i64);
    let mut shutdown = state.shutdown.subscribe();
    loop {
        let now = Utc::now();
        let next_collect = state
            .settings()
            .collect_mode
            .serve_collects()
            .then(|| next_wake(now, minute, second));
        let next_reload = now + reload;
        state.update_schedule(|s| {
            s.next_refresh = next_collect;
            s.next_reload = Some(next_reload);
        });
        let (wake, kind) = match next_collect {
            Some(c) if c <= next_reload => (c, RefreshKind::Collect),
            _ => (next_reload, RefreshKind::Reload),
        };
        let sleep = (wake - Utc::now()).to_std().unwrap_or_default();
        let kind = tokio::select! {
            _ = tokio::time::sleep(sleep) => kind,
            req = refresh_rx.recv() => match req {
                Some(k) => {
                    tracing::info!(?k, "收到刷新请求");
                    k
                }
                None => break,
            },
            _ = shutdown.changed() => break,
        };
        refresh_once(&state, &sources, kind).await;
    }
}

/// 计算全部物体在 `t` 时刻的位置帧。
pub fn compute_frame(cs: &CatalogState, t: DateTime<Utc>) -> PositionFrame {
    let n = cs.catalog.len();
    let all: Vec<usize> = (0..n).collect();
    let snap = cs.catalog.snapshot(&all, &Filter::pass_all(), t);
    let mut ecef_km = vec![[f32::NAN; 3]; n];
    let mut alt_km = vec![f32::NAN; n];
    for (i, s) in snap.states {
        ecef_km[i] = s.ecef_pos_km.map(|v| v as f32);
        alt_km[i] = s.geodetic.alt_km as f32;
    }
    PositionFrame {
        generation: cs.generation,
        time: t,
        ecef_km,
        alt_km,
    }
}

/// 位置计算循环:只有存在 WS 订阅者时才计算。
pub async fn run_ticker(state: Arc<AppState>) {
    let mut interval = tokio::time::interval(state.options.tick);
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut shutdown = state.shutdown.subscribe();
    loop {
        tokio::select! {
            _ = interval.tick() => {}
            _ = shutdown.changed() => break,
        }
        if state.positions.receiver_count() == 0 {
            continue;
        }
        let cs = state.catalog.load_full();
        let t = state.now();
        match tokio::task::spawn_blocking(move || compute_frame(&cs, t)).await {
            Ok(frame) => {
                state.positions.send_replace(Some(Arc::new(frame)));
            }
            Err(e) => tracing::error!(error = %e, "位置计算任务失败"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn refresh_minute_avoids_top_and_bottom() {
        for seed in 0..1000u64 {
            let m = pick_refresh_minute(seed);
            assert!((2..=28).contains(&m) || (32..=58).contains(&m), "{m}");
        }
        let all: std::collections::BTreeSet<u32> = (0..54).map(pick_refresh_minute).collect();
        assert_eq!(all.len(), 54);
    }

    #[test]
    fn next_wake_times() {
        let t = Utc.with_ymd_and_hms(2024, 1, 1, 10, 5, 0).unwrap();
        assert_eq!(
            next_wake(t, 17, 30),
            Utc.with_ymd_and_hms(2024, 1, 1, 10, 17, 30).unwrap()
        );
        assert_eq!(
            next_wake(t, 3, 0),
            Utc.with_ymd_and_hms(2024, 1, 1, 11, 3, 0).unwrap()
        );
        // 恰好到点:取下一小时
        let t = Utc.with_ymd_and_hms(2024, 1, 1, 23, 17, 30).unwrap();
        assert_eq!(
            next_wake(t, 17, 30),
            Utc.with_ymd_and_hms(2024, 1, 2, 0, 17, 30).unwrap()
        );
    }
}
