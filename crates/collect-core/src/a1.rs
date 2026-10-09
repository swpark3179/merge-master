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

#[cfg(test)]
mod tests {
    use super::*;

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
}
