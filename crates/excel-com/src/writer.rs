//! 5단계 엑셀 저장: 저장 계획([`ExportPlan`])을 Excel 시트에 쓴다.
//!
//! - [`Session::save_into`]: 원본 통합문서에 쓰고 저장한다.
//! - [`Session::save_copy`]: 원본을 `SaveCopyAs`로 복사해 **별도의 숨은 Excel**에서 열고, 거기에 쓰고 저장한다(원본은 그대로).
//!
//! 쓰는 동안 화면 갱신·이벤트·자동 계산·경고를 끄고 끝나면 되돌린다. 값은 열마다 이어진 칸을 한 번에(`Value2` 배열) 쓰고,
//! 저장한 뒤 다시 읽어 맞는지 확인한다. 수식 칸은 쓰지 않는다(계획에 들어 있지 않다).
//!
//! 참여자가 표 아래에 붙인 행이 있으면 값을 쓰기 전에 그 자리에 행을 끼워 넣는다(아래 내용은 밀려 내려가고, 새 행은
//! 위 행의 서식을 따른다). 위 행의 수식은 새 행까지 채워 내린다.

use std::path::Path;

use collect_core::export::{ExportPlan, HistoryTable, RowInsert, ValueBlock};
use collect_core::{CellRange, CellRef, CellStyle, CellValue, HAlign, SheetCell};
use serde::Serialize;
use ts_rs::TS;

use crate::calc::{MSO_AUTOMATION_SECURITY_FORCE_DISABLE, XL_CALCULATION_MANUAL};
use crate::dispatch::Dispatch;
use crate::error::{ExcelError, Result};
use crate::reader::{XL_EDGE_BOTTOM, XL_EDGE_LEFT, XL_EDGE_RIGHT, XL_EDGE_TOP, XL_INSIDE_HORIZONTAL, XL_NONE};
use crate::session::{Session, create_excel};
use crate::variant::Var;

const XL_UNDERLINE_SINGLE: i32 = 2;
pub const HISTORY_SHEET: &str = "변경 이력";

/// 저장할 원본 통합문서의 상태.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SourceStatus {
    pub id: String,
    pub name: String,
    /// 저장 폴더. 저장한 적 없는 새 문서면 빈 문자열.
    pub folder: String,
    /// Excel에 저장하지 않은 변경이 없다.
    pub saved: bool,
    pub read_only: bool,
    /// 앱이 띄운 숨은 Excel에서 연 문서.
    pub background: bool,
}

#[derive(Debug, Clone, Default, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SaveReport {
    /// 저장한 파일 전체 경로.
    pub path: String,
    /// 값을 쓴 칸 수.
    pub written: u32,
    /// 저장한 뒤 다시 읽었더니 다른 칸(시트 순번 = 넘긴 계획 순서, 모델 좌표).
    pub mismatched: Vec<SheetCell>,
    /// 서식을 바꾼 칸 수.
    pub styled: u32,
    pub merges: u32,
    /// 크기(열 너비·행 높이)를 바꾼 열·행 수.
    pub resized: u32,
    /// 표 아래에 끼워 넣은 행 수(참여자가 추가한 행).
    pub inserted: u32,
    /// 더한 '변경 이력' 시트 이름.
    #[ts(optional)]
    pub history_sheet: Option<String>,
    #[ts(type = "number")]
    pub ms: u64,
}

/// `0xRRGGBB` → Excel `0x00BBGGRR`.
fn rgb_to_bgr(rgb: u32) -> f64 {
    (((rgb & 0xFF) << 16) | (rgb & 0xFF00) | ((rgb >> 16) & 0xFF)) as f64
}

/// 칸에 쓸 값. `literal`이면 글자 앞에 `'`를 붙여 Excel이 숫자·날짜·수식으로 바꾸지 않게 한다.
pub(crate) fn write_var(v: &CellValue, literal: bool) -> Var {
    match v {
        CellValue::Number(n) => Var::f64(*n),
        CellValue::Text(s) if s.is_empty() => Var::empty(),
        CellValue::Text(s) if literal => Var::str(&format!("'{s}")),
        CellValue::Text(s) => Var::str(s),
        CellValue::Bool(b) => Var::bool(*b),
        CellValue::Empty | CellValue::Error { .. } => Var::empty(),
    }
}

/// 쓰는 동안 화면 갱신·이벤트·자동 계산·경고를 끄고, 끝나면(실패해도) 되돌린다.
struct Suspend<'a> {
    app: &'a Dispatch,
    saved: Vec<(&'static str, Var)>,
}

impl<'a> Suspend<'a> {
    fn new(app: &'a Dispatch) -> Self {
        let mut saved = Vec::new();
        for (name, off) in [
            ("ScreenUpdating", Var::bool(false)),
            ("EnableEvents", Var::bool(false)),
            ("DisplayAlerts", Var::bool(false)),
            ("Calculation", Var::i32(XL_CALCULATION_MANUAL)),
        ] {
            if let Ok(before) = app.get(name)
                && app.put(name, off).is_ok()
            {
                saved.push((name, before));
            }
        }
        Self { app, saved }
    }
}

impl Drop for Suspend<'_> {
    fn drop(&mut self) {
        // 계산 모드를 먼저 되돌려 저장 전에 수식이 다시 계산되게 한다.
        while let Some((name, v)) = self.saved.pop() {
            let _ = self.app.put(name, v);
        }
    }
}

struct Target<'a> {
    ws: &'a Dispatch,
    origin: CellRef,
}

impl Target<'_> {
    fn abs(&self, r: CellRange) -> CellRange {
        CellRange::new(r.row + self.origin.row, r.col + self.origin.col, r.rows, r.cols)
    }

    fn range(&self, r: CellRange) -> Result<Dispatch> {
        self.ws.obj_call("Range", vec![Var::str(&self.abs(r).to_string())])
    }
}

/// 열 너비·행 높이(pt). 행은 `RowHeight`(pt)로 바로 쓴다. 열은 글자 단위(`ColumnWidth`)로만 쓸 수 있어
/// 지금 `Width`(pt)와 `ColumnWidth`의 비로 바꿔 쓰고, `Width`를 다시 읽어 맞을 때까지 몇 번 고친다(여백 때문에 비례하지 않는다).
fn apply_sizes(t: &Target, plan: &ExportPlan) -> Result<()> {
    for &(row, pt) in &plan.row_heights {
        t.range(CellRange::new(row, 0, 1, 1))?.obj("EntireRow")?.put("RowHeight", Var::f64(f64::from(pt)))?;
    }
    for &(col, pt) in &plan.col_widths {
        let column = t.range(CellRange::new(0, col, 1, 1))?.obj("EntireColumn")?;
        let want = f64::from(pt);
        for _ in 0..3 {
            let width = column.get("Width")?.as_f64().unwrap_or(0.0);
            let chars = column.get("ColumnWidth")?.as_f64().unwrap_or(0.0);
            if width <= 0.0 || chars <= 0.0 || (width - want).abs() < 0.75 {
                break;
            }
            column.put("ColumnWidth", Var::f64((chars * want / width).clamp(0.1, 255.0)))?;
        }
    }
    Ok(())
}

/// 표 아래(`at`)에 행을 끼워 넣고, 높이를 맞추고, 위 행의 수식을 새 행까지 채워 내린다.
fn insert_rows(t: &Target, ins: &RowInsert) -> Result<()> {
    // Insert(Shift, CopyOrigin)의 기본값: 아래로 밀고, 서식은 위 행을 따른다.
    t.range(CellRange::new(ins.at, 0, ins.count, 1))?.obj("EntireRow")?.call("Insert", vec![])?;
    for (k, h) in ins.heights.iter().enumerate() {
        let row = t.range(CellRange::new(ins.at + k as u32, 0, 1, 1))?.obj("EntireRow")?;
        row.put("RowHeight", Var::f64(f64::from(*h)))?;
    }
    if ins.at > 0 {
        for &c in &ins.formula_cols {
            t.range(CellRange::new(ins.at - 1, c, ins.count + 1, 1))?.call("FillDown", vec![])?;
        }
    }
    Ok(())
}

fn write_block(t: &Target, b: &ValueBlock) -> Result<()> {
    let rng = t.range(b.range)?;
    let value = if b.values.len() == 1 {
        write_var(&b.values[0].value, b.values[0].literal)
    } else {
        Var::array2d(b.values.len(), 1, |r, _| write_var(&b.values[r].value, b.values[r].literal))?
    };
    rng.put("Value2", value)
}

fn apply_style(rng: &Dispatch, range: CellRange, s: &CellStyle) -> Result<()> {
    let font = rng.obj("Font")?;
    if !s.font_name.is_empty() {
        font.put("Name", Var::str(&s.font_name))?;
    }
    if s.font_size > 0.0 {
        font.put("Size", Var::f64(f64::from(s.font_size)))?;
    }
    font.put("Bold", Var::bool(s.bold))?;
    font.put("Italic", Var::bool(s.italic))?;
    font.put("Underline", Var::i32(if s.underline { XL_UNDERLINE_SINGLE } else { XL_NONE }))?;
    font.put("Color", Var::f64(rgb_to_bgr(s.color)))?;
    let interior = rng.obj("Interior")?;
    match s.fill {
        Some(rgb) => interior.put("Color", Var::f64(rgb_to_bgr(rgb)))?,
        None => interior.put("Pattern", Var::i32(XL_NONE))?,
    }
    let h = match s.h_align {
        HAlign::General => 1,
        HAlign::Left => -4131,
        HAlign::Center => -4108,
        HAlign::Right => -4152,
    };
    rng.put("HorizontalAlignment", Var::i32(h))?;
    let borders = rng.obj("Borders")?;
    let mut edges = vec![
        (XL_EDGE_TOP, s.borders.top),
        (XL_EDGE_BOTTOM, s.borders.bottom),
        (XL_EDGE_LEFT, s.borders.left),
        (XL_EDGE_RIGHT, s.borders.right),
    ];
    if range.rows > 1 {
        // 같은 서식이 위아래로 이어진 칸: 칸 사이 선은 각 칸의 아래 선과 같다.
        edges.push((XL_INSIDE_HORIZONTAL, s.borders.bottom));
    }
    for (index, edge) in edges {
        let b = borders.obj_call("Item", vec![Var::i32(index)])?;
        match edge {
            Some(e) => {
                b.put("LineStyle", Var::i32(e.style))?;
                b.put("Weight", Var::i32(e.weight))?;
                b.put("Color", Var::f64(rgb_to_bgr(e.color)))?;
            }
            None => b.put("LineStyle", Var::i32(XL_NONE))?,
        }
    }
    Ok(())
}

/// 이름이 겹치지 않는 시트 이름(`변경 이력`, `변경 이력 2` …).
fn unique_sheet_name(book: &Dispatch, base: &str) -> Result<String> {
    let sheets = book.obj("Worksheets")?;
    let n = sheets.get("Count")?.as_i32().unwrap_or(0);
    let mut names = Vec::new();
    for i in 1..=n {
        names.push(sheets.obj_call("Item", vec![Var::i32(i)])?.get_string("Name")?.to_lowercase());
    }
    let mut name = base.to_string();
    let mut k = 2;
    while names.contains(&name.to_lowercase()) {
        name = format!("{base} {k}");
        k += 1;
    }
    Ok(name)
}

fn add_history(book: &Dispatch, table: &HistoryTable) -> Result<String> {
    let name = unique_sheet_name(book, HISTORY_SHEET)?;
    let before = book.obj("ActiveSheet").ok();
    let sheets = book.obj("Worksheets")?;
    let count = sheets.get("Count")?.as_i32().unwrap_or(1);
    let last = sheets.obj_call("Item", vec![Var::i32(count)])?;
    let ws = sheets.obj_call("Add", vec![Var::missing(), Var::dispatch(last.raw())])?;
    ws.put("Name", Var::str(&name))?;
    let rows = table.rows.len() + 1;
    let cols = table.header.len();
    let data = Var::array2d(rows, cols, |r, c| {
        if r == 0 { Var::str(&table.header[c]) } else { write_var(&table.rows[r - 1][c], true) }
    })?;
    let all = CellRange::new(0, 0, rows as u32, cols as u32);
    ws.obj_call("Range", vec![Var::str(&all.to_string())])?.put("Value2", data)?;
    let head = CellRange::new(0, 0, 1, cols as u32);
    ws.obj_call("Range", vec![Var::str(&head.to_string())])?.obj("Font")?.put("Bold", Var::bool(true))?;
    if rows > 1 {
        let time = CellRange::new(1, table.time_col as u32, rows as u32 - 1, 1);
        ws.obj_call("Range", vec![Var::str(&time.to_string())])?
            .put("NumberFormat", Var::str("yyyy-mm-dd hh:mm:ss"))?;
    }
    let _ = ws.obj_call("Range", vec![Var::str(&all.to_string())])?.obj("Columns")?.call("AutoFit", vec![]);
    // 시트를 더하면 그 시트가 활성화된다: 보던 시트로 돌아간다.
    if let Some(sheet) = before {
        let _ = sheet.call("Activate", vec![]);
    }
    Ok(name)
}

/// 계획 하나를 그 시트에 쓴다(저장은 하지 않는다. '변경 이력' 시트는 [`apply_all`]이 한 번 더한다).
fn apply_sheet(ws: &Dispatch, plan: &ExportPlan, report: &mut SaveReport) -> Result<()> {
    let t = Target { ws, origin: plan.origin };
    if let Some(ins) = &plan.insert {
        insert_rows(&t, ins)?;
        report.inserted += ins.count;
    }
    for m in &plan.merges_remove {
        t.range(*m)?.call("UnMerge", vec![])?;
    }
    for b in &plan.blocks {
        write_block(&t, b)?;
        report.written += b.values.len() as u32;
    }
    for m in &plan.merges_add {
        t.range(*m)?.call("Merge", vec![])?;
    }
    report.merges += (plan.merges_add.len() + plan.merges_remove.len()) as u32;
    for s in &plan.styles {
        apply_style(&t.range(s.range)?, s.range, &s.style)?;
        report.styled += s.range.len() as u32;
    }
    apply_sizes(&t, plan)?;
    report.resized += plan.resized();
    if let Some(rgb) = plan.mark {
        for b in &plan.blocks {
            t.range(b.range)?.obj("Interior")?.put("Color", Var::f64(rgb_to_bgr(rgb)))?;
        }
    }
    Ok(())
}

/// 계획들을 시트마다 쓰고, 계획에 든 '변경 이력' 표(처음 것 하나)를 시트로 더한다(저장은 하지 않는다).
fn apply_all(book: &Dispatch, sheets: &[(Dispatch, &ExportPlan)], report: &mut SaveReport) -> Result<()> {
    let app = book.obj("Application")?;
    let _suspend = Suspend::new(&app);
    for (ws, plan) in sheets {
        apply_sheet(ws, plan, report)?;
    }
    if let Some(table) = sheets.iter().find_map(|(_, p)| p.history.as_ref()) {
        report.history_sheet = Some(add_history(book, table)?);
    }
    Ok(())
}

/// 저장한 뒤 쓴 칸을 다시 읽어 확인한다.
fn verify(sheet: u32, ws: &Dispatch, plan: &ExportPlan) -> Result<Vec<SheetCell>> {
    let t = Target { ws, origin: plan.origin };
    let mut bad = Vec::new();
    for b in &plan.blocks {
        let grid = t.range(b.range)?.get("Value2")?.into_grid()?;
        for (i, want) in b.values.iter().enumerate() {
            let got = grid.value(i, 0);
            let want = match &want.value {
                CellValue::Error { .. } => CellValue::Empty,
                v => v.clone(),
            };
            if got != want {
                bad.push(SheetCell::new(sheet, CellRef::new(b.range.row + i as u32, b.range.col)));
            }
        }
    }
    Ok(bad)
}

fn sheet_of(book: &Dispatch, sheet: &str) -> Result<Dispatch> {
    book.obj("Worksheets")?
        .obj_call("Item", vec![Var::str(sheet)])
        .map_err(|_| ExcelError::SheetNotFound(sheet.to_string()))
}

/// 쓸 시트를 모두 먼저 찾는다(없는 시트가 있으면 아무것도 쓰지 않는다).
fn sheets_of<'a>(book: &Dispatch, plans: &'a [(String, ExportPlan)]) -> Result<Vec<(Dispatch, &'a ExportPlan)>> {
    plans.iter().map(|(name, plan)| Ok((sheet_of(book, name)?, plan))).collect()
}

/// 쓰고 저장한 뒤 시트마다 다시 읽어 확인한다.
fn write_and_save(book: &Dispatch, plans: &[(String, ExportPlan)], report: &mut SaveReport) -> Result<()> {
    let sheets = sheets_of(book, plans)?;
    apply_all(book, &sheets, report)?;
    book.call("Save", vec![])?;
    for (i, (ws, plan)) in sheets.iter().enumerate() {
        report.mismatched.extend(verify(i as u32, ws, plan)?);
    }
    Ok(())
}

fn extension(name: &str) -> String {
    Path::new(name).extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

impl Session {
    /// 원본이 지금 열려 있으면 그 상태(목록을 새로 고친다. 닫혔어도 다시 열지는 않는다).
    pub fn peek(&mut self, workbook_id: &str) -> Result<Option<SourceStatus>> {
        let status = self.list_workbooks()?;
        let Some(info) = status.workbooks.into_iter().find(|b| b.id == workbook_id) else { return Ok(None) };
        let book = self.workbook(workbook_id)?;
        Ok(Some(SourceStatus {
            id: info.id,
            name: info.name,
            folder: info.path,
            saved: book.get("Saved").ok().and_then(|v| v.as_bool()).unwrap_or(true),
            read_only: book.get("ReadOnly").ok().and_then(|v| v.as_bool()).unwrap_or(false),
            background: info.background,
        }))
    }

    /// 저장할 원본을 찾는다. 닫혔으면 파일을 다시 연다(실행 중인 Excel이 있으면 거기서, 없으면 숨은 Excel에서).
    pub fn locate(&mut self, workbook_id: &str) -> Result<SourceStatus> {
        if let Some(status) = self.peek(workbook_id)? {
            return Ok(status);
        }
        let path = Path::new(workbook_id);
        if !(path.is_absolute() && path.exists()) {
            return Err(ExcelError::WorkbookNotFound(workbook_id.to_string()));
        }
        self.open_file(path)?;
        self.peek(workbook_id)?.ok_or_else(|| ExcelError::WorkbookNotFound(workbook_id.to_string()))
    }

    /// 불러온 범위(`origin`부터 `rows × cols`)의 지금 값(행 우선).
    pub fn read_range_values(
        &self,
        workbook_id: &str,
        sheet: &str,
        origin: CellRef,
        rows: u32,
        cols: u32,
    ) -> Result<Vec<CellValue>> {
        let ws = self.worksheet(workbook_id, sheet)?;
        let range = CellRange::new(origin.row, origin.col, rows, cols);
        let grid = ws.obj_call("Range", vec![Var::str(&range.to_string())])?.get("Value2")?.into_grid()?;
        let mut out = Vec::with_capacity((rows * cols) as usize);
        for r in 0..rows as usize {
            for c in 0..cols as usize {
                out.push(grid.value(r, c));
            }
        }
        Ok(out)
    }

    /// 원본 통합문서의 한 시트에 쓰고 저장한다. [`Session::save_into_multi`] 참고.
    pub fn save_into(&mut self, workbook_id: &str, sheet: &str, plan: &ExportPlan) -> Result<SaveReport> {
        self.save_into_multi(workbook_id, &[(sheet.to_string(), plan.clone())])
    }

    /// 원본 통합문서의 시트들(이름, 계획)에 쓰고 한 번 저장한다. '변경 이력' 시트는 하나만 더한다.
    pub fn save_into_multi(&mut self, workbook_id: &str, plans: &[(String, ExportPlan)]) -> Result<SaveReport> {
        let started = std::time::Instant::now();
        let book = self.workbook(workbook_id)?.clone();
        if book.get_string("Path").unwrap_or_default().is_empty() {
            return Err(ExcelError::Type(
                "원본이 아직 저장한 적 없는 새 문서입니다. '다른 이름으로 저장'을 골라 주세요".into(),
            ));
        }
        if book.get("ReadOnly").ok().and_then(|v| v.as_bool()).unwrap_or(false) {
            return Err(ExcelError::Type(
                "원본이 읽기 전용으로 열려 있습니다. '다른 이름으로 저장'을 골라 주세요".into(),
            ));
        }
        let mut report = SaveReport { path: book.get_string("FullName")?, ..Default::default() };
        write_and_save(&book, plans, &mut report)?;
        report.ms = started.elapsed().as_millis() as u64;
        Ok(report)
    }

    /// 원본의 지금 상태를 `path`로 복사하고 한 시트에 쓴다. [`Session::save_copy_multi`] 참고.
    pub fn save_copy(&mut self, workbook_id: &str, sheet: &str, plan: &ExportPlan, path: &Path) -> Result<SaveReport> {
        self.save_copy_multi(workbook_id, &[(sheet.to_string(), plan.clone())], path)
    }

    /// 원본의 지금 상태를 `path`로 복사하고, 숨은 Excel에서 그 복사본의 시트들에 쓰고 저장한다. 원본은 그대로 둔다.
    /// 실패하면 만들다 만 파일은 지운다.
    pub fn save_copy_multi(
        &mut self,
        workbook_id: &str,
        plans: &[(String, ExportPlan)],
        path: &Path,
    ) -> Result<SaveReport> {
        let started = std::time::Instant::now();
        let source = self.workbook(workbook_id)?.clone();
        let source_ext = extension(&source.get_string("Name")?);
        let ext = extension(&path.to_string_lossy());
        let want = if source_ext.is_empty() { "xlsx".to_string() } else { source_ext };
        if ext != want {
            return Err(ExcelError::Type(format!("원본과 같은 형식(.{want})으로 저장해야 합니다")));
        }
        let full = std::path::absolute(path).map_err(|e| ExcelError::Type(e.to_string()))?;
        let full_text = full.to_string_lossy().to_string();
        if full_text.eq_ignore_ascii_case(workbook_id) {
            return Err(ExcelError::Type(
                "원본과 다른 파일 이름을 골라 주세요(원본에 반영하려면 '원본 통합문서에 반영')".into(),
            ));
        }
        source.call("SaveCopyAs", vec![Var::str(&full_text)])?;

        let result = (|| -> Result<SaveReport> {
            let app = create_excel()?;
            let run = (|| -> Result<SaveReport> {
                app.put("Visible", Var::bool(false))?;
                app.put("DisplayAlerts", Var::bool(false))?;
                let _ = app.put("AutomationSecurity", Var::i32(MSO_AUTOMATION_SECURITY_FORCE_DISABLE));
                let book = app.obj("Workbooks")?.obj_call(
                    "Open",
                    vec![Var::str(&full_text), Var::i32(0) /* UpdateLinks: 안 함 */, Var::bool(false)],
                )?;
                let saved = (|| -> Result<SaveReport> {
                    let mut report = SaveReport { path: full_text.clone(), ..Default::default() };
                    write_and_save(&book, plans, &mut report)?;
                    Ok(report)
                })();
                let _ = book.call("Close", vec![Var::bool(false)]);
                saved
            })();
            let _ = app.call("Quit", vec![]);
            run
        })();
        match result {
            Ok(mut report) => {
                report.ms = started.elapsed().as_millis() as u64;
                Ok(report)
            }
            Err(e) => {
                let _ = std::fs::remove_file(&full);
                Err(e)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_for_excel() {
        assert_eq!(write_var(&CellValue::Text("=1+1".into()), true).as_string().as_deref(), Some("'=1+1"));
        assert_eq!(write_var(&CellValue::Text("0012".into()), false).as_string().as_deref(), Some("0012"));
        assert_eq!(write_var(&CellValue::Number(9800.0), true).as_f64(), Some(9800.0));
        assert!(write_var(&CellValue::Empty, true).is_empty());
        assert!(write_var(&CellValue::Error { err: 2042 }, false).is_empty());
    }

    #[test]
    fn bgr() {
        assert_eq!(rgb_to_bgr(0xFF0000), 255.0);
        assert_eq!(rgb_to_bgr(0xFFF3B0), f64::from(0xB0F3FFu32));
    }
}
