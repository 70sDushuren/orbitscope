//! SATCAT CSV 解析 → 每个 NORAD 编号的补充元数据。
//!
//! 兼容 CelesTrak SATCAT CSV(`OBJECT_TYPE` 为 PAY/R/B/DEB/UNK,`OWNER`,数值 `RCS` m²)
//! 与 Space-Track satcat CSV(`COUNTRY`、`RCS_SIZE`)。按列名取值,列顺序无关。
//!
//! 注意:Space-Track 的 `RCSVALUE` 对普通账号恒为 0,且自 2026-05-28 起单位改为"米"(中位 RCS),
//! 因此**不读取** `RCSVALUE`。

use orbit_core::model::{NoradId, ObjectMeta, ObjectType, RcsCategory, RcsInfo};
use std::collections::HashMap;

/// 解析结果。
#[derive(Debug, Default)]
pub struct SatcatOutcome {
    pub meta: HashMap<NoradId, ObjectMeta>,
    /// 无法解析的行数
    pub bad_rows: usize,
}

fn parse_date(s: &str) -> Option<chrono::NaiveDate> {
    chrono::NaiveDate::parse_from_str(s.trim().get(..10)?, "%Y-%m-%d").ok()
}

/// 解析 SATCAT CSV 文本。表头缺少 `NORAD_CAT_ID` 时返回错误。
pub fn parse_satcat_csv(text: &str) -> Result<SatcatOutcome, String> {
    let mut rdr = csv::ReaderBuilder::new()
        .flexible(true)
        .trim(csv::Trim::All)
        .from_reader(text.as_bytes());
    let headers = rdr.headers().map_err(|e| format!("读取表头失败: {e}"))?.clone();
    let col = |name: &str| headers.iter().position(|h| h.eq_ignore_ascii_case(name));
    let id_col = col("NORAD_CAT_ID").ok_or("缺少 NORAD_CAT_ID 列")?;
    let (name_c, intl_c, type_c) = (
        col("OBJECT_NAME").or(col("SATNAME")),
        col("OBJECT_ID").or(col("INTLDES")),
        col("OBJECT_TYPE"),
    );
    let owner_c = col("OWNER").or(col("COUNTRY"));
    let (launch_c, decay_c) = (
        col("LAUNCH_DATE").or(col("LAUNCH")),
        col("DECAY_DATE").or(col("DECAY")),
    );
    let (rcs_c, rcs_size_c) = (col("RCS"), col("RCS_SIZE"));

    let mut out = SatcatOutcome::default();
    for row in rdr.records() {
        let Ok(row) = row else {
            out.bad_rows += 1;
            continue;
        };
        let get = |c: Option<usize>| {
            c.and_then(|i| row.get(i))
                .map(str::trim)
                .filter(|s| !s.is_empty())
        };
        let Some(id) = get(Some(id_col)).and_then(|s| s.parse::<NoradId>().ok()) else {
            out.bad_rows += 1;
            continue;
        };
        let rcs = match (
            get(rcs_c).and_then(|s| s.parse::<f64>().ok()),
            get(rcs_size_c).and_then(RcsCategory::parse_lenient),
        ) {
            (Some(m2), _) if m2.is_finite() && m2 > 0.0 => RcsInfo::Numeric { m2 },
            (_, Some(category)) => RcsInfo::Category { category },
            _ => RcsInfo::Unknown,
        };
        let meta = ObjectMeta {
            name: get(name_c).map(String::from),
            intl_designator: get(intl_c).map(String::from),
            object_type: get(type_c).map_or(ObjectType::Unknown, ObjectType::parse_lenient),
            country: get(owner_c).map(String::from),
            launch_date: get(launch_c).and_then(parse_date),
            decay_date: get(decay_c).and_then(parse_date),
            rcs,
        };
        out.meta.insert(id, meta);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn celestrak_format() {
        let csv = "OBJECT_NAME,OBJECT_ID,NORAD_CAT_ID,OBJECT_TYPE,OPS_STATUS_CODE,OWNER,LAUNCH_DATE,LAUNCH_SITE,DECAY_DATE,PERIOD,INCLINATION,APOGEE,PERIGEE,RCS,DATA_STATUS_CODE,ORBIT_CENTER,ORBIT_TYPE\n\
ISS (ZARYA),1998-067A,25544,PAY,+,ISS,1998-11-20,TYMSC,,92.9,51.64,422,413,399.0524,,EA,ORB\n\
FENGYUN 1C DEB,1999-025ABC,90001,DEB,,PRC,1999-05-10,TSC,,100.6,98.7,795,775,0.0123,,EA,ORB\n\
OLD DEB,1960-001A,90005,DEB,,US,1960-01-01,AFETR,2001-01-01,,,,,,,EA,IMP\n\
bad,row,notanumber\n";
        let out = parse_satcat_csv(csv).unwrap();
        assert_eq!(out.meta.len(), 3);
        assert_eq!(out.bad_rows, 1);
        let deb = &out.meta[&90001];
        assert_eq!(deb.object_type, ObjectType::Debris);
        assert_eq!(deb.country.as_deref(), Some("PRC"));
        assert_eq!(deb.rcs, RcsInfo::Numeric { m2: 0.0123 });
        assert!(out.meta[&90005].decay_date.is_some());
        assert_eq!(out.meta[&90005].rcs, RcsInfo::Unknown);
    }

    #[test]
    fn spacetrack_format_ignores_rcsvalue() {
        let csv = "INTLDES,NORAD_CAT_ID,OBJECT_TYPE,SATNAME,COUNTRY,LAUNCH,DECAY,RCSVALUE,RCS_SIZE\n\
1998-067A,25544,PAYLOAD,ISS (ZARYA),ISS,1998-11-20,,0,LARGE\n";
        let out = parse_satcat_csv(csv).unwrap();
        let iss = &out.meta[&25544];
        assert_eq!(
            iss.rcs,
            RcsInfo::Category {
                category: RcsCategory::Large
            }
        );
        assert_eq!(iss.object_type, ObjectType::Payload);
        assert_eq!(iss.name.as_deref(), Some("ISS (ZARYA)"));
    }

    #[test]
    fn missing_id_column() {
        assert!(parse_satcat_csv("A,B\n1,2\n").is_err());
    }
}
