//! 请求参数:URL 查询串中的过滤条件,以及 JSON 局部过滤条件的合并。

use orbit_core::{AltitudeMode, Filter, ObjectType, UnknownSizePolicy};
use serde::Deserialize;

/// 各列表类接口共用的查询参数(未用到的字段被忽略)。
///
/// 过滤字段覆盖服务端默认过滤条件;列表字段用逗号分隔,如 `types=debris,rocket-body`。
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ListQuery {
    pub alt_min: Option<f64>,
    pub alt_max: Option<f64>,
    pub alt_mode: Option<AltitudeMode>,
    pub min_size_cm: Option<f64>,
    pub unknown_size: Option<UnknownSizePolicy>,
    pub types: Option<String>,
    pub countries: Option<String>,
    pub inc_min: Option<f64>,
    pub inc_max: Option<f64>,
    pub q: Option<String>,
    pub max_age_days: Option<f64>,
    pub any_age: Option<bool>,
    /// 时刻:now、RFC3339 或相对偏移(+90m 等)
    pub at: Option<String>,
    // ---- 列表 ----
    pub page: Option<usize>,
    pub per_page: Option<usize>,
    pub sort: Option<String>,
    pub desc: Option<bool>,
    // ---- 统计 ----
    pub bin_km: Option<f64>,
}

fn split_list(s: &str) -> impl Iterator<Item = &str> {
    s.split(',').map(str::trim).filter(|s| !s.is_empty())
}

/// 解析类型名,拒绝无法识别的值(避免拼写错误被静默当作 UNKNOWN)。
pub fn parse_type(s: &str) -> Result<ObjectType, String> {
    let norm = s.replace(['-', '_'], " ");
    let t = ObjectType::parse_lenient(&norm);
    let is_unknown_word = matches!(norm.to_ascii_uppercase().as_str(), "UNKNOWN" | "UNK");
    if t == ObjectType::Unknown && !is_unknown_word {
        return Err(format!(
            "未知的物体类型 {s:?}(可选 payload, rocket-body, debris, unknown)"
        ));
    }
    Ok(t)
}

impl ListQuery {
    /// 在 `base` 上叠加查询参数并校验。
    pub fn apply(&self, base: &Filter) -> Result<Filter, String> {
        let mut f = base.clone();
        if let Some(v) = self.alt_min {
            f.alt_min_km = v;
        }
        if let Some(v) = self.alt_max {
            f.alt_max_km = v;
        }
        if let Some(v) = self.alt_mode {
            f.altitude_mode = v;
        }
        if let Some(v) = self.min_size_cm {
            f.min_size_cm = v;
        }
        if let Some(v) = self.unknown_size {
            f.unknown_size = v;
        }
        if let Some(t) = &self.types {
            f.object_types = split_list(t).map(parse_type).collect::<Result<_, _>>()?;
        }
        if let Some(c) = &self.countries {
            f.countries = split_list(c).map(String::from).collect();
        }
        if self.inc_min.is_some() {
            f.inclination_min_deg = self.inc_min;
        }
        if self.inc_max.is_some() {
            f.inclination_max_deg = self.inc_max;
        }
        if self.q.is_some() {
            f.query.clone_from(&self.q);
        }
        if self.any_age == Some(true) {
            f.max_epoch_age_days = None;
        } else if self.max_age_days.is_some() {
            f.max_epoch_age_days = self.max_age_days;
        }
        f.validate().map_err(|e| e.to_string())?;
        Ok(f)
    }
}

/// 把 JSON 对象中的字段合并到 `base` 上(未出现的字段保持不变),再校验。
/// 未知字段会报错(`Filter` 使用 `deny_unknown_fields`)。
pub fn merge_filter(base: &Filter, patch: &serde_json::Value) -> Result<Filter, String> {
    let patch = match patch {
        serde_json::Value::Null => return Ok(base.clone()),
        serde_json::Value::Object(m) => m,
        _ => return Err("filter 必须是 JSON 对象".into()),
    };
    let mut v = serde_json::to_value(base).map_err(|e| e.to_string())?;
    if let serde_json::Value::Object(m) = &mut v {
        for (k, val) in patch {
            m.insert(k.clone(), val.clone());
        }
    }
    let f: Filter = serde_json::from_value(v).map_err(|e| format!("无效的过滤条件: {e}"))?;
    f.validate().map_err(|e| e.to_string())?;
    Ok(f)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn query_overrides() {
        let q = ListQuery {
            alt_min: Some(400.0),
            types: Some("debris, rocket-body".into()),
            any_age: Some(true),
            ..Default::default()
        };
        let f = q.apply(&Filter::default()).unwrap();
        assert_eq!(f.alt_min_km, 400.0);
        assert_eq!(f.object_types, vec![ObjectType::Debris, ObjectType::RocketBody]);
        assert_eq!(f.max_epoch_age_days, None);

        let bad = ListQuery {
            types: Some("debriss".into()),
            ..Default::default()
        };
        assert!(bad.apply(&Filter::default()).is_err());
        let bad = ListQuery {
            alt_min: Some(2000.0),
            ..Default::default()
        };
        assert!(bad.apply(&Filter::default()).is_err());
    }

    #[test]
    fn merge() {
        let f = merge_filter(
            &Filter::default(),
            &json!({"alt_max_km": 600, "object_types": ["DEB"]}),
        )
        .unwrap();
        assert_eq!(f.alt_max_km, 600.0);
        assert_eq!(f.alt_min_km, 100.0);
        assert_eq!(f.object_types, vec![ObjectType::Debris]);
        // null 清除可选字段
        let f = merge_filter(&Filter::default(), &json!({"max_epoch_age_days": null})).unwrap();
        assert_eq!(f.max_epoch_age_days, None);
        assert!(merge_filter(&Filter::default(), &json!({"typo": 1})).is_err());
        assert!(merge_filter(&Filter::default(), &json!({"alt_min_km": 5000})).is_err());
        assert!(merge_filter(&Filter::default(), &json!([1])).is_err());
    }
}
