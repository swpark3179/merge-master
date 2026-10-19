//! 시트 읽기: Excel 범위 → [`SheetModel`].
//!
//! 값·수식은 범위 전체를 한 번에 읽는다(호출 1~2회). 서식·병합·행 높이·숨김은 셀마다 읽으면 호출이 수만 번이
//! 되므로 **분할 정복**한다: 블록 전체의 속성을 물어서 Excel이 `Null`(셀마다 다름)을 돌려주면 블록을 반으로
//! 나눠 다시 묻고, 값이 오면 블록 전체에 그 값을 적용한다.

use std::time::{Duration, Instant};

use collect_core::detect::detect_header_rows;
use collect_core::{
    BorderEdge, Borders, Cell, CellRange, CellRef, CellStyle, CellValue, HAlign, SheetModel, VAlign, Validation,
    ValidationRule,
};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::dispatch::{Dispatch, com_calls};
use crate::error::{ExcelError, Result};
use crate::session::Session;
use crate::variant::Var;

pub(crate) const XL_NONE: i32 = -4142;
const XL_CELL_TYPE_ALL_VALIDATION: i32 = -4174;
const XL_VALIDATE_LIST: i32 = 3;
pub(crate) const XL_EDGE_LEFT: i32 = 7;
pub(crate) const XL_EDGE_TOP: i32 = 8;
pub(crate) const XL_EDGE_BOTTOM: i32 = 9;
pub(crate) const XL_EDGE_RIGHT: i32 = 10;
const XL_INSIDE_VERTICAL: i32 = 11;
pub(crate) const XL_INSIDE_HORIZONTAL: i32 = 12;
/// `Application.International` 인덱스: 현지화된 '일반' 표시 형식 이름(한국어 Excel은 `G/표준`).
const XL_GENERAL_FORMAT_NAME: i32 = 26;
/// 한 번에 읽을 수 있는 최대 셀 수. 값 배열 하나가 이 크기를 넘으면 Excel·메모리 모두 버겁다.
pub const MAX_CELLS: u64 = 1_000_000;
/// 숨긴 행·열을 포함해 불러올 때 쓸 기본 크기(pt). 숨긴 행·열은 Excel이 크기를 0으로 알려 준다.
const DEFAULT_COL_PT: f32 = 48.0;
const DEFAULT_ROW_PT: f32 = 15.0;

/// 1a '가져올 항목'. 값과 표시 형식·맞춤은 항상 읽는다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct ImportItems {
    /// 수식 원문(참여자에게는 결과 값만 보인다).
    pub formulas: bool,
    pub merges: bool,
    /// 글꼴·글자색(굵게·기울임·밑줄 포함).
    pub fonts: bool,
    pub borders: bool,
    /// 배경색.
    pub fills: bool,
    /// 열 너비·행 높이.
    pub dimensions: bool,
    /// 데이터 유효성 → 입력 형식.
    pub validations: bool,
    /// 숨긴 행·열도 보이는 행·열로 가져온다.
    pub include_hidden: bool,
}

impl Default for ImportItems {
    fn default() -> Self {
        Self {
            formulas: true,
            merges: true,
            fonts: true,
            borders: true,
            fills: true,
            dimensions: true,
            validations: true,
            include_hidden: false,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct ReadOptions {
    /// 읽을 범위(A1 표기). `None`이면 `UsedRange`.
    #[ts(optional)]
    pub range: Option<String>,
    /// 머리글 행 수. `None`이면 자동 감지.
    #[ts(optional)]
    pub header_rows: Option<u32>,
    /// 틀 고정 열 수(범위 기준). `None`이면 Excel 창의 틀 고정을 따른다.
    #[ts(optional)]
    pub frozen_cols: Option<u32>,
    pub items: ImportItems,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ReadStage {
    Values,
    Formulas,
    Merges,
    Styles,
    Dimensions,
    Validations,
    Done,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct ReadProgress {
    pub stage: ReadStage,
    /// 단계 안에서 처리한 셀 수.
    #[ts(type = "number")]
    pub done: u64,
    #[ts(type = "number")]
    pub total: u64,
}

/// 단계별 소요 시간(ms)과 Excel 호출 수.
#[derive(Debug, Clone, Default, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ReadStats {
    #[ts(type = "number")]
    pub values_ms: u64,
    #[ts(type = "number")]
    pub formulas_ms: u64,
    #[ts(type = "number")]
    pub merges_ms: u64,
    #[ts(type = "number")]
    pub styles_ms: u64,
    #[ts(type = "number")]
    pub dimensions_ms: u64,
    #[ts(type = "number")]
    pub validations_ms: u64,
    #[ts(type = "number")]
    pub total_ms: u64,
    #[ts(type = "number")]
    pub com_calls: u64,
    /// 서식을 물어본 블록 수 / 서식이 같은 것으로 확정된 영역 수.
    #[ts(type = "number")]
    pub style_probes: u64,
    #[ts(type = "number")]
    pub style_regions: u64,
}

/// 진행률 콜백을 너무 자주 부르지 않게 묶고, 콜백이 `false`를 돌려주면 [`ExcelError::Cancelled`]로 멈춘다.
pub(crate) struct Reporter<'a> {
    callback: &'a mut dyn FnMut(ReadProgress) -> bool,
    stage: ReadStage,
    total: u64,
    last: Instant,
}

impl<'a> Reporter<'a> {
    const INTERVAL: Duration = Duration::from_millis(80);

    pub(crate) fn new(callback: &'a mut dyn FnMut(ReadProgress) -> bool, total: u64) -> Self {
        Self { callback, stage: ReadStage::Values, total, last: Instant::now() }
    }

    pub(crate) fn stage(&mut self, stage: ReadStage) -> Result<()> {
        self.stage = stage;
        self.emit(if stage == ReadStage::Done { self.total } else { 0 })
    }

    pub(crate) fn tick(&mut self, done: u64) -> Result<()> {
        if self.last.elapsed() >= Self::INTERVAL { self.emit(done) } else { Ok(()) }
    }

    fn emit(&mut self, done: u64) -> Result<()> {
        self.last = Instant::now();
        if (self.callback)(ReadProgress { stage: self.stage, done, total: self.total }) {
            Ok(())
        } else {
            Err(ExcelError::Cancelled)
        }
    }
}

impl Session {
    /// 시트를 읽어 [`SheetModel`]을 만든다. `progress`가 `false`를 돌려주면 취소한다.
    pub fn read_sheet(
        &self,
        workbook_id: &str,
        sheet_name: &str,
        opts: &ReadOptions,
        progress: &mut dyn FnMut(ReadProgress) -> bool,
    ) -> Result<(SheetModel, ReadStats)> {
        let started = Instant::now();
        let calls_before = com_calls();
        let book = self.workbook(workbook_id)?;
        let ws = self.worksheet(workbook_id, sheet_name)?;
        let used = match &opts.range {
            Some(a) => CellRange::parse(a)?,
            None => used_range(&ws)?,
        };
        check_size(used)?;
        let sheet = Sheet { ws: &ws, origin: used.top_left() };
        let bounds = CellRange::new(0, 0, used.rows, used.cols);
        let items = opts.items;
        let mut rep = Reporter::new(progress, bounds.len());
        let mut stats = ReadStats::default();

        let t = Instant::now();
        rep.stage(ReadStage::Values)?;
        let whole = sheet.range(bounds)?;
        let mut cells = read_values(&whole, used)?;
        stats.values_ms = ms(t);

        if items.formulas {
            let t = Instant::now();
            rep.stage(ReadStage::Formulas)?;
            read_formulas(&whole, used.cols, &mut cells)?;
            stats.formulas_ms = ms(t);
        }

        let mut model = SheetModel {
            workbook: book.get_string("Name")?,
            sheet: sheet_name.to_string(),
            origin: used.top_left(),
            rows: used.rows,
            cols: used.cols,
            cells,
            styles: vec![CellStyle::default()],
            merges: Vec::new(),
            col_widths: vec![DEFAULT_COL_PT; used.cols as usize],
            row_heights: vec![DEFAULT_ROW_PT; used.rows as usize],
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

        if items.merges {
            let t = Instant::now();
            rep.stage(ReadStage::Merges)?;
            model.merges = read_merges(&sheet, bounds, &mut rep)?;
            stats.merges_ms = ms(t);
        }

        let t = Instant::now();
        rep.stage(ReadStage::Styles)?;
        let general = ws.obj("Application")?.call("International", vec![Var::i32(XL_GENERAL_FORMAT_NAME)])?.as_string();
        let probe = Probe { items, general_format: general.as_deref() };
        read_styles(&sheet, bounds, &probe, &mut model, &mut stats, &mut rep)?;
        stats.styles_ms = ms(t);

        let t = Instant::now();
        rep.stage(ReadStage::Dimensions)?;
        let dims = read_dimensions(&sheet, bounds)?;
        let (hidden_rows, hidden_cols) = (dims.hidden_rows(), dims.hidden_cols());
        if items.dimensions {
            model.col_widths = dims.col_widths;
            model.row_heights = dims.row_heights;
        }
        for &r in &hidden_rows {
            // 숨긴 행·열을 포함하면 기본 크기로 펼치고, 아니면 크기 0(그리드에서 보이지 않음)으로 둔다.
            model.row_heights[r as usize] = if items.include_hidden { DEFAULT_ROW_PT } else { 0.0 };
        }
        for &c in &hidden_cols {
            model.col_widths[c as usize] = if items.include_hidden { DEFAULT_COL_PT } else { 0.0 };
        }
        if !items.include_hidden {
            model.hidden_rows = hidden_rows;
            model.hidden_cols = hidden_cols;
        }
        let (frozen_rows, frozen_cols) = read_freeze(book, &ws, sheet_name)?;
        model.frozen_rows = frozen_rows.saturating_sub(used.row).min(used.rows);
        model.frozen_cols = opts.frozen_cols.unwrap_or(frozen_cols.saturating_sub(used.col)).min(used.cols);
        stats.dimensions_ms = ms(t);

        if items.validations {
            let t = Instant::now();
            rep.stage(ReadStage::Validations)?;
            model.validations = read_validations(&sheet, bounds)?;
            stats.validations_ms = ms(t);
        }

        model.header_rows = opts.header_rows.map_or_else(|| detect_header_rows(&model), |h| h.min(used.rows));
        stats.total_ms = ms(started);
        stats.com_calls = com_calls() - calls_before;
        rep.stage(ReadStage::Done)?;
        Ok((model, stats))
    }
}

fn ms(t: Instant) -> u64 {
    t.elapsed().as_millis() as u64
}

pub(crate) fn used_range(ws: &Dispatch) -> Result<CellRange> {
    Ok(CellRange::parse(&ws.obj("UsedRange")?.get_string("Address")?)?)
}

pub(crate) fn check_size(range: CellRange) -> Result<()> {
    if range.len() > MAX_CELLS {
        return Err(ExcelError::TooLarge { cells: range.len(), limit: MAX_CELLS });
    }
    Ok(())
}

/// 불러온 범위 기준 상대 좌표로 Excel `Range`를 만든다.
pub(crate) struct Sheet<'a> {
    pub ws: &'a Dispatch,
    pub origin: CellRef,
}

impl Sheet<'_> {
    pub fn absolute(&self, rel: CellRange) -> CellRange {
        CellRange::new(rel.row + self.origin.row, rel.col + self.origin.col, rel.rows, rel.cols)
    }

    pub fn range(&self, rel: CellRange) -> Result<Dispatch> {
        self.ws.obj_call("Range", vec![Var::str(&self.absolute(rel).to_string())])
    }
}

/// 열을 먼저 나누고(서식은 보통 열 단위로 같다), 한 열이 되면 행을 반으로 나눈다.
fn split(b: CellRange) -> [CellRange; 2] {
    if b.cols > 1 {
        let w = b.cols / 2;
        [CellRange::new(b.row, b.col, b.rows, w), CellRange::new(b.row, b.col + w, b.rows, b.cols - w)]
    } else {
        let h = b.rows / 2;
        [CellRange::new(b.row, b.col, h, 1), CellRange::new(b.row + h, b.col, b.rows - h, 1)]
    }
}

// ---------------------------------------------------------------- 값 · 수식

pub(crate) fn read_values(whole: &Dispatch, used: CellRange) -> Result<Vec<Cell>> {
    let values = whole.get("Value2")?.into_grid()?;
    if values.rows != used.rows as usize || values.cols != used.cols as usize {
        return Err(ExcelError::Type(format!("값 배열 크기 불일치 {}x{}", values.rows, values.cols)));
    }
    let mut cells = Vec::with_capacity(used.len() as usize);
    for r in 0..values.rows {
        for c in 0..values.cols {
            cells.push(Cell { v: values.value(r, c), f: None, s: 0 });
        }
    }
    Ok(cells)
}

pub(crate) fn read_formulas(whole: &Dispatch, cols: u32, cells: &mut [Cell]) -> Result<()> {
    // HasFormula: 하나도 없으면 False, 전부면 True, 섞이면 Null.
    if whole.get("HasFormula")?.as_bool() == Some(false) {
        return Ok(());
    }
    let formulas = whole.get("Formula")?.into_grid()?;
    for r in 0..formulas.rows {
        for c in 0..formulas.cols {
            if let Some(f) = formulas.string(r, c).filter(|s| s.starts_with('=')) {
                cells[r * cols as usize + c].f = Some(f);
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- 병합

pub(crate) fn read_merges(sheet: &Sheet, bounds: CellRange, rep: &mut Reporter) -> Result<Vec<CellRange>> {
    let abs_bounds = sheet.absolute(bounds);
    let cols = bounds.col + bounds.cols;
    let mut covered = vec![false; ((bounds.row + bounds.rows) * cols) as usize];
    let idx = |at: CellRef| (at.row * cols + at.col) as usize;
    let mut out = Vec::new();
    let mut done = 0u64;
    let mut stack = vec![bounds];
    while let Some(b) = stack.pop() {
        rep.tick(done)?;
        let m = sheet.range(b)?.get("MergeCells")?;
        if m.is_null() {
            if !b.is_single() {
                stack.extend(split(b));
            }
            continue;
        }
        done += b.len();
        if m.as_bool() != Some(true) {
            continue;
        }
        // 블록 전체가 병합 셀: 덮이지 않은 셀마다 MergeArea를 물어 영역을 확정한다.
        for at in b.cells() {
            if covered[idx(at)] {
                continue;
            }
            let addr = sheet.range(CellRange::cell(at))?.obj("MergeArea")?.get_string("Address")?;
            let area = CellRange::parse(&addr)?;
            let Some(rel) = area.intersect(&abs_bounds).and_then(|a| a.relative_to(sheet.origin)) else { continue };
            for c in rel.cells() {
                covered[idx(c)] = true;
            }
            if !rel.is_single() {
                out.push(rel);
            }
        }
    }
    out.sort_by_key(|m| (m.row, m.col));
    Ok(out)
}

// ---------------------------------------------------------------- 서식

/// 어떤 서식 속성을 물어볼지. 끈 항목은 묻지 않고 기본값으로 둔다(호출 수가 줄어든다).
pub(crate) struct Probe<'a> {
    pub items: ImportItems,
    pub general_format: Option<&'a str>,
}

pub(crate) fn read_styles(
    sheet: &Sheet,
    bounds: CellRange,
    probe: &Probe,
    model: &mut SheetModel,
    stats: &mut ReadStats,
    rep: &mut Reporter,
) -> Result<()> {
    let mut done = 0u64;
    let mut stack = vec![bounds];
    while let Some(b) = stack.pop() {
        rep.tick(done)?;
        stats.style_probes += 1;
        let style = match probe_style(&sheet.range(b)?, b, probe)? {
            Some(s) => s,
            None if !b.is_single() => {
                stack.extend(split(b));
                continue;
            }
            None => CellStyle::default(),
        };
        let id = match model.styles.iter().position(|s| *s == style) {
            Some(i) => i as u32,
            None => {
                model.styles.push(style);
                (model.styles.len() - 1) as u32
            }
        };
        for at in b.cells() {
            if let Some(cell) = model.cell_mut(at) {
                cell.s = id;
            }
        }
        stats.style_regions += 1;
        done += b.len();
    }
    Ok(())
}

/// 블록 전체의 서식이 같으면 그 서식, 하나라도 섞여 있으면 `None`. 섞인 속성을 만나는 즉시 멈춘다.
fn probe_style(rng: &Dispatch, block: CellRange, probe: &Probe) -> Result<Option<CellStyle>> {
    macro_rules! known {
        ($v:expr) => {{
            let v = $v;
            if v.is_null() {
                return Ok(None);
            }
            v
        }};
    }
    let mut style = CellStyle { font_size: 11.0, ..CellStyle::default() };
    style.number_format = known!(rng.get("NumberFormat")?).as_string().unwrap_or_default();
    if probe.general_format == Some(style.number_format.as_str()) {
        style.number_format = "General".into();
    }

    if probe.items.fills {
        let interior = rng.obj("Interior")?;
        let pattern = known!(interior.get("Pattern")?).as_i32().unwrap_or(XL_NONE);
        if pattern != XL_NONE {
            style.fill = Some(bgr_to_rgb(known!(interior.get("Color")?).as_f64()));
        }
    }

    if probe.items.fonts {
        let font = rng.obj("Font")?;
        style.font_name = known!(font.get("Name")?).as_string().unwrap_or_default();
        style.font_size = known!(font.get("Size")?).as_f64().unwrap_or(11.0) as f32;
        style.bold = known!(font.get("Bold")?).as_bool().unwrap_or(false);
        style.italic = known!(font.get("Italic")?).as_bool().unwrap_or(false);
        style.underline = known!(font.get("Underline")?).as_i32().unwrap_or(XL_NONE) != XL_NONE;
        style.strike = known!(font.get("Strikethrough")?).as_bool().unwrap_or(false);
        style.color = bgr_to_rgb(known!(font.get("Color")?).as_f64());
    }

    style.h_align = match known!(rng.get("HorizontalAlignment")?).as_i32() {
        Some(-4131) => HAlign::Left,
        Some(-4108) | Some(7) => HAlign::Center, // xlCenter, xlCenterAcrossSelection
        Some(-4152) => HAlign::Right,
        _ => HAlign::General,
    };
    style.v_align = match known!(rng.get("VerticalAlignment")?).as_i32() {
        Some(-4160) => VAlign::Top,
        Some(-4108) => VAlign::Center,
        _ => VAlign::Bottom,
    };
    style.wrap = known!(rng.get("WrapText")?).as_bool().unwrap_or(false);

    if probe.items.borders {
        let Some(borders) = probe_borders(&rng.obj("Borders")?, block)? else { return Ok(None) };
        style.borders = borders;
    }
    Ok(Some(style))
}

/// 블록 안 모든 셀의 네 변이 같을 때만 값을 돌려준다: 안쪽 가로선 = 위 = 아래, 안쪽 세로선 = 왼쪽 = 오른쪽.
fn probe_borders(borders: &Dispatch, block: CellRange) -> Result<Option<Borders>> {
    let Some(top) = edge(borders, XL_EDGE_TOP)? else { return Ok(None) };
    let Some(bottom) = edge(borders, XL_EDGE_BOTTOM)? else { return Ok(None) };
    if block.rows > 1 && (top != bottom || edge(borders, XL_INSIDE_HORIZONTAL)? != Some(top)) {
        return Ok(None);
    }
    let Some(left) = edge(borders, XL_EDGE_LEFT)? else { return Ok(None) };
    let Some(right) = edge(borders, XL_EDGE_RIGHT)? else { return Ok(None) };
    if block.cols > 1 && (left != right || edge(borders, XL_INSIDE_VERTICAL)? != Some(left)) {
        return Ok(None);
    }
    Ok(Some(Borders { top, right, bottom, left }))
}

/// 바깥 `None` = 섞임, 안쪽 `None` = 선 없음.
fn edge(borders: &Dispatch, index: i32) -> Result<Option<Option<BorderEdge>>> {
    let b = borders.obj_call("Item", vec![Var::i32(index)])?;
    let style = b.get("LineStyle")?;
    if style.is_null() {
        return Ok(None);
    }
    let style = style.as_i32().unwrap_or(XL_NONE);
    if style == XL_NONE {
        return Ok(Some(None));
    }
    let weight = b.get("Weight")?;
    let color = b.get("Color")?;
    if weight.is_null() || color.is_null() {
        return Ok(None);
    }
    Ok(Some(Some(BorderEdge { style, weight: weight.as_i32().unwrap_or(2), color: bgr_to_rgb(color.as_f64()) })))
}

/// Excel 색상은 `0x00BBGGRR` 정수다.
fn bgr_to_rgb(v: Option<f64>) -> u32 {
    let bgr = v.unwrap_or(0.0) as u32;
    ((bgr & 0xFF) << 16) | (bgr & 0xFF00) | ((bgr >> 16) & 0xFF)
}

/// 머리글 감지용으로 '굵게'만 읽는다(사전 감지에서 쓰는 가벼운 버전).
pub(crate) fn read_bold(sheet: &Sheet, bounds: CellRange, model: &mut SheetModel) -> Result<()> {
    let bold_style = model.styles.len() as u32;
    model.styles.push(CellStyle { bold: true, ..CellStyle::default() });
    let mut stack = vec![bounds];
    while let Some(b) = stack.pop() {
        let v = sheet.range(b)?.obj("Font")?.get("Bold")?;
        if v.is_null() {
            if !b.is_single() {
                stack.extend(split(b));
            }
            continue;
        }
        if v.as_bool() == Some(true) {
            for at in b.cells() {
                if let Some(cell) = model.cell_mut(at) {
                    cell.s = bold_style;
                }
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- 치수 · 숨김 · 틀 고정

/// 열 너비·행 높이(pt). 숨긴 행·열은 Excel이 크기를 0으로 알려 주므로 숨김도 여기서 판정한다.
/// (여러 행 범위의 `EntireRow.Hidden`은 섞여 있어도 Null이 아니라 False를 돌려줘 분할 정복에 쓸 수 없다.)
pub(crate) struct Dimensions {
    pub col_widths: Vec<f32>,
    pub row_heights: Vec<f32>,
}

impl Dimensions {
    pub fn hidden_rows(&self) -> Vec<u32> {
        zero_sized(&self.row_heights)
    }

    pub fn hidden_cols(&self) -> Vec<u32> {
        zero_sized(&self.col_widths)
    }
}

fn zero_sized(sizes: &[f32]) -> Vec<u32> {
    sizes.iter().enumerate().filter(|(_, s)| **s <= 0.0).map(|(i, _)| i as u32).collect()
}

/// `bounds` 범위의 열 너비(열마다)와 행 높이(같은 높이끼리 분할 정복). 결과 인덱스는 `bounds` 기준.
pub(crate) fn read_dimensions(sheet: &Sheet, bounds: CellRange) -> Result<Dimensions> {
    let mut col_widths = vec![DEFAULT_COL_PT; bounds.cols as usize];
    for (i, c) in (bounds.col..=bounds.last_col()).enumerate() {
        let col = sheet.range(CellRange::new(bounds.row, c, 1, 1))?.obj("EntireColumn")?;
        col_widths[i] = col.get("Width")?.as_f64().unwrap_or(DEFAULT_COL_PT as f64) as f32;
    }
    let mut row_heights = vec![DEFAULT_ROW_PT; bounds.rows as usize];
    let mut stack = vec![CellRange::new(bounds.row, bounds.col, bounds.rows, 1)];
    while let Some(b) = stack.pop() {
        let h = sheet.range(b)?.get("RowHeight")?;
        match h.as_f64() {
            Some(v) if !h.is_null() => {
                let from = (b.row - bounds.row) as usize;
                row_heights[from..from + b.rows as usize].fill(v as f32);
            }
            _ if b.rows > 1 => stack.extend(split(b)),
            _ => {}
        }
    }
    Ok(Dimensions { col_widths, row_heights })
}

/// 틀 고정 행/열 수(시트 A1 기준). 틀 고정은 그 시트를 보여 주는 창에서만 읽을 수 있어, 다른 시트가 활성이면
/// 화면 갱신과 이벤트(VBA `Worksheet_Activate` 등)를 끈 채 잠깐 활성화해 읽고 원래 시트·통합문서로 되돌린다.
pub(crate) fn read_freeze(book: &Dispatch, ws: &Dispatch, sheet_name: &str) -> Result<(u32, u32)> {
    let app = book.obj("Application")?;
    let screen = app.get("ScreenUpdating")?.as_bool().unwrap_or(true);
    app.put("ScreenUpdating", Var::bool(false))?;

    let active = book.obj("ActiveSheet").and_then(|s| s.get_string("Name")).unwrap_or_default();
    let result = if active == sheet_name { window_freeze(book) } else { freeze_of_inactive(&app, book, ws) };

    let _ = app.put("ScreenUpdating", Var::bool(screen));
    match result {
        Ok(v) => Ok(v),
        Err(e @ (ExcelError::Disconnected | ExcelError::Busy)) => Err(e),
        // 활성화할 수 없는 시트(숨김 시트 등)는 틀 고정 없음으로 본다.
        Err(_) => Ok((0, 0)),
    }
}

fn freeze_of_inactive(app: &Dispatch, book: &Dispatch, ws: &Dispatch) -> Result<(u32, u32)> {
    let prev_book = app.obj("ActiveWorkbook").ok();
    let prev_sheet = book.obj("ActiveSheet").ok();
    let events = app.get("EnableEvents")?.as_bool().unwrap_or(true);
    app.put("EnableEvents", Var::bool(false))?;

    let result = ws.call("Activate", vec![]).and_then(|_| window_freeze(book));

    if let Some(s) = prev_sheet {
        let _ = s.call("Activate", vec![]);
    }
    if let Some(b) = prev_book {
        let same = b.get_string("FullName").ok() == book.get_string("FullName").ok();
        if !same {
            let _ = b.call("Activate", vec![]);
        }
    }
    let _ = app.put("EnableEvents", Var::bool(events));
    result
}

/// 창의 틀 고정 행/열 수(시트 1행·A열부터).
///
/// `SplitRow`/`SplitColumn`은 창이 작으면 '보이는' 칸 수로 줄어든다(숨은 Excel 창은 작게 잡히기도 한다).
/// 스크롤되는 창(마지막 창)은 고정 영역 위로 올라갈 수 없으므로, 1행·A열로 보내 Excel이 막아 준 위치를 읽고
/// 원래 스크롤 위치로 되돌린다. 창 크기·스크롤 상태와 관계없이 정확하다.
fn window_freeze(book: &Dispatch) -> Result<(u32, u32)> {
    let win = book.obj("Windows")?.obj_call("Item", vec![Var::i32(1)])?;
    if win.get("FreezePanes")?.as_bool() != Some(true) {
        return Ok((0, 0));
    }
    let panes = win.obj("Panes")?;
    let last = panes.get("Count")?.as_i32().unwrap_or(1);
    let pane = panes.obj_call("Item", vec![Var::i32(last)])?;
    let scroll_row = pane.get("ScrollRow")?.as_i32().unwrap_or(1);
    let scroll_col = pane.get("ScrollColumn")?.as_i32().unwrap_or(1);
    pane.put("ScrollRow", Var::i32(1))?;
    pane.put("ScrollColumn", Var::i32(1))?;
    let first_row = pane.get("ScrollRow")?.as_i32().unwrap_or(1);
    let first_col = pane.get("ScrollColumn")?.as_i32().unwrap_or(1);
    pane.put("ScrollRow", Var::i32(scroll_row))?;
    pane.put("ScrollColumn", Var::i32(scroll_col))?;
    Ok(((first_row - 1).max(0) as u32, (first_col - 1).max(0) as u32))
}

// ---------------------------------------------------------------- 데이터 유효성

pub(crate) fn read_validations(sheet: &Sheet, bounds: CellRange) -> Result<Vec<Validation>> {
    let special = match sheet.range(bounds)?.obj_call("SpecialCells", vec![Var::i32(XL_CELL_TYPE_ALL_VALIDATION)]) {
        Ok(s) => s,
        // 유효성이 하나도 없으면 Excel이 1004 오류를 낸다.
        Err(ExcelError::Call { .. }) => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let areas = special.obj("Areas")?;
    let n = areas.get("Count")?.as_i32().unwrap_or(0);
    let abs_bounds = sheet.absolute(bounds);
    let mut out = Vec::new();
    for i in 1..=n {
        let addr = areas.obj_call("Item", vec![Var::i32(i)])?.get_string("Address")?;
        let Some(rel) = CellRange::parse(&addr)?.intersect(&abs_bounds).and_then(|a| a.relative_to(sheet.origin))
        else {
            continue;
        };
        read_validation_area(sheet, rel, &mut out)?;
    }
    Ok(out)
}

/// 인접한 서로 다른 유효성은 Areas에서 한 영역으로 합쳐질 수 있다. 그때는 `Validation.Type`이 오류를 내므로
/// 영역을 나눠 다시 읽는다. 병합 영역에 덮인 칸은 읽을 수 없어 건너뛴다.
fn read_validation_area(sheet: &Sheet, rel: CellRange, out: &mut Vec<Validation>) -> Result<()> {
    let v = sheet.range(rel)?.obj("Validation")?;
    let ty = match v.get("Type") {
        Ok(t) => t.as_i32().unwrap_or(0),
        Err(ExcelError::Call { .. }) if !rel.is_single() => {
            for part in split(rel) {
                read_validation_area(sheet, part, out)?;
            }
            return Ok(());
        }
        // 한 칸인데도 읽지 못하는 칸: 병합 영역에 덮인 칸(왼쪽 위 칸이 아닌 칸)은 Validation 속성이 모두 0x800A03EC로
        // 실패한다. 병합 영역의 유효성은 왼쪽 위 칸에서 읽히므로 건너뛴다. 유효성은 보조 정보라서 원인을 알 수 없는
        // 다른 칸 오류도 전체 읽기를 중단시키지 않고 그 칸만 건너뛴다.
        Err(ExcelError::Call { .. }) => return Ok(()),
        Err(e) => return Err(e),
    };
    let formula1 = v.get("Formula1").ok().and_then(|f| f.as_string()).unwrap_or_default();
    let rule = if ty == XL_VALIDATE_LIST {
        ValidationRule::List { items: list_items(sheet.ws, &formula1)?, source: formula1 }
    } else {
        ValidationRule::Other { excel_type: ty, formula1 }
    };
    out.push(Validation { range: rel, rule });
    Ok(())
}

fn list_items(ws: &Dispatch, formula1: &str) -> Result<Vec<String>> {
    let Some(expr) = formula1.strip_prefix('=') else {
        return Ok(formula1.split([',', ';']).map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect());
    };
    let result = ws.call("Evaluate", vec![Var::str(expr)])?;
    let grid = match result.as_dispatch() {
        Some(range) => Dispatch(range).get("Value2")?.into_grid()?,
        None => result.into_grid()?,
    };
    let mut items = Vec::new();
    for r in 0..grid.rows {
        for c in 0..grid.cols {
            match grid.value(r, c) {
                CellValue::Text(s) => items.push(s),
                CellValue::Number(n) if n.fract() == 0.0 => items.push(format!("{}", n as i64)),
                CellValue::Number(n) => items.push(n.to_string()),
                CellValue::Bool(b) => items.push(if b { "TRUE".into() } else { "FALSE".into() }),
                CellValue::Empty | CellValue::Error { .. } => {}
            }
        }
    }
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_prefers_columns_then_rows() {
        let [a, b] = split(CellRange::new(0, 0, 10, 9));
        assert_eq!((a.cols, b.cols, b.col), (4, 5, 4));
        let [a, b] = split(CellRange::new(2, 3, 1001, 1));
        assert_eq!((a.row, a.rows, b.row, b.rows), (2, 500, 502, 501));
    }

    #[test]
    fn bgr_conversion() {
        assert_eq!(bgr_to_rgb(Some(0xBFBFBF as f64)), 0xBFBFBF);
        assert_eq!(bgr_to_rgb(Some(0x0000FF as f64)), 0xFF0000); // Excel 빨강 = 255
    }

    #[test]
    fn reporter_throttles_and_cancels() {
        let mut calls = Vec::new();
        let mut cb = |p: ReadProgress| {
            calls.push(p.stage);
            p.stage != ReadStage::Validations
        };
        let mut rep = Reporter::new(&mut cb, 100);
        rep.stage(ReadStage::Values).unwrap();
        rep.tick(10).unwrap(); // 80ms가 지나지 않아 건너뜀
        rep.stage(ReadStage::Styles).unwrap();
        assert!(matches!(rep.stage(ReadStage::Validations), Err(ExcelError::Cancelled)));
        drop(rep);
        assert_eq!(calls, [ReadStage::Values, ReadStage::Styles, ReadStage::Validations]);
    }

    #[test]
    fn read_options_from_json_use_defaults() {
        let o: ReadOptions = serde_json::from_str(r#"{"range":"A1:I1002","items":{"borders":false}}"#).unwrap();
        assert_eq!(o.range.as_deref(), Some("A1:I1002"));
        assert!(!o.items.borders && o.items.fonts && !o.items.include_hidden);
        assert_eq!(o.header_rows, None);
    }
}
