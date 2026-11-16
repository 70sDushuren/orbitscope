#![allow(clippy::unwrap_used, clippy::expect_used)]

use chrono::{DateTime, Duration, TimeZone, Utc};
use orbit_archive::{Archive, ArchiveConfig, History, HistoryConfig, SnapshotConfig, TimePolicy};
use orbit_core::{DataSource, GpRecord, parse_omm_json};
use orbit_sources::{Cache, CacheMeta, FetchAllResult, Origin, ResourceStatus, SourceStatus};

fn iss() -> GpRecord {
    let text = std::fs::read_to_string(format!(
        "{}/../../testdata/spacetrack_gp.json",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    let out = parse_omm_json(&text, DataSource::SpaceTrack).unwrap();
    out.records.into_iter().find(|r| r.norad_id() == 25544).unwrap()
}

/// 构造历元偏移 `epoch_h` 小时、获取时间为 `seen` 的 ISS 根数。
fn iss_at(epoch_h: i64, seen: DateTime<Utc>) -> GpRecord {
    let mut r = iss();
    r.elements.datetime += Duration::hours(epoch_h);
    r.fetched_at = Some(seen);
    r
}

fn t(h: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2024, 1, 2, h, 0, 0).unwrap()
}

fn epoch_of(r: &GpRecord) -> DateTime<Utc> {
    r.epoch()
}

#[test]
fn history_is_append_only_and_deterministic() {
    let dir = tempfile::tempdir().unwrap();
    let h = History::open(dir.path().join("h.sqlite"), HistoryConfig::default()).unwrap();
    // 三次获取:t(1) 得到历元 +0h,t(5) 得到 +4h,t(9) 得到 +8h
    let a = iss_at(0, t(1));
    let b = iss_at(4, t(5));
    let c = iss_at(8, t(9));
    assert_eq!(
        h.ingest(std::slice::from_ref(&a), &[], t(1)).unwrap().new_elsets,
        1
    );
    assert_eq!(
        h.ingest(&[a.clone(), b.clone()], &[], t(5)).unwrap().new_elsets,
        1,
        "已存在的根数不重复写入"
    );
    h.ingest(&[b.clone(), c.clone()], &[], t(9)).unwrap();

    // 同一根数之后再次出现,首次获取时间不变
    let mut b_again = b.clone();
    b_again.fetched_at = Some(t(20));
    assert_eq!(h.ingest(&[b_again], &[], t(20)).unwrap().new_elsets, 0);
    let hist = h.object_history(25544, 10).unwrap();
    assert_eq!(hist.len(), 3);
    assert_eq!(hist[1].first_seen, t(5), "first_seen 保持首次获取的时间");

    // known-at:只用 T 时刻已获取到的数据
    let (r, p) = h.select(TimePolicy::KnownAt, t(6)).unwrap();
    assert_eq!(r.len(), 1);
    assert_eq!(epoch_of(&r[0]), epoch_of(&b));
    assert_eq!(p.first_seen_max, Some(t(5)));
    assert_eq!(p.records, 1);
    let (r, _) = h.select(TimePolicy::KnownAt, t(0)).unwrap();
    assert!(r.is_empty(), "T 之前没有获取过数据");

    // 重复查询结果完全相同
    let first = serde_json::to_string(&h.select(TimePolicy::KnownAt, t(6)).unwrap().0).unwrap();
    let second = serde_json::to_string(&h.select(TimePolicy::KnownAt, t(6)).unwrap().0).unwrap();
    assert_eq!(first, second);

    // nearest-epoch:历元最接近 T(即使是之后才获取的)
    let target = epoch_of(&c) - Duration::minutes(30);
    let (r, _) = h.select(TimePolicy::NearestEpoch, target).unwrap();
    assert_eq!(epoch_of(&r[0]), epoch_of(&c));

    // latest:历元最新
    let (r, _) = h.select(TimePolicy::Latest, t(0)).unwrap();
    assert_eq!(epoch_of(&r[0]), epoch_of(&c));
}

#[test]
fn source_priority_on_equal_epoch() {
    let dir = tempfile::tempdir().unwrap();
    let h = History::open(dir.path().join("h.sqlite"), HistoryConfig::default()).unwrap();
    let mut ct = iss_at(0, t(1));
    ct.source = DataSource::CelesTrak;
    ct.meta.country = Some("FROM-CELESTRAK".into());
    let st = iss_at(0, t(1));
    h.ingest(&[ct, st], &[], t(1)).unwrap();
    let (r, _) = h.select(TimePolicy::KnownAt, t(2)).unwrap();
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].source, DataSource::SpaceTrack);
}

#[test]
fn retention_keeps_newest_per_object() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = HistoryConfig {
        enabled: true,
        retain_days: 1,
        max_total_mb: 0,
    };
    let h = History::open(dir.path().join("h.sqlite"), cfg).unwrap();
    let now = t(12) + Duration::days(10);
    // 10 天前的 3 条 + 昨天的 1 条;另一个物体只有 10 天前的 1 条
    let mut recs: Vec<GpRecord> = (0..3).map(|k| iss_at(k, t(1))).collect();
    recs.push(iss_at(200, now - Duration::hours(2)));
    let mut other = iss_at(0, t(1));
    other.elements.norad_id = 90_001;
    recs.push(other);
    h.ingest(&recs, &[], t(1)).unwrap();
    assert_eq!(h.usage().unwrap().elsets, 5);

    let deleted = h.prune(now).unwrap();
    assert_eq!(deleted, 3, "删除过期的旧根数");
    let u = h.usage().unwrap();
    assert_eq!(u.elsets, 2);
    assert_eq!(u.objects, 2, "每个物体至少保留最新一条");
}

#[test]
fn size_limit_prunes_oldest_but_keeps_newest_per_object() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = HistoryConfig {
        enabled: true,
        retain_days: 0,
        max_total_mb: 1,
    };
    let h = History::open(dir.path().join("h.sqlite"), cfg).unwrap();
    let recs: Vec<GpRecord> = (0..4000)
        .map(|k| iss_at(k, t(0) + Duration::minutes(k)))
        .collect();
    h.ingest(&recs, &[], t(0)).unwrap();
    assert!(h.usage().unwrap().bytes > 1024 * 1024, "测试数据应超过 1 MB");
    h.prune(t(0) + Duration::days(5)).unwrap();
    let u = h.usage().unwrap();
    assert!(u.bytes <= 1024 * 1024, "清理后不超过上限: {}", u.bytes);
    assert_eq!(u.objects, 1);
    // 最新的一条仍在
    let (r, _) = h.select(TimePolicy::Latest, t(0)).unwrap();
    assert_eq!(epoch_of(&r[0]), epoch_of(&recs[3999]));
}

fn status(label: &str, key: &str, origin: Origin, at: DateTime<Utc>) -> SourceStatus {
    SourceStatus {
        label: label.into(),
        source: DataSource::CelesTrak,
        ok: true,
        records: 1,
        record_errors: 0,
        extra_meta: 0,
        resources: vec![ResourceStatus {
            key: key.into(),
            url: "https://example.org/x".into(),
            origin,
            fetched_at: at,
            bytes: 5,
        }],
        notes: vec![],
        error: None,
        error_code: None,
        error_detail: None,
    }
}

#[test]
fn archive_records_snapshots_history_and_fetches() {
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::new(dir.path().join("cache"));
    let cfg = ArchiveConfig {
        dir: Some(dir.path().join("archive")),
        snapshots: SnapshotConfig {
            enabled: true,
            max_files: 2,
            max_total_mb: 0,
        },
        ..Default::default()
    };
    let archive = Archive::open(&cfg).unwrap();

    let rt = tokio_like_block_on(async {
        for (h, body) in [(1u32, "v1"), (2, "v2"), (3, "v3")] {
            let meta = CacheMeta {
                url: "https://example.org/x".into(),
                fetched_at: t(h),
                etag: None,
                last_modified: None,
                bytes: 2,
            };
            cache.write("celestrak/gp-active", &meta, body).await.unwrap();
            let result = FetchAllResult {
                records: vec![iss_at(i64::from(h), t(h))],
                extra_meta: Default::default(),
                statuses: vec![status("celestrak", "celestrak/gp-active", Origin::Network, t(h))],
            };
            archive.record(&result, &cache, t(h)).unwrap();
        }
        // 来自缓存的获取:不保存快照、不记采集记录,但根数仍写入(幂等)
        let result = FetchAllResult {
            records: vec![iss_at(3, t(3))],
            extra_meta: Default::default(),
            statuses: vec![status(
                "celestrak",
                "celestrak/gp-active",
                Origin::Cache { age_secs: 60 },
                t(3),
            )],
        };
        archive.record(&result, &cache, t(4)).unwrap()
    });
    assert_eq!(rt.snapshots_saved, 0);
    assert_eq!(rt.history.as_ref().unwrap().new_elsets, 0);

    let files = archive.snapshots().list().unwrap();
    assert_eq!(files.len(), 2, "快照数量上限 2");
    assert_eq!(archive.snapshots().read(&files[1]).unwrap(), "v3");
    let fetches = archive.history().unwrap().recent_fetches(10).unwrap();
    assert_eq!(fetches.len(), 3);
    assert_eq!(fetches[0].fetched_at, t(3));
    assert_eq!(fetches[0].origin, "network");

    let (cat, _, prov) = archive
        .catalog_at(TimePolicy::KnownAt, t(2) + Duration::minutes(30))
        .unwrap()
        .unwrap();
    assert_eq!(cat.len(), 1);
    assert_eq!(prov.first_seen_max, Some(t(2)));
    let u = archive.usage().unwrap();
    assert_eq!(u.history.unwrap().elsets, 3);
    assert_eq!(u.snapshots.files, 2);
}

#[test]
fn history_disabled() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = ArchiveConfig {
        dir: Some(dir.path().to_path_buf()),
        ..Default::default()
    };
    cfg.history.enabled = false;
    let a = Archive::open(&cfg).unwrap();
    assert!(a.history().is_none());
    assert!(a.catalog_at(TimePolicy::KnownAt, t(1)).unwrap().is_none());
    assert!(!dir.path().join("history.sqlite").exists());
}

/// 测试中运行少量异步缓存写入(避免为此引入完整运行时宏)。
fn tokio_like_block_on<F: std::future::Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(f)
}
