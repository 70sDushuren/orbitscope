//! M1 演示:读取 TLE/OMM 文件,按默认过滤条件在指定时刻外推并打印表格。
//!
//! ```bash
//! cargo run -p orbit-core --example demo -- testdata/spacetrack_gp.json 2024-01-01T12:30:00Z
//! cargo run -p orbit-core --example demo -- my.tle            # 时刻默认为当前时间
//! ```

use chrono::{DateTime, Utc};
use orbit_core::{Catalog, DataSource, Filter, Stats, parse_omm_json, parse_tle_text};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .ok_or("用法: demo <文件.json|.tle|.3le> [RFC3339 时刻]")?;
    let t: DateTime<Utc> = match args.next() {
        Some(s) => DateTime::parse_from_rfc3339(&s)?.with_timezone(&Utc),
        None => Utc::now(),
    };

    let text = std::fs::read_to_string(&path)?;
    let outcome = if text.trim_start().starts_with(['[', '{']) {
        parse_omm_json(&text, DataSource::File)?
    } else {
        parse_tle_text(&text, DataSource::File)
    };
    for e in &outcome.errors {
        eprintln!("跳过: {e}");
    }

    let (cat, report) = Catalog::build(outcome.records, t);
    eprintln!(
        "输入 {} 条,收录 {},重复 {},已再入 {},剔除 {}",
        report.input,
        report.accepted,
        report.duplicates,
        report.decayed,
        report.rejected.len()
    );

    // 演示用:不限根数龄期,其余为默认条件(100–1000 km,≥10 cm,未知尺寸包含)
    let filter = Filter {
        max_epoch_age_days: None,
        ..Filter::default()
    };
    let snap = cat.snapshot_filtered(&filter, t);

    println!("时刻 {t}  入选 {} 个物体", snap.states.len());
    println!(
        "{:>9}  {:<24} {:<11} {:>8} {:>9} {:>8} {:>7} {:>13}",
        "NORAD", "名称", "类型", "纬度°", "经度°", "高度km", "速度", "近/远地点km"
    );
    for (i, s) in &snap.states {
        let o = &cat.objects()[*i];
        println!(
            "{:>9}  {:<24} {:<11} {:>8.3} {:>9.3} {:>8.1} {:>7.3} {:>6.0}/{:<6.0}",
            o.norad_id,
            o.name.chars().take(24).collect::<String>(),
            o.meta.object_type.as_str(),
            s.geodetic.lat_deg,
            s.geodetic.lon_deg,
            s.geodetic.alt_km,
            s.speed_kms,
            o.derived.perigee_alt_km,
            o.derived.apogee_alt_km
        );
    }
    for (i, e) in &snap.failed {
        eprintln!("外推失败 #{i}: {e}");
    }

    let stats = Stats::compute(snap.states.iter().map(|(i, _)| &cat.objects()[*i]), 100.0);
    println!("按类型: {:?}", stats.by_type);
    Ok(())
}
