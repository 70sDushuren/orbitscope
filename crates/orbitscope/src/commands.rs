//! 子命令实现。

use crate::args::{FilterArgs, parse_duration, parse_time};
use crate::config::{self, AppConfig, EXAMPLE_CONFIG};
use crate::output::{
    Format, ObjectRow, SortKey, bar, fmt_time, kv_table, objects_table, size_label, size_source_label,
    write_csv, write_json,
};
use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use orbit_core::{Catalog, Filter, Snapshot, SpaceObject, StateVector, Stats};
use orbit_sources::spacetrack::Credentials;
use orbit_sources::{FetchAllResult, FetchMode, Origin, SourceSet, SourceStatus};
use std::cmp::Ordering;
use std::io::Write;
use std::path::{Path, PathBuf};

/// 运行上下文。
pub struct Ctx {
    pub cfg: AppConfig,
    pub config_path: Option<PathBuf>,
    pub offline: bool,
    pub quiet: bool,
}

fn describe_origin(o: &Origin) -> String {
    let ago = |s: i64| match s {
        s if s < 120 => format!("{s} 秒前"),
        s if s < 7_200 => format!("{} 分钟前", s / 60),
        s if s < 172_800 => format!("{:.1} 小时前", s as f64 / 3_600.0),
        s => format!("{:.1} 天前", s as f64 / 86_400.0),
    };
    match o {
        Origin::Network => "已下载".into(),
        Origin::NotModified => "服务器确认未更新".into(),
        Origin::Cache { age_secs } => format!("缓存,{}", ago(*age_secs)),
        Origin::StaleCache { age_secs, error } => format!("联网失败,使用旧缓存({}): {error}", ago(*age_secs)),
    }
}

fn print_status(s: &SourceStatus, verbose: bool) {
    match &s.error {
        Some(e) => eprintln!("✗ [{}] 失败: {e}", s.label),
        None => {
            let stale = s
                .resources
                .iter()
                .any(|r| matches!(r.origin, Origin::StaleCache { .. }));
            eprintln!(
                "{} [{}] {} 条记录{}{}",
                if stale { "!" } else { "✓" },
                s.label,
                s.records,
                if s.record_errors > 0 {
                    format!(",跳过 {} 条", s.record_errors)
                } else {
                    String::new()
                },
                if s.extra_meta > 0 {
                    format!(",元数据 {} 条", s.extra_meta)
                } else {
                    String::new()
                },
            );
            if verbose || stale {
                for r in &s.resources {
                    eprintln!("    {:<28} {}", r.key, describe_origin(&r.origin));
                }
            }
            for n in &s.notes {
                eprintln!("    注意: {n}");
            }
        }
    }
}

async fn fetch_sources(ctx: &Ctx, mode: FetchMode) -> Result<FetchAllResult> {
    let set = SourceSet::from_config(&ctx.cfg.sources, Credentials::from_env())?;
    // 缓存新鲜度始终按真实当前时间判断
    let result = set.fetch_all(mode, Utc::now()).await;
    Ok(result)
}

/// 获取数据并构建编目。`at` 用于判断再入。
async fn load_catalog(ctx: &Ctx, at: DateTime<Utc>) -> Result<Catalog> {
    let mode = if ctx.offline {
        FetchMode::CacheOnly
    } else {
        FetchMode::Normal
    };
    let result = fetch_sources(ctx, mode).await?;
    let any_problem = result.statuses.iter().any(|s| {
        !s.ok
            || s.resources
                .iter()
                .any(|r| matches!(r.origin, Origin::StaleCache { .. }))
            || !s.notes.is_empty()
    });
    if !ctx.quiet || any_problem {
        for s in &result.statuses {
            print_status(s, false);
        }
    }
    if !result.any_ok() {
        bail!(
            "所有数据源均失败{}",
            if ctx.offline {
                "(离线模式下需先运行 `orbitscope fetch`)"
            } else {
                ""
            }
        );
    }
    let (cat, report) = result.build_catalog(at);
    if !report.rejected.is_empty() {
        tracing::warn!(count = report.rejected.len(), "部分根数非法被剔除");
    }
    if cat.is_empty() {
        bail!("编目为空");
    }
    Ok(cat)
}

// ---------------------------------------------------------------- fetch

pub async fn fetch(ctx: &Ctx) -> Result<()> {
    let result = fetch_sources(ctx, FetchMode::Normal).await?;
    for s in &result.statuses {
        print_status(s, true);
    }
    if !result.any_ok() {
        bail!("所有数据源均失败");
    }
    let now = Utc::now();
    let (cat, report) = result.build_catalog(now);
    println!(
        "编目:收录 {} 个物体(输入 {},重复 {},已再入 {},非法 {})",
        report.accepted,
        report.input,
        report.duplicates,
        report.decayed,
        report.rejected.len()
    );
    let sel = cat.select(&ctx.cfg.filter, now);
    println!("按配置的过滤条件:{} 个", sel.len());
    Ok(())
}

// ---------------------------------------------------------------- list / export

fn sort_selection(
    cat: &Catalog,
    states: &mut [(usize, Option<StateVector>)],
    key: SortKey,
    desc: bool,
    at: DateTime<Utc>,
) {
    let o = |i: usize| &cat.objects()[i];
    let f = |a: f64, b: f64| a.partial_cmp(&b).unwrap_or(Ordering::Equal);
    states.sort_by(|(a, sa), (b, sb)| {
        let ord = match key {
            SortKey::Id => o(*a).norad_id.cmp(&o(*b).norad_id),
            SortKey::Name => o(*a).name.cmp(&o(*b).name),
            SortKey::Alt => f(
                sa.map_or(f64::NAN, |s| s.geodetic.alt_km),
                sb.map_or(f64::NAN, |s| s.geodetic.alt_km),
            ),
            SortKey::Perigee => f(o(*a).derived.perigee_alt_km, o(*b).derived.perigee_alt_km),
            SortKey::Apogee => f(o(*a).derived.apogee_alt_km, o(*b).derived.apogee_alt_km),
            SortKey::Inc => f(o(*a).derived.inclination_deg, o(*b).derived.inclination_deg),
            SortKey::Age => f(
                orbit_core::time::days_since(o(*a).epoch, at),
                orbit_core::time::days_since(o(*b).epoch, at),
            ),
            SortKey::Size => f(
                o(*a).size.representative_m().unwrap_or(-1.0),
                o(*b).size.representative_m().unwrap_or(-1.0),
            ),
        };
        if desc { ord.reverse() } else { ord }
    });
}

/// 过滤 + 外推,返回 (下标, 状态) 列表,外推失败的状态为 None(仍列出)。
fn compute(
    cat: &Catalog,
    filter: &Filter,
    at: DateTime<Utc>,
) -> (Vec<(usize, Option<StateVector>)>, Snapshot) {
    let idx = cat.select(filter, at);
    let snap = cat.snapshot(&idx, filter, at);
    let mut out: Vec<(usize, Option<StateVector>)> =
        snap.states.iter().map(|(i, s)| (*i, Some(*s))).collect();
    // instantaneous 模式下外推失败的物体无法判定高度,不列出;其他模式列出但无位置
    if !filter.needs_instantaneous() {
        out.extend(snap.failed.iter().map(|(i, _)| (*i, None)));
    }
    (out, snap)
}

pub struct ListOpts {
    pub filter: FilterArgs,
    pub at: String,
    pub sort: SortKey,
    pub desc: bool,
    pub limit: Option<usize>,
    pub format: Format,
    pub output: Option<PathBuf>,
}

fn open_output(path: Option<&Path>) -> Result<Box<dyn Write>> {
    Ok(match path {
        Some(p) => Box::new(std::io::BufWriter::new(
            std::fs::File::create(p).with_context(|| format!("创建输出文件 {}", p.display()))?,
        )),
        None => Box::new(std::io::stdout().lock()),
    })
}

pub async fn list(ctx: &Ctx, opts: ListOpts) -> Result<()> {
    let at = parse_time(&opts.at, Utc::now())?;
    let filter = opts.filter.apply(&ctx.cfg.filter)?;
    let cat = load_catalog(ctx, at).await?;
    let (mut sel, snap) = compute(&cat, &filter, at);
    sort_selection(&cat, &mut sel, opts.sort, opts.desc, at);
    let total = sel.len();
    if let Some(n) = opts.limit {
        sel.truncate(n);
    }

    let objs: Vec<&SpaceObject> = sel.iter().map(|(i, _)| &cat.objects()[*i]).collect();
    let rows: Vec<ObjectRow> = sel
        .iter()
        .zip(&objs)
        .map(|((_, s), o)| ObjectRow::new(o, s.as_ref(), at))
        .collect();

    let mut w = open_output(opts.output.as_deref())?;
    match opts.format {
        Format::Table => {
            writeln!(w, "{}", objects_table(&rows, &objs))?;
            writeln!(
                w,
                "时刻 {}  符合条件 {} 个{}{}",
                fmt_time(at),
                total,
                if rows.len() < total {
                    format!(",显示前 {}(--limit 调整)", rows.len())
                } else {
                    String::new()
                },
                if snap.failed.is_empty() {
                    String::new()
                } else {
                    format!(",外推失败 {}", snap.failed.len())
                },
            )?;
        }
        Format::Json => write_json(&mut w, &rows)?,
        Format::Csv => write_csv(&mut w, &rows)?,
    }
    w.flush()?;
    if let Some(p) = &opts.output {
        eprintln!("已写入 {} 条到 {}", rows.len(), p.display());
    }
    Ok(())
}

// ---------------------------------------------------------------- show

pub struct ShowOpts {
    pub norad_id: u32,
    pub at: String,
    pub track: Option<String>,
    pub step: String,
    pub format: Format,
}

pub async fn show(ctx: &Ctx, opts: ShowOpts) -> Result<()> {
    let at = parse_time(&opts.at, Utc::now())?;
    let cat = load_catalog(ctx, at).await?;
    let idx = cat
        .index_of(opts.norad_id)
        .with_context(|| format!("编目中没有 NORAD {}", opts.norad_id))?;
    let o = &cat.objects()[idx];
    let state = cat.propagate(idx, at);

    let track = match &opts.track {
        Some(span) => {
            let span = parse_duration(span)?;
            let step = parse_duration(&opts.step)?;
            Some(cat.track(idx, at, at + span, step)?)
        }
        None => None,
    };

    let mut out = std::io::stdout().lock();
    if opts.format != Format::Table {
        #[derive(serde::Serialize)]
        struct ShowJson<'a> {
            object: &'a SpaceObject,
            time: String,
            state: Option<StateVector>,
            error: Option<String>,
            track: Option<Vec<orbit_core::TrackPoint>>,
        }
        let (st, err) = match &state {
            Ok(s) => (Some(*s), None),
            Err(e) => (None, Some(e.to_string())),
        };
        return write_json(
            &mut out,
            &ShowJson {
                object: o,
                time: fmt_time(at),
                state: st,
                error: err,
                track,
            },
        );
    }

    let d = &o.derived;
    let mut rows = vec![
        ("NORAD", o.norad_id.to_string()),
        ("名称", o.name.clone()),
        (
            "国际编号",
            o.meta.intl_designator.clone().unwrap_or_else(|| "-".into()),
        ),
        ("类型", o.meta.object_type.to_string()),
        (
            "国家/所有者",
            o.meta.country.clone().unwrap_or_else(|| "-".into()),
        ),
        (
            "发射日期",
            o.meta.launch_date.map_or_else(|| "-".into(), |d| d.to_string()),
        ),
        ("数据来源", o.source.to_string()),
        (
            "根数历元",
            format!(
                "{}(龄期 {:.2} 天)",
                fmt_time(o.epoch),
                orbit_core::time::days_since(o.epoch, at)
            ),
        ),
        (
            "近地点 / 远地点",
            format!("{:.1} / {:.1} km", d.perigee_alt_km, d.apogee_alt_km),
        ),
        ("半长轴", format!("{:.1} km", d.semi_major_axis_km)),
        ("倾角", format!("{:.4}°", d.inclination_deg)),
        ("偏心率", format!("{:.7}", d.eccentricity)),
        ("周期", format!("{:.2} 分钟", d.period_min)),
        (
            "估算尺寸",
            format!(
                "{}(依据:{})",
                size_label(&o.size),
                size_source_label(o.size.source)
            ),
        ),
        ("时刻", fmt_time(at)),
    ];
    match &state {
        Ok(s) => {
            rows.push((
                "纬度 / 经度",
                format!("{:.4}° / {:.4}°", s.geodetic.lat_deg, s.geodetic.lon_deg),
            ));
            rows.push(("高度", format!("{:.2} km", s.geodetic.alt_km)));
            rows.push(("速度(惯性系)", format!("{:.4} km/s", s.speed_kms)));
            let [x, y, z] = s.ecef_pos_km;
            rows.push(("ECEF 位置", format!("({x:.1}, {y:.1}, {z:.1}) km")));
        }
        Err(e) => rows.push(("外推", format!("失败: {e}"))),
    }
    writeln!(out, "{}", kv_table(&rows))?;

    if let Some(points) = track {
        let mut t = comfy_table::Table::new();
        t.load_style(comfy_table::presets::UTF8_FULL_CONDENSED);
        t.set_header(vec!["时刻 (UTC)", "纬度°", "经度°", "高度km"]);
        for p in &points {
            t.add_row(vec![
                fmt_time(p.time),
                format!("{:.3}", p.geodetic.lat_deg),
                format!("{:.3}", p.geodetic.lon_deg),
                format!("{:.1}", p.geodetic.alt_km),
            ]);
        }
        writeln!(out, "地面轨迹({} 点)\n{t}", points.len())?;
    }
    Ok(())
}

// ---------------------------------------------------------------- stats

pub async fn stats(ctx: &Ctx, filter: FilterArgs, at: String, bin_km: f64, format: Format) -> Result<()> {
    let at = parse_time(&at, Utc::now())?;
    let filter = filter.apply(&ctx.cfg.filter)?;
    let cat = load_catalog(ctx, at).await?;
    let (sel, _) = compute(&cat, &filter, at);
    let s = Stats::compute(sel.iter().map(|(i, _)| &cat.objects()[*i]), bin_km);

    let mut out = std::io::stdout().lock();
    if format != Format::Table {
        return write_json(&mut out, &s);
    }
    writeln!(
        out,
        "时刻 {}  编目 {} 个,符合条件 {} 个\n",
        fmt_time(at),
        cat.len(),
        s.total
    )?;

    writeln!(out, "按类型:")?;
    let max = s.by_type.values().copied().max().unwrap_or(0);
    for (k, n) in &s.by_type {
        writeln!(out, "  {k:<12} {n:>7}  {}", bar(*n, max, 40))?;
    }

    let mut countries: Vec<(&String, &usize)> = s.by_country.iter().collect();
    countries.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    writeln!(out, "\n按国家/所有者(前 10):")?;
    let max = countries.first().map_or(0, |c| *c.1);
    for (k, n) in countries.iter().take(10) {
        writeln!(out, "  {k:<12} {n:>7}  {}", bar(**n, max, 40))?;
    }

    writeln!(out, "\n平均高度分布((近地点+远地点)/2,每 {} km):", s.bin_km)?;
    let max = s.altitude_histogram.iter().map(|(_, n)| *n).max().unwrap_or(0);
    for (lo, n) in &s.altitude_histogram {
        writeln!(
            out,
            "  {:>6.0}–{:<6.0} {n:>7}  {}",
            lo,
            lo + s.bin_km,
            bar(*n, max, 40)
        )?;
    }
    Ok(())
}

// ---------------------------------------------------------------- config

pub fn config_show(ctx: &Ctx) -> Result<()> {
    match &ctx.config_path {
        Some(p) => eprintln!("# 配置文件: {}", p.display()),
        None => eprintln!("# 未找到配置文件,使用默认值"),
    }
    print!("{}", toml::to_string_pretty(&ctx.cfg)?);
    Ok(())
}

pub fn config_init(path: Option<PathBuf>, force: bool) -> Result<()> {
    let path = match path {
        Some(p) => p,
        None => config::user_config_path().context("无法确定用户配置目录,请用 --path 指定")?,
    };
    if path.exists() && !force {
        bail!("{} 已存在(使用 --force 覆盖)", path.display());
    }
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).with_context(|| format!("创建目录 {}", dir.display()))?;
    }
    std::fs::write(&path, EXAMPLE_CONFIG).with_context(|| format!("写入 {}", path.display()))?;
    println!("已生成配置文件 {}", path.display());
    Ok(())
}
