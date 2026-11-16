#![allow(clippy::unwrap_used, clippy::expect_used)]
//! 基准:3 万个合成 LEO 物体的批量外推 + 坐标转换(对应 NFR-01)。

use chrono::{Duration, NaiveDate};
use criterion::{Criterion, criterion_group, criterion_main};
use orbit_core::{Catalog, DataSource, Filter, GpRecord, ObjectMeta};
use std::hint::black_box;

fn synthetic_records(n: usize) -> Vec<GpRecord> {
    let epoch = NaiveDate::from_ymd_opt(2024, 1, 1)
        .unwrap()
        .and_hms_opt(0, 0, 0)
        .unwrap();
    (0..n)
        .map(|i| {
            let f = i as f64 / n as f64;
            let elements = sgp4::Elements {
                object_name: Some(format!("SYN {i}")),
                international_designator: None,
                norad_id: 100_000 + i as u64,
                classification: sgp4::Classification::Unclassified,
                datetime: epoch + Duration::minutes((i % 1440) as i64),
                mean_motion_dot: 0.0,
                mean_motion_ddot: 0.0,
                drag_term: 1e-4,
                element_set_number: 999,
                inclination: 20.0 + 80.0 * f,
                right_ascension: (i as f64 * 7.3) % 360.0,
                eccentricity: 0.0005 + 0.02 * ((i * 13 % 97) as f64 / 97.0),
                argument_of_perigee: (i as f64 * 3.1) % 360.0,
                mean_anomaly: (i as f64 * 11.7) % 360.0,
                // 高度约 300–1400 km
                mean_motion: 12.6 + 3.4 * ((i * 31 % 1000) as f64 / 1000.0),
                revolution_number: 0,
                ephemeris_type: 0,
            };
            GpRecord {
                elements,
                meta: ObjectMeta::default(),
                source: DataSource::File,
                fetched_at: None,
            }
        })
        .collect()
}

fn bench(c: &mut Criterion) {
    let now = NaiveDate::from_ymd_opt(2024, 1, 2)
        .unwrap()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc();
    let (cat, _) = Catalog::build(synthetic_records(30_000), now);
    let all: Vec<usize> = (0..cat.len()).collect();
    let f = Filter::pass_all();

    c.bench_function("snapshot_30k_parallel", |b| {
        b.iter(|| black_box(cat.snapshot(black_box(&all), &f, now)));
    });

    c.bench_function("propagate_30k_single_thread", |b| {
        b.iter(|| {
            for &i in &all {
                let _ = black_box(cat.propagate(i, now));
            }
        });
    });

    c.bench_function("select_default_filter_30k", |b| {
        let f = Filter::default();
        b.iter(|| black_box(cat.select(&f, now)));
    });

    c.bench_function("build_catalog_30k", |b| {
        b.iter_batched(
            || synthetic_records(30_000),
            |recs| black_box(Catalog::build(recs, now)),
            criterion::BatchSize::LargeInput,
        );
    });
}

criterion_group!(benches, bench);
criterion_main!(benches);
