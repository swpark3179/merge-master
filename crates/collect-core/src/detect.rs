//! 불러온 시트에서 표 범위·머리글·배정 기준 열을 추정한다(1a '자동 감지').
//!
//! Excel 없이 [`SheetModel`]만 보고 판단한다. 감지는 제안일 뿐이고 호스트가 화면에서 고칠 수 있다.

use std::collections::BTreeSet;

use serde::Serialize;
use ts_rs::TS;

use crate::a1::{CellRange, CellRef};
use crate::sheet::{CellValue, MergeIndex, SheetModel};

/// 표 위쪽에서 제목·안내 행을 건너뛰며 머리글 시작을 찾는 범위.
const SCAN_ROWS: u32 = 15;
const MAX_HEADER_ROWS: u32 = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TableGuess {
    /// 표 범위(모델 기준). 머리글 첫 행부터 내용이 있는 마지막 행·열까지.
    pub range: CellRange,
    pub header_rows: u32,
}

/// 배정 기준 열 후보(예: C열 '담당자').
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AssigneeCandidate {
    /// 모델 기준 열.
    pub col: u32,
    pub label: String,
    /// 서로 다른 이름 수(= 명단 크기).
    pub distinct: u32,
    pub samples: Vec<String>,
    pub score: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct ColumnCount {
    pub col: u32,
    pub count: u32,
}

struct Scan<'a> {
    model: &'a SheetModel,
    index: MergeIndex,
}

impl<'a> Scan<'a> {
    fn new(model: &'a SheetModel) -> Self {
        Self { model, index: MergeIndex::new(model) }
    }

    /// 셀 자신에 값이나 수식이 있다(병합에 덮인 셀은 비어 있음).
    fn own(&self, r: u32, c: u32) -> bool {
        self.model.cell(CellRef::new(r, c)).is_some_and(|cell| !cell.v.is_empty() || cell.f.is_some())
    }

    /// 병합을 고려해 내용이 보인다(병합 영역이면 왼쪽 위 셀 기준).
    fn visible(&self, r: u32, c: u32) -> bool {
        let at = CellRef::new(r, c);
        match self.index.get(at) {
            Some(i) => {
                let tl = self.model.merges[i].top_left();
                self.own(tl.row, tl.col)
            }
            None => self.own(r, c),
        }
    }

    fn effective(&self, r: u32, c: u32) -> &CellValue {
        static EMPTY: CellValue = CellValue::Empty;
        self.model.effective_value_in(&self.index, CellRef::new(r, c)).unwrap_or(&EMPTY)
    }

    fn bold(&self, r: u32, c: u32) -> bool {
        let m = self.model;
        m.cell(CellRef::new(r, c)).and_then(|cell| m.styles.get(cell.s as usize)).is_some_and(|s| s.bold)
    }
}

/// 표 범위와 머리글 행 수를 추정한다.
///
/// 1. 아래·오른쪽(그리고 왼쪽)의 완전히 빈 행·열을 잘라낸다.
/// 2. 위에서부터 빈 행, 제목 행(내용 1칸), 표 너비의 절반도 채우지 않는 안내 행을 건너뛴 첫 행을 머리글 시작으로 본다.
/// 3. 머리글 행 수는 [`header_rows_at`] 규칙으로 센다.
pub fn detect_table(model: &SheetModel) -> TableGuess {
    let scan = Scan::new(model);
    let Some(last_row) = (0..model.rows).rev().find(|&r| (0..model.cols).any(|c| scan.own(r, c))) else {
        return TableGuess { range: model.bounds(), header_rows: 0 };
    };
    let col_span = |from_row: u32| {
        let has_col = |c: u32| (from_row..=last_row).any(|r| scan.visible(r, c));
        let last = (0..model.cols).rev().find(|&c| has_col(c)).unwrap_or(0);
        let first = (0..=last).find(|&c| has_col(c)).unwrap_or(0);
        (first, last)
    };
    let (first_col, last_col) = col_span(0);
    let width = last_col - first_col + 1;

    let mut start = 0;
    for r in 0..=last_row.min(SCAN_ROWS - 1) {
        let own = (first_col..=last_col).filter(|&c| scan.own(r, c)).count() as u32;
        let coverage = (first_col..=last_col).filter(|&c| scan.visible(r, c)).count() as u32;
        let is_title = width >= 3 && own == 1;
        let is_sparse = coverage * 2 < width;
        if r < last_row && (own == 0 || is_title || is_sparse) {
            start = r + 1;
            continue;
        }
        start = r;
        break;
    }
    // 제목 행의 가로 병합이 표보다 넓을 수 있어, 열 경계는 머리글부터 다시 잰다.
    let (first_col, last_col) = col_span(start);
    let range = CellRange::new(start, first_col, last_row - start + 1, last_col - first_col + 1);
    TableGuess { range, header_rows: header_rows_in(&scan, range) }
}

/// 범위 첫 행부터 머리글이 몇 행인지 센다(범위 = 불러온 표 전체일 때).
pub fn detect_header_rows(model: &SheetModel) -> u32 {
    header_rows_in(&Scan::new(model), model.bounds())
}

/// - 첫 행: 비어 있지 않고 모두 글자(수식·숫자 없음)면 머리글.
/// - 다음 행: 위 머리글의 세로 병합에 덮였거나, 모두 글자이면서 모두 굵게이거나 가로 병합이 있으면 머리글.
fn header_rows_in(scan: &Scan, range: CellRange) -> u32 {
    let model = scan.model;
    let cols = range.col..=range.last_col();
    let mut end: Option<u32> = None;
    let limit = (range.row + MAX_HEADER_ROWS).min(range.last_row() + 1);
    for r in range.row..limit {
        let mut non_empty = 0;
        let mut all_text = true;
        let mut all_bold = true;
        for c in cols.clone() {
            let Some(cell) = model.cell(CellRef::new(r, c)) else { continue };
            if cell.v.is_empty() && cell.f.is_none() {
                continue;
            }
            non_empty += 1;
            all_text &= matches!(cell.v, CellValue::Text(_)) && cell.f.is_none();
            all_bold &= scan.bold(r, c);
        }
        let h_merge =
            model.merges.iter().any(|m| m.row <= r && r <= m.last_row() && m.cols > 1 && cols.contains(&m.col));
        let extended = end.is_some_and(|e| r <= e);
        let header = if r == range.row {
            non_empty > 0 && all_text
        } else {
            extended || (non_empty > 0 && all_text && (all_bold || h_merge))
        };
        if !header {
            break;
        }
        // 머리글 행에서 시작하는 세로 병합(예: A1:A2 '본부')은 머리글을 그 아래까지 늘린다.
        let mut e = end.unwrap_or(r).max(r);
        for m in &model.merges {
            if m.row == r && m.rows > 1 && cols.contains(&m.col) {
                e = e.max(m.last_row().min(limit - 1));
            }
        }
        end = Some(e);
    }
    end.map_or(0, |e| e - range.row + 1)
}

/// 표의 열마다 머리글 이름: 가장 아래 머리글 행부터 위로 올라가며 처음 보이는 글자(병합 고려).
/// 머리글이 없으면 빈 문자열.
pub fn column_labels(model: &SheetModel, table: &TableGuess) -> Vec<String> {
    let scan = Scan::new(model);
    let r = table.range;
    (r.col..=r.last_col())
        .map(|c| {
            (r.row..r.row + table.header_rows)
                .rev()
                .find_map(|row| match scan.effective(row, c) {
                    CellValue::Text(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
                    _ => None,
                })
                .unwrap_or_default()
        })
        .collect()
}

/// 데이터 행(머리글 아래)의 수식 셀 수를 열마다 센다. 수식이 있는 열만 돌려준다.
pub fn formula_columns(model: &SheetModel, table: &TableGuess) -> Vec<ColumnCount> {
    let r = table.range;
    (r.col..=r.last_col())
        .filter_map(|c| {
            let count = (r.row + table.header_rows..=r.last_row())
                .filter(|&row| model.cell(CellRef::new(row, c)).is_some_and(|cell| cell.f.is_some()))
                .count() as u32;
            (count > 0).then_some(ColumnCount { col: c, count })
        })
        .collect()
}

const PERSON_WORDS: [&str; 12] =
    ["담당", "작성자", "입력자", "이름", "성명", "성함", "책임자", "제출자", "사원", "owner", "name", "assignee"];
const NOT_PERSON_WORDS: [&str; 10] = ["본부", "부서", "팀", "비목", "항목", "구분", "분류", "지역", "사업", "과목"];
/// 사람 이름이 아닌 한글 낱말이 흔히 끝나는 글자(부서·비목 등).
const NON_NAME_SUFFIXES: [char; 12] = ['팀', '부', '과', '실', '비', '료', '금', '품', '처', '국', '단', '소'];

fn name_like(s: &str) -> bool {
    let n = s.chars().count();
    (2..=4).contains(&n)
        && s.chars().all(|ch| ('가'..='힣').contains(&ch))
        && !s.chars().last().is_some_and(|ch| NON_NAME_SUFFIXES.contains(&ch))
}

/// 행 배정 기준이 될 만한 열(사람 이름이 반복되는 열)을 점수순으로 돌려준다.
pub fn assignee_candidates(model: &SheetModel, table: &TableGuess) -> Vec<AssigneeCandidate> {
    let scan = Scan::new(model);
    let labels = column_labels(model, table);
    let r = table.range;
    let data_rows: Vec<u32> = (r.row + table.header_rows..=r.last_row()).collect();
    if data_rows.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (i, c) in (r.col..=r.last_col()).enumerate() {
        let label = labels[i].clone();
        let mut texts = Vec::new();
        let mut numbers = 0;
        for &row in &data_rows {
            match scan.effective(row, c) {
                CellValue::Text(s) if !s.trim().is_empty() => texts.push(s.trim().to_string()),
                CellValue::Number(_) => numbers += 1,
                _ => {}
            }
        }
        if texts.is_empty() || numbers * 5 > data_rows.len() {
            continue;
        }
        let distinct: BTreeSet<&str> = texts.iter().map(String::as_str).collect();
        let lower = label.to_lowercase();
        let mut score = 0.0;
        if PERSON_WORDS.iter().any(|w| lower.contains(w)) {
            score += 3.0;
        }
        if NOT_PERSON_WORDS.iter().any(|w| lower.contains(w)) {
            score -= 3.0;
        }
        let names = distinct.iter().filter(|s| name_like(s)).count() as f32 / distinct.len() as f32;
        if names >= 0.7 {
            score += 2.0 * names;
        }
        if (2..=200).contains(&distinct.len()) && distinct.len() < texts.len() {
            score += 1.0;
        }
        if texts.len() * 5 >= data_rows.len() * 4 {
            score += 1.0;
        }
        if score >= 3.0 {
            out.push(AssigneeCandidate {
                col: c,
                label,
                distinct: distinct.len() as u32,
                samples: distinct.iter().take(5).map(|s| s.to_string()).collect(),
                score,
            });
        }
    }
    out.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.col.cmp(&b.col)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sheet::{Cell, CellStyle};

    /// 표 문자열로 모델을 만든다: `""` 빈칸, 숫자는 숫자, `=`로 시작하면 수식, `*`로 시작하면 굵게.
    fn grid(rows: &[&[&str]], merges: &[&str]) -> SheetModel {
        let cols = rows.iter().map(|r| r.len()).max().unwrap_or(0) as u32;
        let mut cells = Vec::new();
        for row in rows {
            for c in 0..cols as usize {
                let raw = row.get(c).copied().unwrap_or("");
                let (bold, text) = match raw.strip_prefix('*') {
                    Some(t) => (true, t),
                    None => (false, raw),
                };
                let mut cell = Cell { s: u32::from(bold), ..Default::default() };
                if let Some(f) = text.strip_prefix('=') {
                    cell.f = Some(format!("={f}"));
                    cell.v = CellValue::Number(0.0);
                } else if let Ok(n) = text.parse::<f64>() {
                    cell.v = CellValue::Number(n);
                } else if !text.is_empty() {
                    cell.v = CellValue::Text(text.into());
                }
                cells.push(cell);
            }
        }
        SheetModel {
            workbook: "t.xlsx".into(),
            sheet: "s".into(),
            origin: CellRef::new(0, 0),
            rows: rows.len() as u32,
            cols,
            cells,
            styles: vec![CellStyle::default(), CellStyle { bold: true, ..Default::default() }],
            merges: merges.iter().map(|a| CellRange::parse(a).unwrap()).collect(),
            col_widths: vec![50.0; cols as usize],
            row_heights: vec![15.0; rows.len()],
            header_rows: 0,
            frozen_rows: 0,
            frozen_cols: 0,
            validations: vec![],
            hidden_rows: vec![],
            hidden_cols: vec![],
        }
    }

    #[test]
    fn budget_like_table_with_two_header_rows() {
        let m = grid(
            &[
                &["*기본 정보", "", "", "*금액", ""],
                &["*본부", "*담당자", "*비목", "*전년", "*신청"],
                &["경영지원", "이서연", "교육훈련비", "9200", ""],
                &["", "", "회의비", "4400", "5000"],
                &["", "최도윤", "출장비", "1800", ""],
            ],
            &["A1:C1", "D1:E1", "A3:A5", "B3:B4"],
        );
        let t = detect_table(&m);
        assert_eq!(t, TableGuess { range: CellRange::parse("A1:E5").unwrap(), header_rows: 2 });
        assert_eq!(column_labels(&m, &t), ["본부", "담당자", "비목", "전년", "신청"]);
    }

    #[test]
    fn skips_title_note_and_blank_rows_and_trims_empty_edges() {
        let m = grid(
            &[
                &["2026년 부서별 예산 신청서", "", "", "", ""],
                &["", "", "", "", ""],
                &["작성일: 10/8", "", "", "", ""],
                &["*부서", "*담당자", "*금액", "*비고", ""],
                &["인사팀", "김민지", "100", "", ""],
                &["재무팀", "최도윤", "", "", ""],
                &["", "", "", "", ""],
            ],
            &["A1:E1"],
        );
        let t = detect_table(&m);
        assert_eq!(t.range, CellRange::parse("A4:D6").unwrap());
        assert_eq!(t.header_rows, 1);
    }

    #[test]
    fn vertical_header_merge_extends_header() {
        // A1:A2 '본부'가 세로 병합이고, 2행은 굵지 않아도 머리글이다.
        let m = grid(&[&["본부", "금액", ""], &["", "전년", "신청"], &["영업", "100", "200"]], &["A1:A2", "B1:C1"]);
        let t = detect_table(&m);
        assert_eq!(t.header_rows, 2);
        assert_eq!(column_labels(&m, &t), ["본부", "전년", "신청"]);
    }

    #[test]
    fn plain_first_row_header_and_no_header() {
        let m = grid(&[&["이름", "부서"], &["김민지", "인사팀"], &["최도윤", "재무팀"]], &[]);
        assert_eq!(detect_table(&m).header_rows, 1);
        let m = grid(&[&["100", "200"], &["300", "400"]], &[]);
        assert_eq!(detect_table(&m).header_rows, 0);
        let empty = grid(&[&["", ""]], &[]);
        assert_eq!(detect_table(&empty).header_rows, 0);
    }

    #[test]
    fn formula_columns_count_data_rows_only() {
        let m = grid(&[&["*a", "*b"], &["1", "=A2*2"], &["2", "=A3*2"]], &[]);
        let t = detect_table(&m);
        assert_eq!(formula_columns(&m, &t), [ColumnCount { col: 1, count: 2 }]);
    }

    #[test]
    fn assignee_column_ranks_first() {
        let m = grid(
            &[
                &["*본부", "*부서", "*담당자", "*비목", "*금액"],
                &["경영지원", "인사팀", "이서연", "교육훈련비", "1"],
                &["", "", "", "회의비", "2"],
                &["", "재무팀", "최도윤", "출장비", "3"],
                &["영업", "국내영업팀", "김민지", "회의비", "4"],
                &["", "", "", "소모품비", "5"],
            ],
            &["A2:A4", "B2:B3", "C2:C3", "A5:A6", "B5:B6", "C5:C6"],
        );
        let t = detect_table(&m);
        let c = assignee_candidates(&m, &t);
        assert_eq!(c.first().map(|c| (c.col, c.label.as_str(), c.distinct)), Some((2, "담당자", 3)));
        // 부서·비목·본부는 사람 이름 열로 보지 않는다.
        assert!(c.iter().all(|c| ![0, 1, 3].contains(&c.col)), "{c:?}");
    }

    #[test]
    fn name_heuristic() {
        assert!(name_like("김민지"));
        assert!(name_like("남궁민수"));
        assert!(!name_like("인사팀"));
        assert!(!name_like("회의비"));
        assert!(!name_like("Kim"));
        assert!(!name_like("김"));
    }
}
