//! 输出:物体行模型与表格 / JSON / CSV 渲染。

use anyhow::Result;
use chrono::{DateTime, SecondsFormat, Utc};
use clap::ValueEnum;
use comfy_table::presets::UTF8_FULL_CONDENSED;
use comfy_table::{Cell, CellAlignment, ContentArrangement, Table};
use orbit_core::time::days_since;
use orbit_core::{SizeEstimate, SizeSource, SpaceObject, StateVector};
use serde::Serialize;
use std::io::Write;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Format {
    Table,
    Json,
    Csv,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SortKey {
    Id,
    Name,
    Alt,
    Perigee,
    Apogee,
    Inc,
    Age,
    Size,
}

/// 一个物体在某时刻的扁平化输出行(CSV / JSON / 表格共用)。
#[derive(Debug, Clone, Serialize)]
pub struct ObjectRow {
    pub norad_id: u32,
    pub name: String,
    pub object_type: &'static str,
    pub country: Option<String>,
    pub intl_designator: Option<String>,
    pub source: String,
    pub epoch: String,
    pub epoch_age_days: f64,
    pub perigee_km: f64,
    pub apogee_km: f64,
    pub inclination_deg: f64,
    pub period_min: f64,
    pub size_min_m: Option<f64>,
    pub size_max_m: Option<f64>,
    pub size_source: SizeSource,
    pub time: String,
    pub lat_deg: Option<f64>,
    pub lon_deg: Option<f64>,
    pub alt_km: Option<f64>,
    pub speed_kms: Option<f64>,
}

fn round(v: f64, digits: i32) -> f64 {
    let p = 10f64.powi(digits);
    (v * p).round() / p
}

pub fn fmt_time(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Secs, true)
}

impl ObjectRow {
    pub fn new(o: &SpaceObject, state: Option<&StateVector>, t: DateTime<Utc>) -> Self {
        Self {
            norad_id: o.norad_id,
            name: o.name.clone(),
            object_type: o.meta.object_type.as_str(),
            country: o.meta.country.clone(),
            intl_designator: o.meta.intl_designator.clone(),
            source: o.source.to_string(),
            epoch: fmt_time(o.epoch),
            epoch_age_days: round(days_since(o.epoch, t), 3),
            perigee_km: round(o.derived.perigee_alt_km, 1),
            apogee_km: round(o.derived.apogee_alt_km, 1),
            inclination_deg: round(o.derived.inclination_deg, 4),
            period_min: round(o.derived.period_min, 3),
            size_min_m: o.size.min_diameter_m.map(|v| round(v, 3)),
            size_max_m: o.size.max_diameter_m.map(|v| round(v, 3)),
            size_source: o.size.source,
            time: fmt_time(t),
            lat_deg: state.map(|s| round(s.geodetic.lat_deg, 4)),
            lon_deg: state.map(|s| round(s.geodetic.lon_deg, 4)),
            alt_km: state.map(|s| round(s.geodetic.alt_km, 2)),
            speed_kms: state.map(|s| round(s.speed_kms, 4)),
        }
    }
}

/// 尺寸依据的中文说明。
pub fn size_source_label(s: SizeSource) -> &'static str {
    match s {
        SizeSource::NumericRcs => "数值 RCS",
        SizeSource::RcsCategory => "RCS 分档",
        SizeSource::Unknown => "无 RCS 数据",
    }
}

/// 尺寸的简短文字描述。
pub fn size_label(s: &SizeEstimate) -> String {
    match (s.source, s.min_diameter_m, s.max_diameter_m) {
        (SizeSource::Unknown, ..) => "?".into(),
        (SizeSource::NumericRcs, Some(d), _) => format!("≈{d:.2}m"),
        (_, Some(lo), Some(hi)) if lo > 0.0 => format!("{lo:.2}–{hi:.2}m"),
        (_, _, Some(hi)) => format!("<{hi:.2}m"),
        (_, Some(lo), None) => format!(">{lo:.2}m"),
        _ => "?".into(),
    }
}

fn opt(v: Option<f64>, digits: usize) -> String {
    v.map_or_else(|| "-".into(), |x| format!("{x:.digits$}"))
}

fn new_table(headers: &[&str]) -> Table {
    let mut t = Table::new();
    t.load_style(UTF8_FULL_CONDENSED)
        .set_content_arrangement(ContentArrangement::Dynamic);
    t.set_header(headers.iter().map(Cell::new));
    t
}

fn right_align(t: &mut Table, cols: &[usize]) {
    for &c in cols {
        if let Some(col) = t.column_mut(c) {
            col.set_cell_alignment(CellAlignment::Right);
        }
    }
}

/// 物体列表表格。`objs` 与 `rows` 一一对应。
pub fn objects_table(rows: &[ObjectRow], objs: &[&SpaceObject]) -> Table {
    let mut t = new_table(&[
        "NORAD",
        "名称",
        "类型",
        "国家",
        "纬度°",
        "经度°",
        "高度km",
        "近/远地点km",
        "倾角°",
        "尺寸",
        "龄期d",
    ]);
    for (r, o) in rows.iter().zip(objs) {
        t.add_row(vec![
            r.norad_id.to_string(),
            r.name.clone(),
            r.object_type.to_string(),
            r.country.clone().unwrap_or_else(|| "-".into()),
            opt(r.lat_deg, 2),
            opt(r.lon_deg, 2),
            opt(r.alt_km, 1),
            format!("{:.0}/{:.0}", r.perigee_km, r.apogee_km),
            format!("{:.2}", r.inclination_deg),
            size_label(&o.size),
            format!("{:.1}", r.epoch_age_days),
        ]);
    }
    right_align(&mut t, &[0, 4, 5, 6, 7, 8, 10]);
    t
}

/// 两列键值表。
pub fn kv_table(rows: &[(&str, String)]) -> Table {
    let mut t = new_table(&["字段", "值"]);
    for (k, v) in rows {
        t.add_row(vec![k.to_string(), v.clone()]);
    }
    t
}

pub fn write_json<T: Serialize + ?Sized>(w: &mut dyn Write, v: &T) -> Result<()> {
    serde_json::to_writer_pretty(&mut *w, v)?;
    writeln!(w)?;
    Ok(())
}

pub fn write_csv(w: &mut dyn Write, rows: &[ObjectRow]) -> Result<()> {
    let mut wtr = csv::Writer::from_writer(w);
    for r in rows {
        wtr.serialize(r)?;
    }
    wtr.flush()?;
    Ok(())
}

/// 用 `#` 画的横向条形。
pub fn bar(n: usize, max: usize, width: usize) -> String {
    if max == 0 {
        return String::new();
    }
    let len = ((n as f64 / max as f64) * width as f64).round() as usize;
    "█".repeat(len.max(usize::from(n > 0)))
}
