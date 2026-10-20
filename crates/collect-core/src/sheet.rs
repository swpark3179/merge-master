//! Excel에서 불러온 시트 하나를 표현하는 모델.
//!
//! 좌표는 모두 **불러온 범위 기준 0부터** 센다. 원본 시트의 위치는 [`SheetModel::origin`]에 남겨
//! 최종 저장 때 되돌려 쓴다.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::a1::{CellRange, CellRef, shift_after_removal};

/// 셀 값. JSON에서는 `null | number | string | boolean | {"err": code}`로 직렬화된다.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, TS)]
#[serde(untagged)]
#[ts(export)]
pub enum CellValue {
    #[default]
    Empty,
    Number(f64),
    Text(String),
    Bool(bool),
    /// Excel 오류 값. `code`는 `#N/A` = 2042 같은 xlErr 코드.
    Error {
        err: i32,
    },
}

impl CellValue {
    pub fn is_empty(&self) -> bool {
        match self {
            CellValue::Empty => true,
            CellValue::Text(s) => s.is_empty(),
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Cell {
    /// 빈 값이면 JSON에서 생략된다.
    #[serde(default, skip_serializing_if = "CellValue::is_empty")]
    #[ts(as = "Option<CellValue>", optional)]
    pub v: CellValue,
    /// 수식 원문(`=F3/E3-1`). 수식이 아니면 없음.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub f: Option<String>,
    /// [`SheetModel::styles`]의 인덱스.
    #[serde(default)]
    pub s: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum HAlign {
    #[default]
    General,
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum VAlign {
    Top,
    Center,
    #[default]
    Bottom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct BorderEdge {
    /// Excel `XlLineStyle` 값 (1 = 실선, -4115 = 파선 …).
    pub style: i32,
    /// Excel `XlBorderWeight` 값 (1 = 가는, 2 = 보통 얇음, -4138 = 중간, 4 = 굵게).
    pub weight: i32,
    /// `0xRRGGBB`.
    pub color: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Borders {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub top: Option<BorderEdge>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub right: Option<BorderEdge>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub bottom: Option<BorderEdge>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub left: Option<BorderEdge>,
}

/// 셀 서식. 같은 서식은 [`SheetModel::styles`]에 한 번만 저장한다.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CellStyle {
    pub font_name: String,
    pub font_size: f32,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    /// 취소선.
    #[serde(default, skip_serializing_if = "is_false")]
    #[ts(as = "Option<bool>", optional)]
    pub strike: bool,
    /// 글자색 `0xRRGGBB`.
    pub color: u32,
    /// 채우기 `0xRRGGBB`. 채우기 없음이면 `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub fill: Option<u32>,
    /// Excel 표시 형식 코드(`#,##0`, `0.0%` …).
    pub number_format: String,
    pub h_align: HAlign,
    pub v_align: VAlign,
    pub wrap: bool,
    pub borders: Borders,
}

fn is_false(b: &bool) -> bool {
    !*b
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// 앱에서 표 아래에 붙인 행 하나와 그 행을 추가한 사람.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AddedRow {
    pub row: u32,
    pub by: String,
}

/// 데이터 유효성 중 참여자 입력 형식으로 바뀔 수 있는 것.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum ValidationRule {
    /// 목록. `source`는 원본 수식(`=코드표!$A$2:$A$6`), `items`는 읽어 온 실제 값.
    List { source: String, items: Vec<String> },
    /// 목록 외 유효성(정수·소수·날짜·길이 등). 원본 정보만 보존한다.
    Other {
        #[serde(rename = "excelType")]
        excel_type: i32,
        formula1: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Validation {
    pub range: CellRange,
    pub rule: ValidationRule,
}

/// 불러온 시트 범위 하나.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SheetModel {
    pub workbook: String,
    pub sheet: String,
    /// 원본 시트에서 불러온 범위의 왼쪽 위 셀(0부터).
    pub origin: CellRef,
    pub rows: u32,
    pub cols: u32,
    /// 행 우선(row-major) `rows * cols`개.
    pub cells: Vec<Cell>,
    pub styles: Vec<CellStyle>,
    pub merges: Vec<CellRange>,
    /// 열 너비(pt).
    pub col_widths: Vec<f32>,
    /// 행 높이(pt).
    pub row_heights: Vec<f32>,
    /// 머리글 행 수(위에서부터).
    pub header_rows: u32,
    /// 틀 고정 행/열 수(범위 기준).
    pub frozen_rows: u32,
    pub frozen_cols: u32,
    pub validations: Vec<Validation>,
    /// 숨긴 행/열(범위 기준). 참여자 화면에 보이지 않고 입력할 수 없다. 불러오기는 숨긴 줄을 펼치므로 채우지 않는다
    /// (예전 기록 파일, 시점별 '남의 행 숨김' 투영에 쓴다).
    #[serde(default)]
    pub hidden_rows: Vec<u32>,
    #[serde(default)]
    pub hidden_cols: Vec<u32>,
    /// Excel에서 숨겨져 있던 행/열(범위 기준). 불러올 때 기본 높이·너비로 펼쳐 읽고, 저장할 때 Excel에서도 숨김을 푼다.
    /// 원본에 반영하면 비운다.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[ts(as = "Option<Vec<u32>>", optional)]
    pub unhidden_rows: Vec<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[ts(as = "Option<Vec<u32>>", optional)]
    pub unhidden_cols: Vec<u32>,
    /// 앱에서 지운 불러온 행(기준 시트에만 쓴다): 지운 순서대로, 그 순간의 기준 시트 좌표. 저장할 때 Excel에서 이 순서로
    /// 지우면 기준 시트와 같아진다. 원본에 반영하면 쓴 만큼 빠진다.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[ts(as = "Option<Vec<RowCut>>", optional)]
    pub deleted_rows: Vec<RowCut>,
    /// 앱에서 표 아래에 붙인 행과 추가한 사람(행 오름차순). 원본에 저장해 기준 시트가 된 행은 빠진다.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[ts(as = "Option<Vec<AddedRow>>", optional)]
    pub added_rows: Vec<AddedRow>,
    /// 취소 표시한 행(오름차순). 화면은 행 전체를 회색 + 취소선으로 그리고, 필수 칸으로 세지 않는다.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[ts(as = "Option<Vec<u32>>", optional)]
    pub cancelled_rows: Vec<u32>,
    /// 구조 판: 앱에서 추가한 행·열을 지워 칸 좌표가 밀릴 때마다 1씩 는다. 참여자는 편집에 이 값을 실어 보내고,
    /// 서버는 다른 판에서 온 편집을 받지 않는다(오프라인 대기열이 밀린 칸에 쓰지 않게).
    #[serde(default, skip_serializing_if = "is_zero")]
    #[ts(as = "Option<u32>", optional)]
    pub layout: u32,
}

/// 지운 행 묶음 하나: 그때의 행 `from`부터 `count`개([`SheetModel::deleted_rows`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RowCut {
    pub from: u32,
    pub count: u32,
}

/// 셀 → 병합 영역 인덱스. 셀마다 병합 목록을 훑지 않도록 한 번 만들어 쓴다.
pub struct MergeIndex {
    cols: u32,
    at: Vec<i32>,
}

impl MergeIndex {
    pub fn new(model: &SheetModel) -> Self {
        let mut at = vec![-1; model.cells.len()];
        for (i, m) in model.merges.iter().enumerate() {
            for c in m.cells() {
                if c.row < model.rows && c.col < model.cols {
                    at[(c.row * model.cols + c.col) as usize] = i as i32;
                }
            }
        }
        Self { cols: model.cols, at }
    }

    pub fn get(&self, at: CellRef) -> Option<usize> {
        let i = *self.at.get((at.row * self.cols + at.col) as usize)?;
        (i >= 0).then_some(i as usize)
    }
}

impl SheetModel {
    pub fn cell(&self, at: CellRef) -> Option<&Cell> {
        if at.row >= self.rows || at.col >= self.cols {
            return None;
        }
        self.cells.get((at.row * self.cols + at.col) as usize)
    }

    pub fn cell_mut(&mut self, at: CellRef) -> Option<&mut Cell> {
        if at.row >= self.rows || at.col >= self.cols {
            return None;
        }
        self.cells.get_mut((at.row * self.cols + at.col) as usize)
    }

    pub fn bounds(&self) -> CellRange {
        CellRange::new(0, 0, self.rows, self.cols)
    }

    /// 셀이 속한 병합 영역. 병합되지 않았으면 `None`.
    pub fn merge_at(&self, at: CellRef) -> Option<&CellRange> {
        self.merges.iter().find(|m| m.contains(at))
    }

    /// 병합을 고려한 "보이는" 값: 병합 영역 안이면 왼쪽 위 셀의 값.
    /// 배정 기준 열(예: 병합된 '담당자')을 행마다 읽을 때 쓴다.
    pub fn effective_value(&self, at: CellRef) -> Option<&CellValue> {
        let src = self.merge_at(at).map_or(at, CellRange::top_left);
        self.cell(src).map(|c| &c.v)
    }

    /// 표 아래에 빈 행을 `count`개 붙이고 그 범위를 돌려준다(참여자 '행 추가').
    ///
    /// 새 행은 마지막 데이터 행을 본뜬다: 칸 서식·행 높이를 따르고, 수식 칸은 수식을 한 행씩 내려 쓴다(값은 비움 —
    /// Excel이 저장할 때 계산한다). 데이터 행이 없으면(머리글뿐) 기본 서식의 빈 행이다. 병합·유효성은 따르지 않는다.
    pub fn append_rows(&mut self, count: u32) -> CellRange {
        let start = self.rows;
        let template = (self.rows > self.header_rows).then(|| self.rows - 1);
        let height = template.and_then(|r| self.row_heights.get(r as usize).copied()).filter(|h| *h > 0.0);
        for k in 0..count {
            for c in 0..self.cols {
                let cell = match template.and_then(|r| self.cell(CellRef::new(r, c))) {
                    Some(src) => Cell {
                        v: CellValue::Empty,
                        f: src.f.as_deref().map(|f| crate::a1::shift_formula_rows(f, i64::from(k) + 1)),
                        s: src.s,
                    },
                    None => Cell::default(),
                };
                self.cells.push(cell);
            }
            self.row_heights.push(height.unwrap_or(15.0));
        }
        self.rows += count;
        CellRange::new(start, 0, count, self.cols.max(1))
    }

    /// 표 오른쪽에 빈 열을 `count`개 붙이고 그 범위를 돌려준다(호스트 '열 추가'). 칸 서식·열 너비는 마지막 열을 따른다
    /// (Excel이 열을 끼워 넣을 때 왼쪽 열 서식을 따르는 것과 같다). 수식·병합·유효성은 따르지 않는다.
    pub fn append_cols(&mut self, count: u32) -> CellRange {
        let (old, start) = (self.cols, self.cols);
        let cols = old + count;
        let mut cells = Vec::with_capacity((self.rows * cols) as usize);
        for r in 0..self.rows {
            let row = &self.cells[(r * old) as usize..((r + 1) * old) as usize];
            cells.extend_from_slice(row);
            let s = row.last().map_or(0, |c| c.s);
            cells.extend((0..count).map(|_| Cell { s, ..Cell::default() }));
        }
        self.cells = cells;
        let width = self.col_widths.last().copied().filter(|w| *w > 0.0).unwrap_or(48.0);
        self.col_widths.extend((0..count).map(|_| width));
        self.cols = cols;
        CellRange::new(0, start, self.rows.max(1), count)
    }

    /// 행 `from`부터 `count`개를 지운다. 아래 행·병합(한 칸만 남은 병합은 푼다)·유효성·숨김·추가한 행·취소 표시를
    /// 당기고 구조 판을 올린다. [`SheetModel::deleted_rows`]는 건드리지 않는다(세션이 기준 시트에 남긴다).
    pub fn remove_rows(&mut self, from: u32, count: u32) {
        let end = from.saturating_add(count).min(self.rows);
        if from >= end {
            return;
        }
        let count = end - from;
        let cols = self.cols as usize;
        self.cells.drain(from as usize * cols..end as usize * cols);
        self.row_heights.drain(from as usize..(end as usize).min(self.row_heights.len()));
        self.rows -= count;
        self.merges =
            self.merges.iter().filter_map(|m| m.without_rows(from, count)).filter(|m| !m.is_single()).collect();
        self.validations.retain_mut(|v| match v.range.without_rows(from, count) {
            Some(r) => {
                v.range = r;
                true
            }
            None => false,
        });
        let shift =
            |list: &mut Vec<u32>| *list = list.iter().filter_map(|&r| shift_after_removal(r, from, count)).collect();
        shift(&mut self.hidden_rows);
        shift(&mut self.unhidden_rows);
        shift(&mut self.cancelled_rows);
        self.added_rows = self
            .added_rows
            .iter()
            .filter_map(|a| Some(AddedRow { row: shift_after_removal(a.row, from, count)?, by: a.by.clone() }))
            .collect();
        self.header_rows -= self.header_rows.min(end).saturating_sub(from);
        self.frozen_rows -= self.frozen_rows.min(end).saturating_sub(from);
        self.layout += 1;
    }

    /// 열 `from`부터 `count`개를 지운다(앱에서 붙인 열 지우기). 오른쪽 열·병합·유효성·숨김을 당기고 구조 판을 올린다.
    pub fn remove_cols(&mut self, from: u32, count: u32) {
        let end = from.saturating_add(count).min(self.cols);
        if from >= end {
            return;
        }
        let count = end - from;
        let old = self.cols;
        let mut cells = Vec::with_capacity((self.rows * (old - count)) as usize);
        for r in 0..self.rows {
            for c in 0..old {
                if c < from || c >= end {
                    cells.push(self.cells[(r * old + c) as usize].clone());
                }
            }
        }
        self.cells = cells;
        self.col_widths.drain(from as usize..(end as usize).min(self.col_widths.len()));
        self.cols -= count;
        self.merges =
            self.merges.iter().filter_map(|m| m.without_cols(from, count)).filter(|m| !m.is_single()).collect();
        self.validations.retain_mut(|v| match v.range.without_cols(from, count) {
            Some(r) => {
                v.range = r;
                true
            }
            None => false,
        });
        let shift =
            |list: &mut Vec<u32>| *list = list.iter().filter_map(|&c| shift_after_removal(c, from, count)).collect();
        shift(&mut self.hidden_cols);
        shift(&mut self.unhidden_cols);
        self.frozen_cols -= self.frozen_cols.min(end).saturating_sub(from);
        self.layout += 1;
    }

    /// 행 `from`부터 `count`개를 취소 표시하거나(`on`) 푼다. 바뀐 것이 있으면 `true`.
    pub fn set_cancelled(&mut self, from: u32, count: u32, on: bool) -> bool {
        let end = from.saturating_add(count).min(self.rows);
        let before = self.cancelled_rows.clone();
        if on {
            self.cancelled_rows.extend(from..end);
            self.cancelled_rows.sort_unstable();
            self.cancelled_rows.dedup();
        } else {
            self.cancelled_rows.retain(|r| !(from..end).contains(r));
        }
        self.cancelled_rows != before
    }

    pub fn is_cancelled(&self, row: u32) -> bool {
        self.cancelled_rows.binary_search(&row).is_ok()
    }

    /// 그 행을 앱에서 붙였으면 추가한 사람.
    pub fn added_by(&self, row: u32) -> Option<&str> {
        self.added_rows.iter().find(|a| a.row == row).map(|a| a.by.as_str())
    }

    /// [`effective_value`](Self::effective_value)와 같지만 미리 만든 색인을 쓴다.
    pub fn effective_value_in(&self, index: &MergeIndex, at: CellRef) -> Option<&CellValue> {
        let src = index.get(at).map_or(at, |i| self.merges[i].top_left());
        self.cell(src).map(|c| &c.v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny() -> SheetModel {
        let mut cells = vec![Cell::default(); 6];
        cells[0].v = CellValue::Text("담당자".into());
        cells[2].v = CellValue::Text("김민지".into());
        cells[3].v = CellValue::Number(9800.0);
        SheetModel {
            workbook: "a.xlsx".into(),
            sheet: "s".into(),
            origin: CellRef::new(0, 0),
            rows: 3,
            cols: 2,
            cells,
            styles: vec![CellStyle::default()],
            merges: vec![CellRange::parse("A2:A3").unwrap()],
            col_widths: vec![60.0, 60.0],
            row_heights: vec![15.0; 3],
            header_rows: 1,
            frozen_rows: 1,
            frozen_cols: 0,
            validations: vec![],
            hidden_rows: vec![],
            hidden_cols: vec![],
            unhidden_rows: Vec::new(),
            unhidden_cols: Vec::new(),
            deleted_rows: Vec::new(),
            added_rows: vec![],
            cancelled_rows: vec![],
            layout: 0,
        }
    }

    #[test]
    fn effective_value_follows_merge() {
        let m = tiny();
        assert_eq!(m.effective_value(CellRef::new(2, 0)), Some(&CellValue::Text("김민지".into())));
        assert_eq!(m.effective_value(CellRef::new(2, 1)), Some(&CellValue::Empty));
        assert_eq!(m.cell(CellRef::new(3, 0)), None);
    }

    #[test]
    fn appended_rows_copy_the_last_row_style_and_formulas() {
        let mut m = tiny();
        m.cells[5].f = Some("=B2*2".into());
        m.cells[5].s = 0;
        m.cells[4].s = 0;
        m.row_heights[2] = 20.0;
        let r = m.append_rows(2);
        assert_eq!((r.row, r.rows, r.cols), (3, 2, 2));
        assert_eq!((m.rows, m.cells.len(), m.row_heights.len()), (5, 10, 5));
        assert_eq!(m.cell(CellRef::new(3, 1)).unwrap().f.as_deref(), Some("=B3*2"));
        assert_eq!(m.cell(CellRef::new(4, 1)).unwrap().f.as_deref(), Some("=B4*2"));
        assert!(m.cell(CellRef::new(4, 0)).unwrap().v.is_empty());
        assert_eq!(m.row_heights[4], 20.0);
        assert_eq!(m.merges.len(), 1, "병합은 따르지 않는다");

        let mut header_only = tiny();
        header_only.rows = 1;
        header_only.cells.truncate(2);
        header_only.row_heights.truncate(1);
        header_only.append_rows(1);
        assert_eq!(header_only.cells[2], Cell::default());
    }

    #[test]
    fn appended_cols_copy_the_last_column_style_and_width() {
        let mut m = tiny();
        m.cells[1].s = 0;
        m.styles.push(CellStyle { bold: true, ..CellStyle::default() });
        m.cells[3].s = 1;
        m.col_widths[1] = 80.0;
        let r = m.append_cols(2);
        assert_eq!((r.col, r.cols, r.rows), (2, 2, 3));
        assert_eq!((m.cols, m.cells.len(), m.col_widths.len()), (4, 12, 4));
        assert_eq!(m.cell(CellRef::new(1, 2)).unwrap().s, 1, "오른쪽 끝 열 서식");
        assert_eq!(m.cell(CellRef::new(1, 1)).unwrap().v, CellValue::Number(9800.0), "기존 칸은 그대로");
        assert!(m.cell(CellRef::new(1, 3)).unwrap().v.is_empty());
        assert_eq!(m.col_widths[3], 80.0);
    }

    #[test]
    fn removed_rows_and_cols_pull_up_what_follows() {
        let mut m = tiny();
        m.append_rows(3); // 행 3..=5
        m.cells[4 * 2].v = CellValue::Text("넷".into());
        m.cells[5 * 2].v = CellValue::Text("다섯".into());
        m.added_rows = vec![
            AddedRow { row: 3, by: "가".into() },
            AddedRow { row: 4, by: "나".into() },
            AddedRow { row: 5, by: "다".into() },
        ];
        m.cancelled_rows = vec![1, 5];
        m.merges.push(CellRange::parse("A5:B6").unwrap()); // 행 4..=5
        m.remove_rows(3, 1);
        assert_eq!((m.rows, m.cells.len(), m.row_heights.len(), m.layout), (5, 10, 5, 1));
        assert_eq!(m.cell(CellRef::new(3, 0)).unwrap().v, CellValue::Text("넷".into()));
        assert_eq!(m.added_rows, vec![AddedRow { row: 3, by: "나".into() }, AddedRow { row: 4, by: "다".into() }]);
        assert_eq!(m.cancelled_rows, vec![1, 4]);
        assert_eq!(m.merges[1], CellRange::parse("A4:B5").unwrap());
        assert_eq!(m.added_by(4), Some("다"));

        m.append_cols(2);
        m.cells[2].v = CellValue::Text("C1".into());
        m.cells[3].v = CellValue::Text("D1".into());
        m.remove_cols(2, 1);
        assert_eq!((m.cols, m.cells.len(), m.col_widths.len(), m.layout), (3, 15, 3, 2));
        assert_eq!(m.cell(CellRef::new(0, 2)).unwrap().v, CellValue::Text("D1".into()));
        assert_eq!(m.cell(CellRef::new(1, 0)).unwrap().v, CellValue::Text("김민지".into()));
    }

    #[test]
    fn cancelled_rows_stay_sorted() {
        let mut m = tiny();
        assert!(m.set_cancelled(2, 1, true));
        assert!(m.set_cancelled(1, 2, true));
        assert!(!m.set_cancelled(1, 1, true), "이미 표시된 행");
        assert_eq!(m.cancelled_rows, vec![1, 2]);
        assert!(m.is_cancelled(2));
        assert!(m.set_cancelled(2, 5, false));
        assert_eq!(m.cancelled_rows, vec![1]);
    }

    #[test]
    fn cell_value_json_shape() {
        let json = serde_json::to_string(&vec![
            CellValue::Empty,
            CellValue::Number(1.5),
            CellValue::Text("가".into()),
            CellValue::Bool(true),
            CellValue::Error { err: 2042 },
        ])
        .unwrap();
        assert_eq!(json, r#"[null,1.5,"가",true,{"err":2042}]"#);
        let back: Vec<CellValue> = serde_json::from_str(&json).unwrap();
        assert_eq!(back[4], CellValue::Error { err: 2042 });
        assert_eq!(back[0], CellValue::Empty);
    }

    #[test]
    fn empty_cell_serializes_compactly() {
        let c = Cell { s: 3, ..Default::default() };
        assert_eq!(serde_json::to_string(&c).unwrap(), r#"{"s":3}"#);
    }
}
