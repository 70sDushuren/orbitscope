//! 后台任务:定时刷新编目、按周期计算位置帧。
//!
//! 刷新时刻:每小时一次,固定在启动时随机选定的分钟(避开整点与半点,符合 Space-Track 规定)。
//! 是否真正联网由各数据源的缓存与刷新间隔决定(见 orbit-sources),因此即使频繁触发也不会超频。

use crate::state::{AppState, CatalogState, PositionFrame};
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

/// 获取数据源并替换编目。全部数据源失败时保留旧编目。
pub async fn refresh_once(state: &AppState, sources: &SourceSet, mode: FetchMode) {
    state.update_schedule(|s| s.refreshing = true);
    let result = sources.fetch_all(mode, Utc::now()).await;
    let ok = result.any_ok();
    state.set_source_status(result.statuses.clone());
    if ok {
        let now = state.now();
        let built = tokio::task::spawn_blocking(move || result.build_catalog(now)).await;
        match built {
            Ok((catalog, report)) => {
                let generation = state.catalog.load().generation + 1;
                tracing::info!(generation, objects = catalog.len(), "编目已更新");
                state.catalog.store(Arc::new(CatalogState {
                    catalog,
                    generation,
                    loaded_at: Utc::now(),
                    report,
                }));
                state.catalog_gen.send_replace(generation);
            }
            Err(e) => tracing::error!(error = %e, "构建编目任务失败"),
        }
    } else {
        tracing::warn!("所有数据源均失败,保留现有编目");
    }
    state.update_schedule(|s| {
        s.refreshing = false;
        s.last_refresh = Some(Utc::now());
    });
}

/// 刷新调度循环。
pub async fn run_scheduler(
    state: Arc<AppState>,
    sources: Arc<SourceSet>,
    mut refresh_rx: mpsc::Receiver<()>,
) {
    let minute = state.schedule_info().refresh_minute;
    let second = (random_u64() % 60) as u32;
    let mut shutdown = state.shutdown.subscribe();
    loop {
        let wake = next_wake(Utc::now(), minute, second);
        state.update_schedule(|s| s.next_refresh = Some(wake));
        let sleep = (wake - Utc::now()).to_std().unwrap_or_default();
        tokio::select! {
            _ = tokio::time::sleep(sleep) => {}
            req = refresh_rx.recv() => {
                if req.is_none() { break; }
                tracing::info!("收到手动刷新请求");
            }
            _ = shutdown.changed() => break,
        }
        refresh_once(&state, &sources, FetchMode::Normal).await;
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
