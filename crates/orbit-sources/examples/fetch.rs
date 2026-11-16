//! M2 演示:从真实数据源获取数据(遵守缓存与限速),构建编目并打印摘要。
//!
//! ```bash
//! # 只用 CelesTrak(免账号)
//! cargo run -p orbit-sources --example fetch
//! # 加上 Space-Track(需先设置 ORBITSCOPE_SPACETRACK_USER / ORBITSCOPE_SPACETRACK_PASSWORD)
//! cargo run -p orbit-sources --example fetch -- --spacetrack
//! # 只用缓存,不联网
//! cargo run -p orbit-sources --example fetch -- --offline
//! # 只用本地文件
//! cargo run -p orbit-sources --example fetch -- --no-celestrak --file testdata/sample.3le
//! ```

use chrono::Utc;
use orbit_core::{Filter, Stats};
use orbit_sources::spacetrack::Credentials;
use orbit_sources::{FetchMode, SourceSet, SourcesConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut cfg = SourcesConfig::default();
    let mut mode = FetchMode::Normal;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--offline" => mode = FetchMode::CacheOnly,
            "--spacetrack" => cfg.spacetrack.enabled = true,
            "--no-celestrak" => cfg.celestrak.enabled = false,
            "--file" => cfg.files.paths.push(args.next().ok_or("--file 需要路径")?.into()),
            "--cache-dir" => cfg.cache_dir = Some(args.next().ok_or("--cache-dir 需要路径")?.into()),
            other => return Err(format!("未知参数 {other}").into()),
        }
    }

    let set = SourceSet::from_config(&cfg, Credentials::from_env())?;
    println!("缓存目录: {}", set.cache().root().display());
    let now = Utc::now();
    let result = set.fetch_all(mode, now).await;
    for s in &result.statuses {
        println!(
            "[{}] ok={} 记录={} 跳过={} 元数据={}{}",
            s.label,
            s.ok,
            s.records,
            s.record_errors,
            s.extra_meta,
            s.error
                .as_deref()
                .map(|e| format!(" 错误: {e}"))
                .unwrap_or_default()
        );
        for r in &s.resources {
            println!("    {} {:?} ({} 字节)", r.key, r.origin, r.bytes);
        }
        for n in &s.notes {
            println!("    注意: {n}");
        }
    }
    if !result.any_ok() {
        return Err("所有数据源均失败".into());
    }

    let (cat, report) = result.build_catalog(now);
    println!(
        "编目: 收录 {} / 输入 {}(重复 {},已再入 {},剔除 {})",
        report.accepted,
        report.input,
        report.duplicates,
        report.decayed,
        report.rejected.len()
    );
    let sel = cat.select(&Filter::default(), now);
    let stats = Stats::compute(sel.iter().map(|&i| &cat.objects()[i]), 100.0);
    println!("默认过滤(100–1000 km, ≥10 cm)后: {} 个", stats.total);
    println!("  按类型: {:?}", stats.by_type);
    Ok(())
}
