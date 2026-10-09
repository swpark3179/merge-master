//! Excel에서 불러온 시트 하나를 표현하는 모델.
//!
//! 좌표는 모두 **불러온 범위 기준 0부터** 센다. 원본 시트의 위치는 [`SheetModel::origin`]에 남겨
//! 최종 저장 때 되돌려 쓴다.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::a1::{CellRange, CellRef};

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
    /// 숨긴 행/열(범위 기준). 참여자 화면에 보이지 않고 입력할 수 없다. '숨긴 행·열 포함'으로 불러오면 비어 있다.
    #[serde(default)]
    pub hidden_rows: Vec<u32>,
    #[serde(default)]
    pub hidden_cols: Vec<u32>,
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
