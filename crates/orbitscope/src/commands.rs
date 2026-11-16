//! 子命令实现。

use crate::args::{FilterArgs, parse_duration, parse_time};
use crate::config::{self, AppConfig, EXAMPLE_CONFIG};
use crate::output::{
    Format, ObjectRow, SortKey, bar, fmt_time, kv_table, objects_table, size_label, size_source_label,
    write_csv, write_json,
};
use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use orbit_archive::{Archive, ArchiveReport, CollectMode, Provenance, TimePolicy};
use orbit_core::{Catalog, Filter, Snapshot, SpaceObject, StateVector, Stats};
use orbit_sources::spacetrack::Credentials;
use orbit_sources::{FetchAllResult, FetchMode, Origin, SourceSet, SourceStatus};
use std::cmp::Ordering;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// 运行上下文。
pub struct Ctx {
    pub cfg: AppConfig,
    pub config_path: Option<PathBuf>,
    pub offline: bool,
    pub quiet: bool,
    /// 命令行指定的时刻策略(覆盖配置)
    pub policy: Option<TimePolicy>,
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

/// 打开归档(历史库与快照都关闭时返回 `None`)。
pub fn open_archive(cfg: &AppConfig) -> Result<Option<Arc<Archive>>> {
    let a = &cfg.archive;
    if !a.history.enabled && !a.snapshots.enabled {
        return Ok(None);
    }
    Ok(Some(Arc::new(Archive::open(a).context("打开历史归档失败")?)))
}

/// 一次采集的结果。
pub struct Acquired {
    pub result: FetchAllResult,
    pub archive: Option<Arc<Archive>>,
    pub report: Option<ArchiveReport>,
}

/// 获取全部数据源并写入归档。
pub async fn acquire(cfg: &AppConfig, mode: FetchMode) -> Result<Acquired> {
    let set = SourceSet::from_config(&cfg.sources, Credentials::from_env())?;
    // 缓存新鲜度始终按真实当前时间判断
    let result = set.fetch_all(mode, Utc::now()).await;
    let archive = open_archive(cfg)?;
    let Some(arc) = archive.clone() else {
        return Ok(Acquired {
            result,
            archive: None,
            report: None,
        });
    };
    if !result.any_ok() {
        return Ok(Acquired {
            result,
            archive,
            report: None,
        });
    }
    let cache = set.cache().clone();
    let (result, report) = tokio::task::spawn_blocking(move || {
        let r = arc.record(&result, &cache, Utc::now());
        (result, r)
    })
    .await
    .context("归档任务失败")?;
    let report = match report {
        Ok(r) => Some(r),
        Err(e) => {
            // 归档失败不影响本次展示
            eprintln!("! 写入历史归档失败: {e}");
            None
        }
    };
    Ok(Acquired {
        result,
        archive,
        report,
    })
}

/// 编目及其数据依据。
pub struct LoadedCatalog {
    pub catalog: Catalog,
    pub provenance: Option<Provenance>,
}

impl Ctx {
    /// 生效的时刻策略:命令行 `--policy` 优先,其次配置。
    pub fn policy(&self) -> TimePolicy {
        self.policy.unwrap_or(self.cfg.archive.time_policy)
    }
}

fn fmt_opt_time(t: Option<DateTime<Utc>>) -> String {
    t.map_or_else(|| "-".into(), fmt_time)
}

/// 一行数据依据说明。
pub fn describe_provenance(p: &Provenance) -> String {
    let what = match p.policy {
        TimePolicy::KnownAt => format!("截至 {} 已获取的数据", fmt_time(p.data_time)),
        TimePolicy::NearestEpoch => format!("历元最接近 {} 的数据", fmt_time(p.data_time)),
        TimePolicy::Latest => "历史库中最新的数据".to_string(),
    };
    format!(
        "数据依据:{what}(策略 {}),{} 个物体,根数获取于 {} 至 {}",
        p.policy,
        p.records,
        fmt_opt_time(p.first_seen_min),
        fmt_opt_time(p.first_seen_max)
    )
}

/// 获取数据、写入归档,并按时刻策略构建编目。
///
/// 历史库可用时从历史库按 `at` 与策略选取根数(结果可复现);否则使用本次获取的最新数据。
async fn load_catalog(ctx: &Ctx, at: DateTime<Utc>) -> Result<LoadedCatalog> {
    let mode = if ctx.offline {
        FetchMode::CacheOnly
    } else {
        FetchMode::Normal
    };
    let Acquired { result, archive, .. } = acquire(&ctx.cfg, mode).await?;
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

    let history = archive.filter(|a| a.history().is_some());
    if let Some(a) = history {
        let policy = ctx.policy();
        let (cat, report, prov) = tokio::task::spawn_blocking(move || a.catalog_at(policy, at))
            .await
            .context("读取历史库任务失败")??
            .context("历史库不可用")?;
        if !ctx.quiet {
            eprintln!("{}", describe_provenance(&prov));
        }
        if !report.rejected.is_empty() {
            tracing::warn!(count = report.rejected.len(), "部分根数非法被剔除");
        }
        if cat.is_empty() {
            bail!(
                "历史库中没有满足条件的数据(策略 {policy},时刻 {})。{}",
                fmt_time(at),
                if result.any_ok() {
                    "该时刻可能早于第一次采集"
                } else {
                    "请先运行 `orbitscope fetch`"
                }
            );
        }
        return Ok(LoadedCatalog {
            catalog: cat,
            provenance: Some(prov),
        });
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
    Ok(LoadedCatalog {
        catalog: cat,
        provenance: None,
    })
}

// ---------------------------------------------------------------- fetch

fn print_archive_report(archive: &Archive, report: &ArchiveReport) {
    if let Some(h) = &report.history {
        let usage = archive.history().and_then(|h| h.usage().ok());
        println!(
            "历史库:本次 {} 条根数,新增 {} 条{}",
            h.records,
            h.new_elsets,
            usage
                .map(|u| format!("(累计 {} 条,{} 个物体)", u.elsets, u.objects))
                .unwrap_or_default()
        );
    }
    if report.snapshots_saved > 0 || report.snapshots_pruned > 0 {
        println!(
            "完整快照:保存 {} 个,清理旧快照 {} 个",
            report.snapshots_saved, report.snapshots_pruned
        );
    }
    if report.elsets_pruned > 0 {
        println!("历史清理:删除旧根数 {} 条", report.elsets_pruned);
    }
}

pub async fn fetch(ctx: &Ctx) -> Result<()> {
    let Acquired {
        result,
        archive,
        report,
    } = acquire(&ctx.cfg, FetchMode::Normal).await?;
    for s in &result.statuses {
        print_status(s, true);
    }
    if !result.any_ok() {
        bail!("所有数据源均失败");
    }
    if let (Some(a), Some(r)) = (&archive, &report) {
        print_archive_report(a, r);
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

// ---------------------------------------------------------------- collect

/// 常驻采集:立即采集一次,之后每小时在随机分钟(避开整点/半点)再采集。
pub async fn collect(ctx: &Ctx, once: bool) -> Result<()> {
    if ctx.cfg.collect.mode != CollectMode::Daemon && !once {
        eprintln!(
            "提示:配置中的采集策略为 {}({}),但正在运行常驻采集进程;如需由本进程负责采集,请设置 [collect] mode = \"daemon\"",
            ctx.cfg.collect.mode,
            ctx.cfg.collect.mode.label()
        );
    }
    let minute = orbit_server::scheduler::pick_refresh_minute(std::hash::BuildHasher::hash_one(
        &std::collections::hash_map::RandomState::new(),
        std::process::id(),
    ));
    let mut shutdown = std::pin::pin!(shutdown_signal());
    loop {
        let started = Utc::now();
        match acquire(&ctx.cfg, FetchMode::Normal).await {
            Ok(Acquired {
                result,
                archive,
                report,
            }) => {
                for s in &result.statuses {
                    print_status(s, false);
                }
                if let (Some(a), Some(r)) = (&archive, &report) {
                    print_archive_report(a, r);
                }
            }
            Err(e) => eprintln!("✗ 采集失败: {e:#}"),
        }
        if once {
            return Ok(());
        }
        let wake = orbit_server::scheduler::next_wake(Utc::now(), minute, 0);
        eprintln!("[{}] 本轮完成,下次采集 {}", fmt_time(started), fmt_time(wake));
        let sleep = (wake - Utc::now()).to_std().unwrap_or_default();
        tokio::select! {
            _ = tokio::time::sleep(sleep) => {}
            _ = &mut shutdown => {
                eprintln!("采集进程已停止");
                return Ok(());
            }
        }
    }
}

// ---------------------------------------------------------------- history

pub enum HistoryCmd {
    Status,
    Fetches(usize),
    Object(u32, usize),
    Snapshots(usize),
    Prune,
}

fn mb(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_048_576.0)
}

pub async fn history(ctx: &Ctx, cmd: HistoryCmd, format: Format) -> Result<()> {
    let archive = open_archive(&ctx.cfg)?
        .context("历史归档已关闭([archive.history] 与 [archive.snapshots] 均未启用)")?;
    let mut out = std::io::stdout().lock();
    match cmd {
        HistoryCmd::Status => {
            let a = archive.clone();
            let u = tokio::task::spawn_blocking(move || a.usage()).await??;
            if format != Format::Table {
                return write_json(&mut out, &u);
            }
            let cfg = &ctx.cfg.archive;
            let limit = |v: u64, unit: &str| {
                if v == 0 {
                    "不限".to_string()
                } else {
                    format!("{v} {unit}")
                }
            };
            let mut rows = vec![
                ("归档目录", u.dir.clone()),
                (
                    "默认时刻策略",
                    format!("{}({})", cfg.time_policy, cfg.time_policy.label()),
                ),
                (
                    "采集策略",
                    format!("{}({})", ctx.cfg.collect.mode, ctx.cfg.collect.mode.label()),
                ),
            ];
            match &u.history {
                Some(h) => {
                    rows.push(("历史库根数", format!("{} 条,{} 个物体", h.elsets, h.objects)));
                    rows.push(("采集记录", format!("{} 次", h.fetches)));
                    rows.push((
                        "获取时间范围",
                        format!(
                            "{} 至 {}",
                            fmt_opt_time(h.first_seen_min),
                            fmt_opt_time(h.first_seen_max)
                        ),
                    ));
                    rows.push((
                        "历史库大小",
                        format!("{}(上限 {})", mb(h.bytes), limit(cfg.history.max_total_mb, "MB")),
                    ));
                    rows.push(("保留时段", limit(u64::from(cfg.history.retain_days), "天")));
                }
                None => rows.push(("历史库", "已关闭".into())),
            }
            let sn = &u.snapshots;
            rows.push((
                "完整快照",
                if cfg.snapshots.enabled {
                    format!(
                        "{} 个,{}(上限 {} 个 / {})",
                        sn.files,
                        mb(sn.bytes),
                        if cfg.snapshots.max_files == 0 {
                            "不限".into()
                        } else {
                            cfg.snapshots.max_files.to_string()
                        },
                        limit(cfg.snapshots.max_total_mb, "MB")
                    )
                } else {
                    "已关闭".into()
                },
            ));
            rows.push((
                "快照时间范围",
                format!("{} 至 {}", fmt_opt_time(sn.oldest), fmt_opt_time(sn.newest)),
            ));
            writeln!(out, "{}", kv_table(&rows))?;
        }
        HistoryCmd::Fetches(n) => {
            let h = archive.clone();
            let rows =
                tokio::task::spawn_blocking(move || h.history().map(|h| h.recent_fetches(n)).transpose())
                    .await??
                    .context("历史库已关闭")?;
            if format != Format::Table {
                return write_json(&mut out, &rows);
            }
            let mut t = comfy_table::Table::new();
            t.load_style(comfy_table::presets::UTF8_FULL_CONDENSED);
            t.set_header(vec!["获取时间 (UTC)", "数据源", "资源", "方式", "大小"]);
            for r in &rows {
                let how = match r.origin.as_str() {
                    "network" => "下载",
                    "not_modified" => "确认未更新",
                    other => other,
                };
                t.add_row(vec![
                    fmt_time(r.fetched_at),
                    r.source.clone(),
                    r.resource.clone(),
                    how.into(),
                    mb(r.bytes),
                ]);
            }
            writeln!(out, "{t}\n共 {} 条(最近的在前)", rows.len())?;
        }
        HistoryCmd::Object(id, n) => {
            let h = archive.clone();
            let rows =
                tokio::task::spawn_blocking(move || h.history().map(|h| h.object_history(id, n)).transpose())
                    .await??
                    .context("历史库已关闭")?;
            if format != Format::Table {
                return write_json(&mut out, &rows);
            }
            if rows.is_empty() {
                bail!("历史库中没有 NORAD {id}");
            }
            let mut t = comfy_table::Table::new();
            t.load_style(comfy_table::presets::UTF8_FULL_CONDENSED);
            t.set_header(vec!["根数历元 (UTC)", "首次获取 (UTC)", "数据源"]);
            for r in &rows {
                t.add_row(vec![fmt_time(r.epoch), fmt_time(r.first_seen), r.source.clone()]);
            }
            writeln!(out, "NORAD {id} 的历史根数(历元从新到旧,最多 {n} 条)\n{t}")?;
        }
        HistoryCmd::Snapshots(n) => {
            let a = archive.clone();
            let mut files = tokio::task::spawn_blocking(move || a.snapshots().list()).await??;
            files.reverse();
            files.truncate(n);
            if format != Format::Table {
                return write_json(&mut out, &files);
            }
            let mut t = comfy_table::Table::new();
            t.load_style(comfy_table::presets::UTF8_FULL_CONDENSED);
            t.set_header(vec!["获取时间 (UTC)", "资源", "压缩后大小"]);
            for f in &files {
                t.add_row(vec![fmt_time(f.fetched_at), f.resource.clone(), mb(f.bytes)]);
            }
            writeln!(out, "{t}\n快照目录:{}", archive.snapshots().dir().display())?;
        }
        HistoryCmd::Prune => {
            let a = archive.clone();
            let r = tokio::task::spawn_blocking(move || a.prune(Utc::now())).await??;
            println!(
                "已清理:旧根数 {} 条,旧快照 {} 个",
                r.elsets_pruned, r.snapshots_pruned
            );
        }
    }
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
    let LoadedCatalog { catalog: cat, .. } = load_catalog(ctx, at).await?;
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
    let LoadedCatalog {
        catalog: cat,
        provenance,
    } = load_catalog(ctx, at).await?;
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
            provenance: Option<Provenance>,
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
                provenance,
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
    let LoadedCatalog { catalog: cat, .. } = load_catalog(ctx, at).await?;
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

// ---------------------------------------------------------------- serve

/// 等待 Ctrl-C(SIGINT)或 SIGTERM(`docker stop`、systemd 停止服务时发送)。
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(e) => {
                tracing::warn!(error = %e, "无法监听 SIGTERM");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {}
        _ = term => {}
    }
}

pub struct ServeOpts {
    pub bind: Option<String>,
    pub sim_time: Option<String>,
}

pub async fn serve(ctx: &Ctx, opts: ServeOpts) -> Result<()> {
    let cfg = &ctx.cfg;
    let bind = opts.bind.as_deref().unwrap_or(&cfg.server.bind);
    let bind: std::net::SocketAddr = bind.parse().with_context(|| format!("无效的监听地址 {bind:?}"))?;
    if cfg.server.tick_ms < 100 {
        bail!("server.tick_ms 不能小于 100");
    }
    let time_offset = match &opts.sim_time {
        Some(t) => {
            let now = Utc::now();
            parse_time(t, now)? - now
        }
        None => chrono::Duration::zero(),
    };
    let api_token = std::env::var("ORBITSCOPE_API_TOKEN")
        .ok()
        .filter(|t| !t.is_empty());
    if api_token.as_ref().is_some_and(|t| t.len() < 16) {
        bail!("ORBITSCOPE_API_TOKEN 至少需要 16 个字符");
    }

    let mut sources_cfg = cfg.sources.clone();
    if ctx.offline {
        // 离线:不联网,只用缓存与本地文件
        eprintln!("离线模式:服务只使用缓存与本地文件,不会刷新网络数据");
        sources_cfg.spacetrack.enabled = false;
    }
    let sources = SourceSet::from_config(&sources_cfg, Credentials::from_env())?;
    let mut cesium_base_url = cfg.server.cesium_url.trim().to_string();
    if !cesium_base_url.ends_with('/') {
        cesium_base_url.push('/');
    }
    if let Some(dir) = &cfg.server.cesium_dir
        && !dir.join("Cesium.js").is_file()
    {
        bail!(
            "server.cesium_dir 中找不到 Cesium.js: {}(应指向 Build/Cesium 目录)",
            dir.display()
        );
    }
    let archive = open_archive(cfg)?;
    let settings_path = archive.as_ref().map(|a| a.dir().join("ui-settings.json"));
    // 优先级:配置文件 < Web 界面保存的设置 < 命令行 --policy
    let mut time_policy = cfg.archive.time_policy;
    let mut collect = cfg.collect.clone();
    if let Some(saved) = settings_path
        .as_deref()
        .and_then(orbit_server::PersistedSettings::load)
    {
        eprintln!(
            "使用 Web 界面保存的设置:时刻策略 {},采集策略 {}",
            saved.time_policy, saved.collect_mode
        );
        time_policy = saved.time_policy;
        collect.mode = saved.collect_mode;
    }
    if let Some(p) = ctx.policy {
        time_policy = p;
    }
    eprintln!("采集策略:{}({})", collect.mode, collect.mode.label());
    if let Some(a) = &archive {
        eprintln!("历史归档:{}", a.dir().display());
    }
    let options = orbit_server::ServerOptions {
        archive,
        time_policy,
        collect,
        offline: ctx.offline,
        settings_path,
        bind,
        tick: std::time::Duration::from_millis(cfg.server.tick_ms),
        max_ws_clients: cfg.server.max_ws_clients.max(1),
        api_token,
        default_filter: cfg.filter.clone(),
        time_offset,
        refresh_minute: None,
        cesium_base_url,
        cesium_dir: cfg.server.cesium_dir.clone(),
    };
    eprintln!("OrbitScope 服务启动中:http://{bind}/");
    if !time_offset.is_zero() {
        eprintln!("模拟时间:服务端时钟偏移 {} 秒", time_offset.num_seconds());
    }
    orbit_server::serve(options, sources, shutdown_signal()).await?;
    eprintln!("服务已停止");
    Ok(())
}
