//! 사전 감지(1a '범위 · 머리글'과 '감지 결과'): 시트를 고르자마자 가볍게 읽어 표 범위·머리글·틀 고정·배정 기준
//! 열 후보를 제안한다. 값·수식만 전체를 읽고, 병합·굵게는 머리글을 찾는 위쪽 몇 행만 읽어 빠르다.

use std::time::Instant;

use collect_core::detect::{
    AssigneeCandidate, ColumnCount, assignee_candidates, column_labels, detect_table, formula_columns,
};
use collect_core::{CellRange, CellStyle, SheetModel, ValidationRule};
use serde::Serialize;
use ts_rs::TS;

use crate::dispatch::com_calls;
use crate::error::Result;
use crate::reader::{
    Reporter, Sheet, check_size, read_bold, read_dimensions, read_formulas, read_freeze, read_merges, read_validations,
    read_values, used_range,
};
use crate::session::Session;

/// 머리글을 찾느라 병합·굵게를 읽을 위쪽 행 수(제목·안내 행 + 다단 머리글).
const HEAD_SCAN_ROWS: u32 = 21;

/// 열 하나의 이름(감지된 머리글 기준).
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct ColumnInfo {
    /// 시트 기준 열(0 = A).
    pub col: u32,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct ValidationInfo {
    /// 시트 기준 범위.
    pub range: CellRange,
    /// 그 열의 머리글 이름.
    pub label: String,
    pub rule: ValidationRule,
}

/// 사전 감지 결과. 좌표는 모두 **시트 기준**(A1 = 0,0)이다.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SheetInspection {
    pub used_range: CellRange,
    /// 제안하는 표 범위(머리글 첫 행부터).
    pub table: CellRange,
    pub header_rows: u32,
    /// Excel 창의 틀 고정(A1부터 센 행·열 수).
    pub frozen_rows: u32,
    pub frozen_cols: u32,
    /// 표 안 숨긴 행·열 수.
    pub hidden_rows: u32,
    pub hidden_cols: u32,
    pub columns: Vec<ColumnInfo>,
    pub formula_columns: Vec<ColumnCount>,
    pub validations: Vec<ValidationInfo>,
    pub assignee_candidates: Vec<AssigneeCandidate>,
    #[ts(type = "number")]
    pub elapsed_ms: u64,
    #[ts(type = "number")]
    pub com_calls: u64,
}

impl Session {
    pub fn inspect_sheet(&self, workbook_id: &str, sheet_name: &str) -> Result<SheetInspection> {
        let started = Instant::now();
        let calls_before = com_calls();
        let book = self.workbook(workbook_id)?;
        let ws = self.worksheet(workbook_id, sheet_name)?;
        let used = used_range(&ws)?;
        check_size(used)?;
        let sheet = Sheet { ws: &ws, origin: used.top_left() };
        let bounds = CellRange::new(0, 0, used.rows, used.cols);

        let whole = sheet.range(bounds)?;
        let mut cells = read_values(&whole, used)?;
        read_formulas(&whole, used.cols, &mut cells)?;
        let mut probe = SheetModel {
            workbook: String::new(),
            sheet: sheet_name.to_string(),
            origin: used.top_left(),
            rows: used.rows,
            cols: used.cols,
            cells,
            styles: vec![CellStyle::default()],
            merges: Vec::new(),
            col_widths: Vec::new(),
            row_heights: Vec::new(),
            header_rows: 0,
            frozen_rows: 0,
            frozen_cols: 0,
            validations: Vec::new(),
            hidden_rows: Vec::new(),
            hidden_cols: Vec::new(),
            added_rows: Vec::new(),
            cancelled_rows: Vec::new(),
            layout: 0,
        };
        let head = CellRange::new(0, 0, used.rows.min(HEAD_SCAN_ROWS), used.cols);
        let mut no_progress = |_| true;
        probe.merges = read_merges(&sheet, head, &mut Reporter::new(&mut no_progress, head.len()))?;
        read_bold(&sheet, head, &mut probe)?;

        let guess = detect_table(&probe);
        let labels = column_labels(&probe, &guess);
        let to_sheet_col = |c: u32| c + used.col;
        let columns = (guess.range.col..=guess.range.last_col())
            .zip(&labels)
            .map(|(c, label)| ColumnInfo { col: to_sheet_col(c), label: label.clone() })
            .collect();
        let label_of = |c: u32| {
            let i = c.checked_sub(guess.range.col)? as usize;
            labels.get(i).cloned()
        };

        let formula_cols = formula_columns(&probe, &guess)
            .into_iter()
            .map(|f| ColumnCount { col: to_sheet_col(f.col), count: f.count })
            .collect();
        let assignees = assignee_candidates(&probe, &guess)
            .into_iter()
            .map(|a| AssigneeCandidate { col: to_sheet_col(a.col), ..a })
            .collect();

        let table_sheet = sheet.absolute(guess.range);
        let validations = read_validations(&sheet, guess.range)?
            .into_iter()
            .map(|v| ValidationInfo {
                label: label_of(v.range.col).unwrap_or_default(),
                range: sheet.absolute(v.range),
                rule: v.rule,
            })
            .collect();
        let dims = read_dimensions(&sheet, guess.range)?;
        let (hidden_rows, hidden_cols) = (dims.hidden_rows(), dims.hidden_cols());
        let (frozen_rows, frozen_cols) = read_freeze(book, &ws, sheet_name)?;

        Ok(SheetInspection {
            used_range: used,
            table: table_sheet,
            header_rows: guess.header_rows,
            frozen_rows,
            frozen_cols,
            hidden_rows: hidden_rows.len() as u32,
            hidden_cols: hidden_cols.len() as u32,
            columns,
            formula_columns: formula_cols,
            validations,
            assignee_candidates: assignees,
            elapsed_ms: started.elapsed().as_millis() as u64,
            com_calls: com_calls() - calls_before,
        })
    }
}
