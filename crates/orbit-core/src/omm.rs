//! OMM JSON 解析(CelesTrak `FORMAT=json` 与 Space-Track `class/gp/format/json`)。
//!
//! 两者字段名相同(CCSDS OMM 关键字),区别在于 Space-Track 把数值写成字符串,且附带
//! `OBJECT_TYPE`、`RCS_SIZE`、`COUNTRY_CODE`、`LAUNCH_DATE`、`DECAY_DATE` 等扩展字段。
//! 本模块对两种写法都宽松接受;**逐条**解析,单条失败只记入错误列表。

use crate::ParseOutcome;
use crate::error::RecordError;
use crate::model::{DataSource, GpRecord, NoradId, ObjectMeta, ObjectType, RcsCategory, RcsInfo};
use chrono::{NaiveDate, NaiveDateTime};
use serde::{Deserialize, Deserializer};
use serde_json::Value;

/// 宽松的 OMM 记录(所有字段可缺失,数值可为字符串)。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE", default)]
struct RawOmm {
    #[serde(deserialize_with = "de_opt_string")]
    object_name: Option<String>,
    #[serde(deserialize_with = "de_opt_string")]
    object_id: Option<String>,
    #[serde(deserialize_with = "de_opt_string")]
    epoch: Option<String>,
    #[serde(deserialize_with = "de_opt_f64")]
    mean_motion: Option<f64>,
    #[serde(deserialize_with = "de_opt_f64")]
    eccentricity: Option<f64>,
    #[serde(deserialize_with = "de_opt_f64")]
    inclination: Option<f64>,
    #[serde(deserialize_with = "de_opt_f64")]
    ra_of_asc_node: Option<f64>,
    #[serde(deserialize_with = "de_opt_f64")]
    arg_of_pericenter: Option<f64>,
    #[serde(deserialize_with = "de_opt_f64")]
    mean_anomaly: Option<f64>,
    #[serde(deserialize_with = "de_opt_f64")]
    ephemeris_type: Option<f64>,
    #[serde(deserialize_with = "de_opt_string")]
    classification_type: Option<String>,
    #[serde(deserialize_with = "de_opt_f64")]
    norad_cat_id: Option<f64>,
    #[serde(deserialize_with = "de_opt_f64")]
    element_set_no: Option<f64>,
    #[serde(deserialize_with = "de_opt_f64")]
    rev_at_epoch: Option<f64>,
    #[serde(deserialize_with = "de_opt_f64")]
    bstar: Option<f64>,
    #[serde(deserialize_with = "de_opt_f64")]
    mean_motion_dot: Option<f64>,
    #[serde(deserialize_with = "de_opt_f64")]
    mean_motion_ddot: Option<f64>,
    // ---- Space-Track 扩展字段 ----
    #[serde(deserialize_with = "de_opt_string")]
    object_type: Option<String>,
    #[serde(deserialize_with = "de_opt_string")]
    rcs_size: Option<String>,
    /// 部分数据源提供数值 RCS(m²)
    #[serde(deserialize_with = "de_opt_f64", alias = "RCS")]
    rcs_value: Option<f64>,
    #[serde(deserialize_with = "de_opt_string")]
    country_code: Option<String>,
    #[serde(deserialize_with = "de_opt_string")]
    launch_date: Option<String>,
    #[serde(deserialize_with = "de_opt_string")]
    decay_date: Option<String>,
}

fn de_opt_string<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::String(s) => {
            let t = s.trim();
            (!t.is_empty()).then(|| t.to_string())
        }
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    })
}

fn de_opt_f64<'de, D: Deserializer<'de>>(d: D) -> Result<Option<f64>, D::Error> {
    match Value::deserialize(d)? {
        Value::Number(n) => Ok(n.as_f64()),
        Value::String(s) if s.trim().is_empty() => Ok(None),
        Value::String(s) => s
            .trim()
            .parse::<f64>()
            .map(Some)
            .map_err(|e| serde::de::Error::custom(format!("无法解析数值 {s:?}: {e}"))),
        Value::Null => Ok(None),
        other => Err(serde::de::Error::custom(format!("期望数值,得到 {other}"))),
    }
}

/// 解析 OMM epoch:`2024-01-01T12:00:00.123456`,可带/不带小数秒,可带结尾 `Z`。
pub fn parse_epoch(s: &str) -> Option<NaiveDateTime> {
    let s = s.trim().trim_end_matches('Z');
    NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f")
        .or_else(|_| NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f"))
        .ok()
}

fn parse_date(s: &str) -> Option<NaiveDate> {
    // 兼容 "2023-12-25" 与 "2023-12-25T00:00:00"
    let head = s.trim().get(..10)?;
    NaiveDate::parse_from_str(head, "%Y-%m-%d").ok()
}

fn to_u64(v: f64, field: &str) -> Result<u64, String> {
    if v.is_finite() && v >= 0.0 && v.fract() == 0.0 && v <= u64::MAX as f64 {
        Ok(v as u64)
    } else {
        Err(format!("{field} 不是非负整数: {v}"))
    }
}

impl RawOmm {
    fn into_record(self, source: DataSource) -> Result<GpRecord, String> {
        fn req(v: Option<f64>, field: &str) -> Result<f64, String> {
            v.filter(|x| x.is_finite())
                .ok_or_else(|| format!("缺少字段 {field}"))
        }

        let norad = to_u64(req(self.norad_cat_id, "NORAD_CAT_ID")?, "NORAD_CAT_ID")?;
        if NoradId::try_from(norad).is_err() {
            return Err(format!("NORAD_CAT_ID 超出范围: {norad}"));
        }
        let epoch_str = self.epoch.ok_or("缺少字段 EPOCH")?;
        let datetime = parse_epoch(&epoch_str).ok_or_else(|| format!("无法解析 EPOCH {epoch_str:?}"))?;

        let classification = match self.classification_type.as_deref() {
            Some("C") => sgp4::Classification::Classified,
            Some("S") => sgp4::Classification::Secret,
            _ => sgp4::Classification::Unclassified,
        };

        let ephemeris_type = self.ephemeris_type.unwrap_or(0.0);
        let elements = sgp4::Elements {
            object_name: self.object_name.clone(),
            international_designator: self.object_id.clone(),
            norad_id: norad,
            classification,
            datetime,
            mean_motion_dot: self.mean_motion_dot.unwrap_or(0.0),
            mean_motion_ddot: self.mean_motion_ddot.unwrap_or(0.0),
            drag_term: self.bstar.unwrap_or(0.0),
            element_set_number: self
                .element_set_no
                .map_or(Ok(0), |v| to_u64(v, "ELEMENT_SET_NO"))?,
            inclination: req(self.inclination, "INCLINATION")?,
            right_ascension: req(self.ra_of_asc_node, "RA_OF_ASC_NODE")?,
            eccentricity: req(self.eccentricity, "ECCENTRICITY")?,
            argument_of_perigee: req(self.arg_of_pericenter, "ARG_OF_PERICENTER")?,
            mean_anomaly: req(self.mean_anomaly, "MEAN_ANOMALY")?,
            mean_motion: req(self.mean_motion, "MEAN_MOTION")?,
            revolution_number: self.rev_at_epoch.map_or(Ok(0), |v| to_u64(v, "REV_AT_EPOCH"))?,
            ephemeris_type: u8::try_from(to_u64(ephemeris_type, "EPHEMERIS_TYPE")?)
                .map_err(|_| format!("EPHEMERIS_TYPE 超出范围: {ephemeris_type}"))?,
        };

        let rcs = match (
            self.rcs_value,
            self.rcs_size.as_deref().and_then(RcsCategory::parse_lenient),
        ) {
            (Some(m2), _) if m2 > 0.0 => RcsInfo::Numeric { m2 },
            (_, Some(category)) => RcsInfo::Category { category },
            _ => RcsInfo::Unknown,
        };
        let object_type = match self.object_type.as_deref() {
            Some(t) => ObjectType::parse_lenient(t),
            None => self
                .object_name
                .as_deref()
                .map_or(ObjectType::Unknown, ObjectType::guess_from_name),
        };

        let meta = ObjectMeta {
            name: self.object_name,
            intl_designator: self.object_id,
            object_type,
            country: self.country_code,
            launch_date: self.launch_date.as_deref().and_then(parse_date),
            decay_date: self.decay_date.as_deref().and_then(parse_date),
            rcs,
        };
        Ok(GpRecord {
            elements,
            meta,
            source,
        })
    }
}

/// 解析 OMM JSON 文本(顶层为数组或单个对象)。
///
/// 顶层 JSON 语法错误时返回 `Err`;单条记录错误收集在 [`ParseOutcome::errors`] 中。
pub fn parse_omm_json(text: &str, source: DataSource) -> Result<ParseOutcome, crate::CoreError> {
    let root: Value = serde_json::from_str(text).map_err(|e| crate::CoreError::Json(e.to_string()))?;
    let items = match root {
        Value::Array(items) => items,
        obj @ Value::Object(_) => vec![obj],
        other => {
            return Err(crate::CoreError::Json(format!(
                "顶层应为数组或对象,得到 {}",
                match other {
                    Value::Null => "null",
                    Value::Bool(_) => "bool",
                    Value::Number(_) => "number",
                    Value::String(_) => "string",
                    _ => "unknown",
                }
            )));
        }
    };

    let mut out = ParseOutcome::default();
    out.records.reserve(items.len());
    for (index, item) in items.into_iter().enumerate() {
        let norad_hint = item
            .get("NORAD_CAT_ID")
            .and_then(|v| {
                v.as_u64()
                    .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
            })
            .and_then(|n| NoradId::try_from(n).ok());
        let result = RawOmm::deserialize(item)
            .map_err(|e| e.to_string())
            .and_then(|raw| raw.into_record(source));
        match result {
            Ok(rec) => out.records.push(rec),
            Err(message) => out.errors.push(RecordError {
                index,
                norad_id: norad_hint,
                message,
            }),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_formats() {
        assert!(parse_epoch("2024-01-01T12:00:00.123456").is_some());
        assert!(parse_epoch("2024-01-01T12:00:00").is_some());
        assert!(parse_epoch("2024-01-01T12:00:00Z").is_some());
        assert!(parse_epoch("2024-01-01").is_none());
    }

    #[test]
    fn string_and_number_fields() {
        let text = r#"[{"NORAD_CAT_ID":"25544","EPOCH":"2024-01-01T12:00:00","MEAN_MOTION":"15.5",
            "ECCENTRICITY":0.0007,"INCLINATION":"51.64","RA_OF_ASC_NODE":247.46,"ARG_OF_PERICENTER":"130.5",
            "MEAN_ANOMALY":325.0,"BSTAR":"","DECAY_DATE":null,"RCS_SIZE":"LARGE","OBJECT_TYPE":"PAYLOAD"}]"#;
        let out = parse_omm_json(text, DataSource::SpaceTrack).unwrap();
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        let r = &out.records[0];
        assert_eq!(r.norad_id(), 25544);
        assert_eq!(r.elements.mean_motion, 15.5);
        assert_eq!(r.elements.drag_term, 0.0);
        assert_eq!(
            r.meta.rcs,
            RcsInfo::Category {
                category: RcsCategory::Large
            }
        );
        assert_eq!(r.meta.object_type, ObjectType::Payload);
    }

    #[test]
    fn bad_top_level() {
        assert!(parse_omm_json("42", DataSource::File).is_err());
        assert!(parse_omm_json("not json", DataSource::File).is_err());
    }

    #[test]
    fn bad_record_reports_norad() {
        let text = r#"[{"NORAD_CAT_ID":"123","MEAN_MOTION":"abc"}]"#;
        let out = parse_omm_json(text, DataSource::File).unwrap();
        assert_eq!(out.records.len(), 0);
        assert_eq!(out.errors[0].norad_id, Some(123));
    }
}
