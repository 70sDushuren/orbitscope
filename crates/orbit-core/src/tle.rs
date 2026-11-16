//! TLE / 2LE / 3LE 文本解析(宽松、逐条容错)。
//!
//! 注意:TLE 只能表示 5 位(Alpha-5 扩展后约 34 万)编号;9 位编号必须使用 OMM。

use crate::ParseOutcome;
use crate::error::RecordError;
use crate::model::{DataSource, GpRecord, NoradId, ObjectMeta, ObjectType};

fn is_line1(l: &str) -> bool {
    l.len() >= 64 && l.starts_with("1 ")
}

fn is_line2(l: &str) -> bool {
    l.len() >= 64 && l.starts_with("2 ")
}

/// 解析 TLE 文本,自动识别 2LE(无名称行)与 3LE(名称行,可带 `0 ` 前缀)。
pub fn parse_tle_text(text: &str, source: DataSource) -> ParseOutcome {
    let lines: Vec<&str> = text
        .lines()
        .map(|l| l.trim_end_matches(['\r', ' ', '\t']))
        .filter(|l| !l.trim().is_empty())
        .collect();

    let mut out = ParseOutcome::default();
    let mut i = 0;
    let mut index = 0;
    while i < lines.len() {
        // 识别一组记录:[name] line1 line2
        let (name, l1, l2, consumed) = if is_line1(lines[i]) && lines.get(i + 1).is_some_and(|l| is_line2(l))
        {
            (None, lines[i], lines[i + 1], 2)
        } else if lines.get(i + 1).is_some_and(|l| is_line1(l))
            && lines.get(i + 2).is_some_and(|l| is_line2(l))
        {
            let raw = lines[i].trim();
            let name = raw.strip_prefix("0 ").unwrap_or(raw).trim().to_string();
            (Some(name), lines[i + 1], lines[i + 2], 3)
        } else {
            out.errors.push(RecordError {
                index,
                norad_id: None,
                message: format!("第 {} 行无法识别为 TLE 记录: {:?}", i + 1, lines[i]),
            });
            i += 1;
            index += 1;
            continue;
        };

        match sgp4::Elements::from_tle(name.clone(), l1.as_bytes(), l2.as_bytes()) {
            Ok(elements) => match NoradId::try_from(elements.norad_id) {
                Ok(_) => {
                    let object_type = name
                        .as_deref()
                        .map_or(ObjectType::Unknown, ObjectType::guess_from_name);
                    let meta = ObjectMeta {
                        name,
                        intl_designator: elements.international_designator.clone(),
                        object_type,
                        ..Default::default()
                    };
                    out.records.push(GpRecord {
                        elements,
                        meta,
                        source,
                        fetched_at: None,
                    });
                }
                Err(_) => out.errors.push(RecordError {
                    index,
                    norad_id: None,
                    message: format!("NORAD 编号超出范围: {}", elements.norad_id),
                }),
            },
            Err(e) => out.errors.push(RecordError {
                index,
                norad_id: l1.get(2..7).and_then(|s| s.trim().parse().ok()),
                message: format!("TLE 解析失败: {e}"),
            }),
        }
        i += consumed;
        index += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const ISS_2LE: &str = "1 25544U 98067A   24001.50000000  .00016717  00000-0  30306-3 0  9999\n\
                           2 25544  51.6416 247.4627 0006703 130.5360 325.0288 15.49815514432348\n";

    #[test]
    fn two_line() {
        let out = parse_tle_text(ISS_2LE, DataSource::File);
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        assert_eq!(out.records.len(), 1);
        assert_eq!(out.records[0].norad_id(), 25544);
        assert_eq!(out.records[0].meta.name, None);
    }

    #[test]
    fn three_line_with_zero_prefix_and_crlf() {
        let text = format!("0 ISS (ZARYA)\r\n{}", ISS_2LE.replace('\n', "\r\n"));
        let out = parse_tle_text(&text, DataSource::File);
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        assert_eq!(out.records[0].meta.name.as_deref(), Some("ISS (ZARYA)"));
        assert_eq!(out.records[0].meta.intl_designator.as_deref(), Some("1998-067A"));
    }

    #[test]
    fn garbage_line_is_skipped() {
        let text = format!("garbage\n{ISS_2LE}");
        let out = parse_tle_text(&text, DataSource::File);
        // "garbage" 被当作名称行,记录仍可解析
        assert_eq!(out.records.len(), 1);
        let text = format!("garbage\nmore garbage\n{ISS_2LE}");
        let out = parse_tle_text(&text, DataSource::File);
        assert_eq!(out.records.len(), 1);
        assert_eq!(out.errors.len(), 1);
    }
}
