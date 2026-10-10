//! A1 주소 표기 유틸리티. 모든 인덱스는 0부터 시작한다(A1 = row 0, col 0).

use std::fmt;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CellRef {
    pub row: u32,
    pub col: u32,
}

/// 직사각형 범위. `rows`/`cols`는 1 이상이다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CellRange {
    pub row: u32,
    pub col: u32,
    pub rows: u32,
    pub cols: u32,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("잘못된 셀 주소: {0}")]
pub struct A1Error(pub String);

impl CellRef {
    pub const fn new(row: u32, col: u32) -> Self {
        Self { row, col }
    }

    /// `"B3"`, `"$B$3"` 형태를 해석한다.
    pub fn parse(s: &str) -> Result<Self, A1Error> {
        let err = || A1Error(s.to_string());
        let t = s.trim().replace('$', "");
        let split = t.find(|c: char| c.is_ascii_digit()).ok_or_else(err)?;
        let (letters, digits) = t.split_at(split);
        if letters.is_empty() || !letters.chars().all(|c| c.is_ascii_alphabetic()) {
            return Err(err());
        }
        let row: u32 = digits.parse().map_err(|_| err())?;
        if row == 0 {
            return Err(err());
        }
        Ok(Self { row: row - 1, col: col_from_letters(letters).ok_or_else(err)? })
    }
}

impl fmt::Display for CellRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", col_to_letters(self.col), self.row + 1)
    }
}

impl CellRange {
    pub const fn new(row: u32, col: u32, rows: u32, cols: u32) -> Self {
        Self { row, col, rows, cols }
    }

    pub const fn cell(at: CellRef) -> Self {
        Self { row: at.row, col: at.col, rows: 1, cols: 1 }
    }

    /// `"B3:D7"`, `"$B$3:$D$7"`, `"B3"` 형태를 해석한다.
    pub fn parse(s: &str) -> Result<Self, A1Error> {
        let mut parts = s.split(':');
        let a = CellRef::parse(parts.next().unwrap_or_default())?;
        let b = match parts.next() {
            Some(p) => CellRef::parse(p)?,
            None => a,
        };
        if parts.next().is_some() {
            return Err(A1Error(s.to_string()));
        }
        let (r0, r1) = (a.row.min(b.row), a.row.max(b.row));
        let (c0, c1) = (a.col.min(b.col), a.col.max(b.col));
        Ok(Self { row: r0, col: c0, rows: r1 - r0 + 1, cols: c1 - c0 + 1 })
    }

    pub const fn last_row(&self) -> u32 {
        self.row + self.rows - 1
    }

    pub const fn last_col(&self) -> u32 {
        self.col + self.cols - 1
    }

    pub const fn len(&self) -> u64 {
        self.rows as u64 * self.cols as u64
    }

    pub const fn is_single(&self) -> bool {
        self.rows == 1 && self.cols == 1
    }

    pub const fn contains(&self, at: CellRef) -> bool {
        at.row >= self.row && at.row <= self.last_row() && at.col >= self.col && at.col <= self.last_col()
    }

    /// `other`가 이 범위 안에 통째로 들어 있다.
    pub const fn contains_range(&self, other: &CellRange) -> bool {
        other.row >= self.row
            && other.last_row() <= self.last_row()
            && other.col >= self.col
            && other.last_col() <= self.last_col()
    }

    pub const fn top_left(&self) -> CellRef {
        CellRef { row: self.row, col: self.col }
    }

    /// 이 범위를 `origin` 기준의 상대 좌표로 옮긴다. `origin`보다 앞에 있으면 `None`.
    pub fn relative_to(&self, origin: CellRef) -> Option<Self> {
        Some(Self {
            row: self.row.checked_sub(origin.row)?,
            col: self.col.checked_sub(origin.col)?,
            rows: self.rows,
            cols: self.cols,
        })
    }

    pub fn intersect(&self, other: &Self) -> Option<Self> {
        let r0 = self.row.max(other.row);
        let c0 = self.col.max(other.col);
        let r1 = self.last_row().min(other.last_row());
        let c1 = self.last_col().min(other.last_col());
        (r0 <= r1 && c0 <= c1).then(|| Self { row: r0, col: c0, rows: r1 - r0 + 1, cols: c1 - c0 + 1 })
    }

    pub fn cells(&self) -> impl Iterator<Item = CellRef> + '_ {
        (self.row..=self.last_row()).flat_map(move |r| (self.col..=self.last_col()).map(move |c| CellRef::new(r, c)))
    }
}

impl fmt::Display for CellRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_single() {
            write!(f, "{}", self.top_left())
        } else {
            write!(f, "{}:{}", self.top_left(), CellRef::new(self.last_row(), self.last_col()))
        }
    }
}

/// 시트 안의 칸(여러 시트를 함께 취합할 때). `sheet`는 세션 안 시트 순번(불러온 순서).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SheetCell {
    pub sheet: u32,
    pub row: u32,
    pub col: u32,
}

impl SheetCell {
    pub const fn new(sheet: u32, at: CellRef) -> Self {
        Self { sheet, row: at.row, col: at.col }
    }

    pub const fn at(self) -> CellRef {
        CellRef { row: self.row, col: self.col }
    }
}

/// Excel 주소 앞에 붙이는 시트 이름: `인력계획!`, 글자·숫자·`_` 밖의 문자가 있거나 숫자로 시작하면 `'1분기 예산'!`.
pub fn sheet_prefix(name: &str) -> String {
    let plain = !name.is_empty()
        && !name.starts_with(|c: char| c.is_ascii_digit())
        && name.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '.');
    if plain { format!("{name}!") } else { format!("'{}'!", name.replace('\'', "''")) }
}

/// `인력계획!F7`, `'1분기 예산'!F7:H7` → (시트 이름, 나머지 주소). 시트 이름이 없으면 `None`.
pub fn split_sheet(text: &str) -> (Option<String>, &str) {
    let t = text.trim();
    if let Some(rest) = t.strip_prefix('\'') {
        // 따옴표 안의 '' 는 ' 하나다.
        let mut name = String::new();
        let mut chars = rest.char_indices().peekable();
        while let Some((i, c)) = chars.next() {
            if c == '\'' {
                if chars.peek().is_some_and(|(_, n)| *n == '\'') {
                    name.push('\'');
                    chars.next();
                    continue;
                }
                return match rest[i + 1..].strip_prefix('!') {
                    Some(addr) => (Some(name), addr),
                    None => (None, t),
                };
            }
            name.push(c);
        }
        return (None, t);
    }
    match t.rsplit_once('!') {
        Some((name, addr)) if !name.is_empty() => (Some(name.to_string()), addr),
        _ => (None, t),
    }
}

/// `시트!범위` 또는 `범위`를 (시트 순번, 범위)로. 시트 이름이 없으면 `default` 시트, 모르는 시트 이름이면 오류.
pub fn parse_sheet_range(text: &str, sheets: &[&str], default: usize) -> Result<(usize, CellRange), A1Error> {
    let (name, addr) = split_sheet(text);
    let sheet = match name {
        None => default,
        Some(n) => {
            sheets.iter().position(|s| s.trim() == n.trim()).ok_or_else(|| A1Error(format!("{n} 시트가 없습니다")))?
        }
    };
    Ok((sheet, CellRange::parse(addr)?))
}

/// `0 → "A"`, `25 → "Z"`, `26 → "AA"`.
pub fn col_to_letters(mut col: u32) -> String {
    let mut out = Vec::new();
    loop {
        out.push(b'A' + (col % 26) as u8);
        if col < 26 {
            break;
        }
        col = col / 26 - 1;
    }
    out.reverse();
    String::from_utf8(out).expect("ASCII")
}

/// `"A" → 0`, `"AA" → 26`. 대소문자를 구분하지 않는다.
pub fn col_from_letters(s: &str) -> Option<u32> {
    if s.is_empty() || s.len() > 3 {
        return None;
    }
    let mut n: u32 = 0;
    for ch in s.chars() {
        let d = ch.to_ascii_uppercase();
        if !d.is_ascii_uppercase() {
            return None;
        }
        n = n * 26 + (d as u32 - 'A' as u32 + 1);
    }
    Some(n - 1)
}

/// Excel 시트의 마지막 행(1부터).
const MAX_SHEET_ROW: i64 = 1_048_576;

/// 수식 안 상대 행 참조(`F3`, `$F3`, `Sheet1!F3:F9`)의 행을 `delta`만큼 옮긴다(위 행의 수식을 아래로 채울 때).
///
/// 절대 행(`F$3`)·문자열(`"A1"`)·시트 이름(`'A1'!`)·표 참조(`[...]`)·함수 이름(`LOG10(`)은 건드리지 않는다.
/// 행 번호만 보는 간단한 해석이라 화면 표시용이다(실제 계산은 Excel이 한다).
pub fn shift_formula_rows(formula: &str, delta: i64) -> String {
    let chars: Vec<char> = formula.chars().collect();
    let mut out = String::with_capacity(formula.len());
    let mut i = 0;
    let word = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '$';
    while i < chars.len() {
        let c = chars[i];
        // 문자열·따옴표 시트 이름·표 참조는 통째로 그대로 둔다.
        if let Some(close) = match c {
            '"' => Some('"'),
            '\'' => Some('\''),
            '[' => Some(']'),
            _ => None,
        } {
            let start = i;
            i += 1;
            while i < chars.len() {
                if chars[i] == close {
                    // 겹친 따옴표("" '')는 글자다.
                    if close != ']' && chars.get(i + 1) == Some(&close) {
                        i += 2;
                        continue;
                    }
                    break;
                }
                i += 1;
            }
            i = (i + 1).min(chars.len());
            out.extend(&chars[start..i]);
            continue;
        }
        let starts_word = word(c) && (i == 0 || !word(chars[i - 1]));
        if !starts_word {
            out.push(c);
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && word(chars[i]) {
            i += 1;
        }
        let token: String = chars[start..i].iter().collect();
        let next = chars.get(i).copied();
        match (next != Some('(') && next != Some('!')).then(|| shift_ref(&token, delta)).flatten() {
            Some(shifted) => out.push_str(&shifted),
            None => out.push_str(&token),
        }
    }
    out
}

/// `[$]열[$]행` 모양의 낱말이면 상대 행을 옮긴 것. 아니면(이름·숫자·절대 행) `None`.
fn shift_ref(token: &str, delta: i64) -> Option<String> {
    let body = token.strip_prefix('$').unwrap_or(token);
    let letters = body.chars().take_while(char::is_ascii_alphabetic).count();
    if letters == 0 || letters > 3 {
        return None;
    }
    let (col, rest) = body.split_at(letters);
    col_from_letters(col).filter(|&c| c < 16_384)?;
    if rest.starts_with('$') {
        return None; // 절대 행
    }
    if rest.is_empty() || !rest.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    let row: i64 = rest.parse().ok()?;
    let moved = row + delta;
    if row == 0 || row > MAX_SHEET_ROW || !(1..=MAX_SHEET_ROW).contains(&moved) {
        return None;
    }
    let prefix = &token[..token.len() - body.len()];
    Some(format!("{prefix}{col}{moved}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formulas_shift_relative_rows_only() {
        assert_eq!(shift_formula_rows("=F3/E3-1", 1), "=F4/E4-1");
        assert_eq!(shift_formula_rows("=SUM($F3:F$9)+$A$1", 2), "=SUM($F5:F$9)+$A$1");
        assert_eq!(shift_formula_rows("=LOG10(A2)&\"A1\"", 1), "=LOG10(A3)&\"A1\"");
        assert_eq!(shift_formula_rows("='A1'!B2+Sheet1!C3", 1), "='A1'!B3+Sheet1!C4");
        assert_eq!(shift_formula_rows("=표1[@금액]*1.1", 1), "=표1[@금액]*1.1");
        assert_eq!(shift_formula_rows("=IF(A1>0,\"a\"\"b\",B1)", 1), "=IF(A2>0,\"a\"\"b\",B2)");
        assert_eq!(shift_formula_rows("=A:A", 1), "=A:A");
        assert_eq!(shift_formula_rows("=ROUND(F3,0)", 3), "=ROUND(F6,0)");
    }

    #[test]
    fn letters_round_trip() {
        for (i, s) in
            [(0, "A"), (25, "Z"), (26, "AA"), (51, "AZ"), (52, "BA"), (701, "ZZ"), (702, "AAA"), (16383, "XFD")]
        {
            assert_eq!(col_to_letters(i), s);
            assert_eq!(col_from_letters(s), Some(i));
        }
        assert_eq!(col_from_letters("a"), Some(0));
        assert_eq!(col_from_letters(""), None);
        assert_eq!(col_from_letters("A1"), None);
    }

    #[test]
    fn parse_refs_and_ranges() {
        assert_eq!(CellRef::parse("$B$3").unwrap(), CellRef::new(2, 1));
        assert_eq!(CellRange::parse("$B$3:$B$7").unwrap(), CellRange::new(2, 1, 5, 1));
        assert_eq!(CellRange::parse("D7:B3").unwrap(), CellRange::new(2, 1, 5, 3));
        assert_eq!(CellRange::parse("F6").unwrap(), CellRange::new(5, 5, 1, 1));
        assert!(CellRef::parse("B0").is_err());
        assert!(CellRange::parse("A1:B2:C3").is_err());
        assert_eq!(CellRange::parse("A1:I1002").unwrap().to_string(), "A1:I1002");
    }

    #[test]
    fn range_ops() {
        let a = CellRange::parse("B2:D4").unwrap();
        let b = CellRange::parse("C3:F9").unwrap();
        assert_eq!(a.intersect(&b), Some(CellRange::parse("C3:D4").unwrap()));
        assert_eq!(a.intersect(&CellRange::parse("E5").unwrap()), None);
        assert!(a.contains(CellRef::new(3, 3)));
        assert_eq!(a.cells().count(), 9);
        assert_eq!(b.relative_to(CellRef::new(2, 2)), Some(CellRange::new(0, 0, 7, 4)));
    }

    #[test]
    fn sheet_qualified_addresses() {
        assert_eq!(sheet_prefix("인력계획"), "인력계획!");
        assert_eq!(sheet_prefix("1분기 예산"), "'1분기 예산'!");
        assert_eq!(sheet_prefix("Bob's"), "'Bob''s'!");
        assert_eq!(split_sheet("인력계획!F7"), (Some("인력계획".into()), "F7"));
        assert_eq!(split_sheet("'1분기 예산'!F7:H7"), (Some("1분기 예산".into()), "F7:H7"));
        assert_eq!(split_sheet("'Bob''s'!A1"), (Some("Bob's".into()), "A1"));
        assert_eq!(split_sheet("F7"), (None, "F7"));
        let sheets = ["예산신청서", "인력계획"];
        assert_eq!(parse_sheet_range("H5:H9", &sheets, 0).unwrap(), (0, CellRange::parse("H5:H9").unwrap()));
        assert_eq!(parse_sheet_range("인력계획!E5", &sheets, 0).unwrap(), (1, CellRange::parse("E5").unwrap()));
        assert!(parse_sheet_range("없는시트!E5", &sheets, 0).is_err());
        let c = SheetCell::new(1, CellRef::new(4, 2));
        assert_eq!((c.sheet, c.at()), (1, CellRef::new(4, 2)));
    }
}
