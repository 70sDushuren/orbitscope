//! 命令行参数:通用过滤参数、时间解析。

use anyhow::{Result, anyhow};
use chrono::{DateTime, Duration, Utc};
use clap::{Args, ValueEnum};
use orbit_core::{AltitudeMode, Filter, ObjectType, UnknownSizePolicy};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum TypeArg {
    Payload,
    RocketBody,
    Debris,
    Unknown,
}

impl From<TypeArg> for ObjectType {
    fn from(t: TypeArg) -> Self {
        match t {
            TypeArg::Payload => ObjectType::Payload,
            TypeArg::RocketBody => ObjectType::RocketBody,
            TypeArg::Debris => ObjectType::Debris,
            TypeArg::Unknown => ObjectType::Unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum AltModeArg {
    OrbitOverlap,
    OrbitWithin,
    Instantaneous,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum UnknownSizeArg {
    Include,
    Exclude,
}

/// 过滤参数,覆盖配置文件 `[filter]` 中的对应项。
#[derive(Debug, Clone, Default, Args)]
pub struct FilterArgs {
    /// 高度下限(km)
    #[arg(long, value_name = "KM")]
    pub alt_min: Option<f64>,
    /// 高度上限(km)
    #[arg(long, value_name = "KM")]
    pub alt_max: Option<f64>,
    /// 高度范围简写,如 `400..600`
    #[arg(long, value_name = "MIN..MAX", conflicts_with_all = ["alt_min", "alt_max"])]
    pub alt: Option<String>,
    /// 高度判据
    #[arg(long, value_enum)]
    pub alt_mode: Option<AltModeArg>,
    /// 最小等效直径(cm,基于 RCS 估算)
    #[arg(long, value_name = "CM")]
    pub min_size: Option<f64>,
    /// 尺寸无法判定时的处理
    #[arg(long, value_enum)]
    pub unknown_size: Option<UnknownSizeArg>,
    /// 物体类型(可重复)
    #[arg(long = "type", value_enum, value_name = "TYPE")]
    pub types: Vec<TypeArg>,
    /// 国家/所有者代码(可重复)
    #[arg(long = "country", value_name = "CODE")]
    pub countries: Vec<String>,
    /// 倾角下限(°)
    #[arg(long, value_name = "DEG")]
    pub inc_min: Option<f64>,
    /// 倾角上限(°)
    #[arg(long, value_name = "DEG")]
    pub inc_max: Option<f64>,
    /// 名称子串 / NORAD 编号 / 国际编号前缀
    #[arg(short, long)]
    pub query: Option<String>,
    /// 根数龄期上限(天)
    #[arg(long, value_name = "DAYS", conflicts_with = "any_age")]
    pub max_age: Option<f64>,
    /// 不限制根数龄期
    #[arg(long)]
    pub any_age: bool,
}

/// 解析 `MIN..MAX`。
pub fn parse_range(s: &str) -> Result<(f64, f64)> {
    let (a, b) = s
        .split_once("..")
        .ok_or_else(|| anyhow!("范围格式应为 MIN..MAX,得到 {s:?}"))?;
    let a: f64 = a.trim().parse().map_err(|_| anyhow!("无效的下限 {a:?}"))?;
    let b: f64 = b.trim().parse().map_err(|_| anyhow!("无效的上限 {b:?}"))?;
    Ok((a, b))
}

impl FilterArgs {
    /// 把命令行参数叠加到配置中的过滤条件上,并校验。
    pub fn apply(&self, base: &Filter) -> Result<Filter> {
        let mut f = base.clone();
        if let Some(r) = &self.alt {
            (f.alt_min_km, f.alt_max_km) = parse_range(r)?;
        }
        if let Some(v) = self.alt_min {
            f.alt_min_km = v;
        }
        if let Some(v) = self.alt_max {
            f.alt_max_km = v;
        }
        if let Some(m) = self.alt_mode {
            f.altitude_mode = match m {
                AltModeArg::OrbitOverlap => AltitudeMode::OrbitOverlap,
                AltModeArg::OrbitWithin => AltitudeMode::OrbitWithin,
                AltModeArg::Instantaneous => AltitudeMode::Instantaneous,
            };
        }
        if let Some(v) = self.min_size {
            f.min_size_cm = v;
        }
        if let Some(u) = self.unknown_size {
            f.unknown_size = match u {
                UnknownSizeArg::Include => UnknownSizePolicy::Include,
                UnknownSizeArg::Exclude => UnknownSizePolicy::Exclude,
            };
        }
        if !self.types.is_empty() {
            f.object_types = self.types.iter().map(|&t| t.into()).collect();
        }
        if !self.countries.is_empty() {
            f.countries.clone_from(&self.countries);
        }
        if self.inc_min.is_some() {
            f.inclination_min_deg = self.inc_min;
        }
        if self.inc_max.is_some() {
            f.inclination_max_deg = self.inc_max;
        }
        if self.query.is_some() {
            f.query.clone_from(&self.query);
        }
        if self.any_age {
            f.max_epoch_age_days = None;
        } else if self.max_age.is_some() {
            f.max_epoch_age_days = self.max_age;
        }
        f.validate()?;
        Ok(f)
    }
}

/// 解析时间参数(见 [`orbit_core::time::parse_time_arg`])。
pub fn parse_time(s: &str, now: DateTime<Utc>) -> Result<DateTime<Utc>> {
    Ok(orbit_core::time::parse_time_arg(s, now)?)
}

/// 解析时长参数(见 [`orbit_core::time::parse_duration_arg`])。
pub fn parse_duration(s: &str) -> Result<Duration> {
    Ok(orbit_core::time::parse_duration_arg(s)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn times() {
        let now = Utc.with_ymd_and_hms(2024, 1, 1, 12, 0, 0).unwrap();
        assert_eq!(parse_time("now", now).unwrap(), now);
        assert_eq!(parse_time("+90m", now).unwrap(), now + Duration::minutes(90));
        assert_eq!(parse_time("-2h", now).unwrap(), now - Duration::hours(2));
        assert_eq!(parse_time("+1d", now).unwrap(), now + Duration::days(1));
        assert_eq!(
            parse_time("2024-01-02T00:00:00+08:00", now).unwrap(),
            Utc.with_ymd_and_hms(2024, 1, 1, 16, 0, 0).unwrap()
        );
        assert!(parse_time("+5x", now).is_err());
        assert!(parse_time("yesterday", now).is_err());
        assert_eq!(parse_duration("5m").unwrap(), Duration::minutes(5));
        assert!(parse_duration("0s").is_err());
    }

    #[test]
    fn filter_overrides() {
        let args = FilterArgs {
            alt: Some("400..600".into()),
            types: vec![TypeArg::Debris, TypeArg::RocketBody],
            any_age: true,
            ..Default::default()
        };
        let f = args.apply(&Filter::default()).unwrap();
        assert_eq!((f.alt_min_km, f.alt_max_km), (400.0, 600.0));
        assert_eq!(f.object_types, vec![ObjectType::Debris, ObjectType::RocketBody]);
        assert_eq!(f.max_epoch_age_days, None);
        assert_eq!(f.min_size_cm, 10.0, "未指定的项保持配置值");

        let bad = FilterArgs {
            alt: Some("600..400".into()),
            ..Default::default()
        };
        assert!(bad.apply(&Filter::default()).is_err());
        assert!(parse_range("abc").is_err());
    }
}
