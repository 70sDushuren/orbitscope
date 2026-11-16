#![allow(clippy::unwrap_used, clippy::expect_used)]
//! 基于 testdata/ 的端到端测试:解析 → 构建编目 → 过滤 → 外推。

use chrono::{DateTime, Duration, TimeZone, Utc};
use orbit_core::{
    AltitudeMode, Catalog, DataSource, Filter, ObjectMeta, ObjectType, RcsInfo, SizeSource, Stats,
    UnknownSizePolicy, apply_meta, parse_omm_json, parse_tle_text,
};
use std::collections::HashMap;

fn testdata(name: &str) -> String {
    let path = format!("{}/../../testdata/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("读取 {path}: {e}"))
}

fn t0() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2024, 1, 1, 12, 0, 0).unwrap()
}

fn full_catalog() -> Catalog {
    let mut recs = parse_omm_json(&testdata("spacetrack_gp.json"), DataSource::SpaceTrack)
        .unwrap()
        .records;
    recs.extend(parse_tle_text(&testdata("sample.3le"), DataSource::File).records);
    recs.extend(
        parse_omm_json(&testdata("celestrak_gp.json"), DataSource::CelesTrak)
            .unwrap()
            .records,
    );
    Catalog::build(recs, t0()).0
}

#[test]
fn parse_celestrak_json() {
    let out = parse_omm_json(&testdata("celestrak_gp.json"), DataSource::CelesTrak).unwrap();
    assert_eq!(out.records.len(), 2);
    assert_eq!(out.errors.len(), 1, "缺 EPOCH 的记录应报错");
    assert_eq!(out.errors[0].norad_id, Some(90009));
    // 9 位编号
    assert!(out.records.iter().any(|r| r.norad_id() == 270_000_123));
}

#[test]
fn parse_spacetrack_json() {
    let out = parse_omm_json(&testdata("spacetrack_gp.json"), DataSource::SpaceTrack).unwrap();
    assert!(out.errors.is_empty(), "{:?}", out.errors);
    assert_eq!(out.records.len(), 3);
    let deb = out.records.iter().find(|r| r.norad_id() == 90001).unwrap();
    assert_eq!(deb.meta.object_type, ObjectType::Debris);
    assert_eq!(deb.meta.country.as_deref(), Some("PRC"));
    assert!(out.records.iter().any(|r| r.meta.decay_date.is_some()));
}

#[test]
fn tle_with_errors() {
    let out = parse_tle_text(&testdata("sample_with_errors.3le"), DataSource::File);
    assert_eq!(out.records.len(), 3);
    assert_eq!(out.errors.len(), 1);
    assert_eq!(out.errors[0].norad_id, Some(90003));
    let rb = out.records.iter().find(|r| r.norad_id() == 90002).unwrap();
    assert_eq!(
        rb.meta.object_type,
        ObjectType::RocketBody,
        "名称后缀 R/B 推断类型"
    );
}

/// 同一 ISS 根数的 TLE 与 OMM 两种输入应得到完全相同的位置。
#[test]
fn omm_and_tle_agree() {
    let omm = parse_omm_json(&testdata("celestrak_gp.json"), DataSource::CelesTrak)
        .unwrap()
        .records;
    let tle = parse_tle_text(&testdata("sample.3le"), DataSource::File).records;
    let iss_omm = omm.into_iter().find(|r| r.norad_id() == 25544).unwrap();
    let iss_tle = tle.into_iter().find(|r| r.norad_id() == 25544).unwrap();

    let (a, _) = Catalog::build(vec![iss_omm], t0());
    let (b, _) = Catalog::build(vec![iss_tle], t0());
    let t = t0() + Duration::minutes(45);
    let sa = a.propagate(0, t).unwrap();
    let sb = b.propagate(0, t).unwrap();
    for i in 0..3 {
        assert!(
            (sa.teme_pos_km[i] - sb.teme_pos_km[i]).abs() < 1e-3,
            "{sa:?} vs {sb:?}"
        );
    }
}

#[test]
fn build_dedup_and_decay() {
    let mut recs = parse_omm_json(&testdata("spacetrack_gp.json"), DataSource::SpaceTrack)
        .unwrap()
        .records;
    let input = recs.len();
    recs.extend(parse_tle_text(&testdata("sample.3le"), DataSource::File).records);
    let (cat, report) = Catalog::build(recs, t0());

    assert_eq!(report.input, input + 3);
    assert_eq!(report.decayed, 1);
    // ISS(25544)与 90001 在两份数据中都出现
    assert_eq!(report.duplicates, 2);
    assert!(report.rejected.is_empty(), "{:?}", report.rejected);
    assert_eq!(cat.len(), 3); // 25544, 90001, 90002
    assert!(cat.by_norad(90004).is_none(), "已衰减物体应被剔除");

    // 90001:TLE epoch 06:00 与 OMM epoch 06:00 相同 → 保留先到者(Space-Track),元数据完整
    let deb = cat.by_norad(90001).unwrap();
    assert_eq!(deb.meta.country.as_deref(), Some("PRC"));
    assert_eq!(deb.size.source, SizeSource::RcsCategory);

    // ISS:元数据来自 Space-Track
    let iss = cat.by_norad(25544).unwrap();
    assert_eq!(iss.meta.object_type, ObjectType::Payload);
    assert_eq!(iss.name, "ISS (ZARYA)");
    // 索引按编号升序
    let ids: Vec<u32> = cat.objects().iter().map(|o| o.norad_id).collect();
    assert!(ids.windows(2).all(|w| w[0] < w[1]));
}

#[test]
fn newer_epoch_wins_and_keeps_meta() {
    let mut st = parse_omm_json(&testdata("spacetrack_gp.json"), DataSource::SpaceTrack)
        .unwrap()
        .records;
    st.retain(|r| r.norad_id() == 25544);
    let mut newer = st[0].clone();
    newer.elements.datetime += Duration::hours(3);
    newer.meta = ObjectMeta::default();
    newer.source = DataSource::CelesTrak;
    st.push(newer);
    let (cat, _) = Catalog::build(st, t0());
    let iss = cat.by_norad(25544).unwrap();
    assert_eq!(iss.source, DataSource::CelesTrak);
    assert_eq!(iss.epoch, t0() + Duration::hours(3));
    assert_eq!(
        iss.meta.object_type,
        ObjectType::Payload,
        "旧记录的元数据应补全到新记录"
    );
}

#[test]
fn iss_position_is_plausible() {
    let cat = full_catalog();
    let idx = cat.index_of(25544).unwrap();
    for minutes in [0, 30, 60, 93, 600] {
        let s = cat.propagate(idx, t0() + Duration::minutes(minutes)).unwrap();
        assert!(
            (390.0..450.0).contains(&s.geodetic.alt_km),
            "alt {}",
            s.geodetic.alt_km
        );
        assert!(s.geodetic.lat_deg.abs() <= 52.0, "lat {}", s.geodetic.lat_deg);
        assert!((7.6..7.75).contains(&s.speed_kms), "speed {}", s.speed_kms);
    }
}

#[test]
fn default_filter_selection() {
    let cat = full_catalog();
    let sel: Vec<u32> = cat
        .select(&Filter::default(), t0())
        .iter()
        .map(|&i| cat.objects()[i].norad_id)
        .collect();
    // 默认 100–1000 km、≥10 cm、未知尺寸包含:全部 LEO 样本都应入选
    for id in [25544, 90001, 90002, 270_000_123] {
        assert!(sel.contains(&id), "{id} 应入选: {sel:?}");
    }
}

#[test]
fn altitude_modes() {
    let cat = full_catalog();
    let id_of = |sel: Vec<usize>| -> Vec<u32> { sel.iter().map(|&i| cat.objects()[i].norad_id).collect() };

    // 90002 偏心率 0.02:近地点约 923 km,远地点约 1221 km,跨越 1000 km 上限
    let rb = cat.by_norad(90002).unwrap();
    assert!(
        rb.derived.perigee_alt_km < 1000.0 && rb.derived.apogee_alt_km > 1000.0,
        "{:?}",
        rb.derived
    );

    let overlap = Filter {
        alt_min_km: 900.0,
        alt_max_km: 1000.0,
        ..Filter::pass_all()
    };
    assert!(id_of(cat.select(&overlap, t0())).contains(&90002));

    let within = Filter {
        altitude_mode: AltitudeMode::OrbitWithin,
        ..overlap.clone()
    };
    assert!(!id_of(cat.select(&within, t0())).contains(&90002));

    let iss_shell = Filter {
        alt_min_km: 400.0,
        alt_max_km: 430.0,
        ..Filter::pass_all()
    };
    assert_eq!(id_of(cat.select(&iss_shell, t0())), vec![25544]);
}

#[test]
fn instantaneous_mode() {
    let cat = full_catalog();
    let idx = cat.index_of(90002).unwrap();
    let alt_now = cat.propagate(idx, t0()).unwrap().geodetic.alt_km;

    // 围绕当前高度 ±5 km:应入选
    let f = Filter {
        altitude_mode: AltitudeMode::Instantaneous,
        alt_min_km: alt_now - 5.0,
        alt_max_km: alt_now + 5.0,
        ..Filter::pass_all()
    };
    let snap = cat.snapshot_filtered(&f, t0());
    assert!(snap.states.iter().any(|(i, _)| *i == idx));
    for (_, s) in &snap.states {
        assert!((f.alt_min_km..=f.alt_max_km).contains(&s.geodetic.alt_km));
    }

    // 当前高度之外的窄区间:不应入选
    let f2 = Filter {
        alt_min_km: alt_now + 20.0,
        alt_max_km: alt_now + 25.0,
        ..f
    };
    let snap = cat.snapshot_filtered(&f2, t0());
    assert!(!snap.states.iter().any(|(i, _)| *i == idx));
}

#[test]
fn size_filter_and_unknown_policy() {
    let mut recs = parse_omm_json(&testdata("spacetrack_gp.json"), DataSource::SpaceTrack)
        .unwrap()
        .records;
    recs.extend(parse_tle_text(&testdata("sample.3le"), DataSource::File).records);
    // SATCAT 风格补充数据:90002 给数值 RCS 0.5 m²(d ≈ 0.80 m)
    let extra = HashMap::from([(
        90002,
        ObjectMeta {
            rcs: RcsInfo::Numeric { m2: 0.5 },
            country: Some("US".into()),
            ..Default::default()
        },
    )]);
    apply_meta(&mut recs, &extra);
    let (cat, _) = Catalog::build(recs, t0());
    let ids = |f: &Filter| -> Vec<u32> {
        cat.select(f, t0())
            .iter()
            .map(|&i| cat.objects()[i].norad_id)
            .collect()
    };

    // 门限 1 m,排除未知:只剩 ISS(LARGE)
    let f = Filter {
        min_size_cm: 100.0,
        unknown_size: UnknownSizePolicy::Exclude,
        ..Filter::pass_all()
    };
    assert_eq!(ids(&f), vec![25544]);

    // 门限 50 cm,排除未知:ISS + 90002(0.80 m);90001 为 SMALL(< 0.36 m)确定不通过
    let f = Filter {
        min_size_cm: 50.0,
        unknown_size: UnknownSizePolicy::Exclude,
        ..Filter::pass_all()
    };
    assert_eq!(ids(&f), vec![25544, 90002]);

    // 门限 10 cm:SMALL 无法判定 → 由策略决定
    let f = Filter {
        min_size_cm: 10.0,
        unknown_size: UnknownSizePolicy::Exclude,
        ..Filter::pass_all()
    };
    assert_eq!(ids(&f), vec![25544, 90002]);
    let f = Filter {
        unknown_size: UnknownSizePolicy::Include,
        ..f
    };
    assert_eq!(ids(&f), vec![25544, 90001, 90002]);
}

#[test]
fn type_country_query_epoch_filters() {
    let cat = full_catalog();
    let ids = |f: &Filter| -> Vec<u32> {
        cat.select(f, t0())
            .iter()
            .map(|&i| cat.objects()[i].norad_id)
            .collect()
    };

    let f = Filter {
        object_types: vec![ObjectType::Debris],
        ..Filter::pass_all()
    };
    assert_eq!(ids(&f), vec![90001]);

    let f = Filter {
        countries: vec!["prc".into()],
        ..Filter::pass_all()
    };
    assert_eq!(ids(&f), vec![90001]);

    let f = Filter {
        query: Some("zarya".into()),
        ..Filter::pass_all()
    };
    assert_eq!(ids(&f), vec![25544]);
    let f = Filter {
        query: Some("25544".into()),
        ..Filter::pass_all()
    };
    assert_eq!(ids(&f), vec![25544]);
    let f = Filter {
        query: Some("1998-067".into()),
        ..Filter::pass_all()
    };
    assert_eq!(ids(&f), vec![25544]);

    let f = Filter {
        inclination_min_deg: Some(90.0),
        ..Filter::pass_all()
    };
    assert_eq!(ids(&f), vec![90001]);

    // 30 天后,14 天龄期上限 → 全部过期
    let f = Filter {
        max_epoch_age_days: Some(14.0),
        ..Filter::pass_all()
    };
    assert!(cat.select(&f, t0() + Duration::days(30)).is_empty());
}

#[test]
fn ground_track() {
    let cat = full_catalog();
    let idx = cat.index_of(25544).unwrap();
    let pts = cat
        .track(idx, t0(), t0() + Duration::minutes(93), Duration::seconds(60))
        .unwrap();
    assert_eq!(pts.len(), 94);
    let max_lat = pts.iter().map(|p| p.geodetic.lat_deg).fold(f64::MIN, f64::max);
    // 大地纬度可比地心纬度大约 0.19°,且瞬时倾角有短周期摆动,上界放宽到 52°
    assert!(
        max_lat > 45.0 && max_lat <= 52.0,
        "一圈内纬度应接近倾角: {max_lat}"
    );

    assert!(
        cat.track(idx, t0(), t0() + Duration::days(365), Duration::seconds(1))
            .is_err()
    );
    assert!(cat.track(idx, t0(), t0(), Duration::zero()).is_err());
    assert!(cat.track(9999, t0(), t0(), Duration::seconds(1)).is_err());
}

#[test]
fn stats() {
    let cat = full_catalog();
    let s = Stats::compute(cat.objects(), 100.0);
    assert_eq!(s.total, cat.len());
    assert_eq!(s.by_type.get("DEBRIS"), Some(&1));
    assert_eq!(
        s.altitude_histogram.iter().map(|(_, n)| n).sum::<usize>(),
        cat.len()
    );
}

#[test]
fn space_object_serializes() {
    let cat = full_catalog();
    let json = serde_json::to_value(cat.by_norad(25544).unwrap()).unwrap();
    assert_eq!(json["norad_id"], 25544);
    assert_eq!(json["object_type"], "PAYLOAD");
    assert_eq!(json["size"]["source"], "rcs_category");
    assert!(json["derived"]["perigee_alt_km"].as_f64().is_some());
}
