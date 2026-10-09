//! 참여자 입력 검사·정규화. 서버가 셀에 쓰기 전에 열 형식([`InputFormat`])대로 값을 고친다.
//!
//! - 숫자: `"9,800"`, `"9 800"`, `"9800천원"`(열 단위 붙임), `"5%"` → 숫자. 그 밖의 글자는 거절.
//! - 목록: 항목 중 하나와 같아야 한다(앞뒤 공백 무시).
//! - 날짜: `"2026-10-10"`, `"2026.10.10"`, `"2026/10/10"` → Excel 날짜 일련번호(1900 날짜 체계).
//! - 글자: 그대로(최대 길이 제한).
//! - 빈 값은 언제나 허용(지우기).

use crate::config::InputFormat;
use crate::sheet::CellValue;

pub const MAX_TEXT_CHARS: usize = 2000;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct InvalidInput(pub String);

pub fn normalize(format: &InputFormat, value: &CellValue) -> Result<CellValue, InvalidInput> {
    if value.is_empty() || matches!(value, CellValue::Text(s) if s.trim().is_empty()) {
        return Ok(CellValue::Empty);
    }
    if let CellValue::Error { .. } = value {
        return Err(InvalidInput("오류 값은 입력할 수 없습니다".into()));
    }
    match format {
        InputFormat::Text => match value {
            CellValue::Text(s) if s.chars().count() > MAX_TEXT_CHARS => {
                Err(InvalidInput(format!("{MAX_TEXT_CHARS}자까지 입력할 수 있습니다")))
            }
            CellValue::Text(s) => Ok(CellValue::Text(s.trim_end_matches(['\r', '\n']).to_string())),
            other => Ok(other.clone()),
        },
        InputFormat::Number { unit } => match value {
            CellValue::Number(n) if n.is_finite() => Ok(CellValue::Number(*n)),
            CellValue::Text(s) => parse_number(s, unit).map(CellValue::Number).ok_or_else(|| {
                InvalidInput(if unit.is_empty() {
                    "숫자만 입력할 수 있습니다".into()
                } else {
                    format!("숫자만 입력할 수 있습니다({unit} 단위)")
                })
            }),
            _ => Err(InvalidInput("숫자만 입력할 수 있습니다".into())),
        },
        InputFormat::List { items } => {
            let text = match value {
                CellValue::Text(s) => s.trim().to_string(),
                CellValue::Number(n) if n.fract() == 0.0 => format!("{}", *n as i64),
                CellValue::Number(n) => n.to_string(),
                CellValue::Bool(b) => if *b { "TRUE" } else { "FALSE" }.to_string(),
                _ => String::new(),
            };
            match items.iter().find(|i| i.trim() == text) {
                Some(item) => Ok(CellValue::Text(item.clone())),
                None => Err(InvalidInput(format!("목록에 있는 값만 고를 수 있습니다: {}", items.join(", ")))),
            }
        }
        InputFormat::Date => match value {
            CellValue::Number(n) if n.is_finite() && *n >= 1.0 => Ok(CellValue::Number(*n)),
            CellValue::Text(s) => parse_date(s)
                .map(CellValue::Number)
                .ok_or_else(|| InvalidInput("날짜 형식(예: 2026-10-10)으로 입력해 주세요".into())),
            _ => Err(InvalidInput("날짜 형식(예: 2026-10-10)으로 입력해 주세요".into())),
        },
    }
}

fn parse_number(text: &str, unit: &str) -> Option<f64> {
    let mut s: String = text.trim().to_string();
    if !unit.is_empty() {
        s = s.trim_end_matches(unit).to_string();
    }
    let percent = s.trim_end().ends_with('%');
    let cleaned: String = s.chars().filter(|c| !matches!(c, ',' | ' ' | '%' | '\u{a0}')).collect();
    if cleaned.is_empty() {
        return None;
    }
    let n: f64 = cleaned.parse().ok()?;
    n.is_finite().then_some(if percent { n / 100.0 } else { n })
}

/// 그레고리력 날짜 → 1970-01-01부터 센 날(Howard Hinnant의 days_from_civil).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// 날짜 → Excel 일련번호(1899-12-30 = 0).
pub fn excel_serial(y: i32, m: u32, d: u32) -> Option<f64> {
    if !(1..=12).contains(&m) || d == 0 || d > days_in_month(y, m) {
        return None;
    }
    Some((days_from_civil(y as i64, m as i64, d as i64) - days_from_civil(1899, 12, 30)) as f64)
}

fn days_in_month(y: i32, m: u32) -> u32 {
    match m {
        2 if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

fn parse_date(text: &str) -> Option<f64> {
    let parts: Vec<&str> = text.trim().trim_end_matches('.').split(['-', '.', '/']).map(str::trim).collect();
    let [y, m, d] = parts[..] else { return None };
    if y.len() != 4 {
        return None;
    }
    excel_serial(y.parse().ok()?, m.parse().ok()?, d.parse().ok()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn num(unit: &str) -> InputFormat {
        InputFormat::Number { unit: unit.into() }
    }

    fn t(s: &str) -> CellValue {
        CellValue::Text(s.into())
    }

    #[test]
    fn numbers() {
        assert_eq!(normalize(&num("천원"), &t("9,800")), Ok(CellValue::Number(9800.0)));
        assert_eq!(normalize(&num("천원"), &t(" 9 800천원 ")), Ok(CellValue::Number(9800.0)));
        assert_eq!(normalize(&num(""), &t("5%")), Ok(CellValue::Number(0.05)));
        assert_eq!(normalize(&num(""), &t("-1.5")), Ok(CellValue::Number(-1.5)));
        assert_eq!(normalize(&num(""), &CellValue::Number(3.0)), Ok(CellValue::Number(3.0)));
        assert!(normalize(&num("천원"), &t("구천팔백")).unwrap_err().0.contains("천원"));
        assert!(normalize(&num(""), &CellValue::Bool(true)).is_err());
    }

    #[test]
    fn empty_is_always_allowed() {
        for f in [num(""), InputFormat::Text, InputFormat::Date, InputFormat::List { items: vec!["a".into()] }] {
            assert_eq!(normalize(&f, &t("  ")), Ok(CellValue::Empty));
            assert_eq!(normalize(&f, &CellValue::Empty), Ok(CellValue::Empty));
        }
    }

    #[test]
    fn lists() {
        let f = InputFormat::List { items: vec!["1분기".into(), "연중".into(), "3".into()] };
        assert_eq!(normalize(&f, &t(" 연중 ")), Ok(t("연중")));
        assert_eq!(normalize(&f, &CellValue::Number(3.0)), Ok(t("3")));
        assert!(normalize(&f, &t("5분기")).unwrap_err().0.contains("1분기, 연중"));
    }

    #[test]
    fn dates_become_excel_serials() {
        assert_eq!(excel_serial(1900, 3, 1), Some(61.0)); // Excel 1900 윤년 버그 이후 구간
        assert_eq!(excel_serial(2026, 10, 10), Some(46305.0));
        assert_eq!(normalize(&InputFormat::Date, &t("2026.10.10")), Ok(CellValue::Number(46305.0)));
        assert_eq!(normalize(&InputFormat::Date, &t("2026/10/10")), Ok(CellValue::Number(46305.0)));
        assert!(normalize(&InputFormat::Date, &t("2026-02-30")).is_err());
        assert!(normalize(&InputFormat::Date, &t("10/10")).is_err());
    }

    #[test]
    fn text_limits() {
        assert_eq!(normalize(&InputFormat::Text, &t("간담회 4회\r\n")), Ok(t("간담회 4회")));
        assert!(normalize(&InputFormat::Text, &t(&"가".repeat(MAX_TEXT_CHARS + 1))).is_err());
        assert!(normalize(&InputFormat::Text, &CellValue::Error { err: 2042 }).is_err());
    }
}
