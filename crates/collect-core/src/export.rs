//! 5단계 엑셀 저장: 불러온 시점의 시트(기준)와 지금 시트를 비교해 Excel에 쓸 것을 만든다.
//!
//! - [`plan`]: 바뀐 값(연속 블록), 앱에서 바꾼 서식·병합·열 너비·행 높이, 변경 칸 표시, '변경 이력' 시트.
//! - [`compare`]: 저장 직전에 읽은 Excel 값과 기준을 비교해 그사이 Excel에서 직접 고친 칸(충돌)과
//!   취합하지 않는 칸의 변화(표 구조가 바뀌었는지)를 찾는다.
//! - [`summary`]: 저장·마감 화면의 요약(필수 완료율, 담당자별 미입력, 바뀐 칸).
//!
//! 좌표는 모두 모델 기준이다. 시트 주소로 바꾸는 일(`origin`)과 실제 Excel 호출은 `excel-com`이 한다.
//!
//! 참여자가 표 아래에 붙인 행(지금 시트가 기준보다 행이 많음)은 [`RowInsert`]로 Excel에 그만큼 행을 끼워 넣은 뒤 쓴다.
//! 새 행의 '원래 값'은 빈 칸, '원래 서식'은 Excel이 끼워 넣을 때 따르는 기준 시트의 마지막 행 서식으로 본다.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::a1::{CellRange, CellRef};
use crate::access::{Access, flag};
use crate::config::CollectConfig;
use crate::history::ChangeEntry;
use crate::sheet::{Cell, CellStyle, CellValue, SheetModel};

/// 칸에 쓸 값. `literal`이면 글자를 그대로 둔다(Excel이 숫자·날짜·수식으로 바꾸지 않게 앞에 `'`를 붙여 쓴다).
#[derive(Debug, Clone, PartialEq)]
pub struct WriteValue {
    pub value: CellValue,
    pub literal: bool,
}

/// 한 열에서 위아래로 이어진 칸들(한 번에 쓴다).
#[derive(Debug, Clone, PartialEq)]
pub struct ValueBlock {
    pub range: CellRange,
    pub values: Vec<WriteValue>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StyleBlock {
    pub range: CellRange,
    pub style: CellStyle,
}

/// '변경 이력' 시트에 쓸 표. `time_col` 열은 Excel 날짜·시각 일련번호다.
#[derive(Debug, Clone, PartialEq)]
pub struct HistoryTable {
    pub header: Vec<String>,
    pub rows: Vec<Vec<CellValue>>,
    pub time_col: usize,
}

/// 표 아래에 붙인 행: 기준 시트의 끝(`at`, 모델 행)에 `count`행을 끼워 넣는다(아래 내용은 밀려 내려간다).
#[derive(Debug, Clone, PartialEq)]
pub struct RowInsert {
    pub at: u32,
    pub count: u32,
    /// 위 행(기준 시트 마지막 행)의 수식을 새 행까지 채워 내릴 열(Excel이 상대 참조를 맞춘다).
    pub formula_cols: Vec<u32>,
    /// 새 행 높이(pt).
    pub heights: Vec<f32>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ExportPlan {
    pub origin: CellRef,
    /// 표 아래에 붙인 행(값을 쓰기 전에 끼워 넣는다).
    pub insert: Option<RowInsert>,
    /// 값을 쓰는 칸(행 우선).
    pub cells: Vec<CellRef>,
    pub blocks: Vec<ValueBlock>,
    /// 앱에서 서식이 바뀐 칸(같은 서식끼리 위아래로 묶음).
    pub styles: Vec<StyleBlock>,
    pub merges_add: Vec<CellRange>,
    pub merges_remove: Vec<CellRange>,
    /// 앱에서 바꾼 열 너비(모델 열, pt).
    pub col_widths: Vec<(u32, f32)>,
    /// 앱에서 바꾼 행 높이(모델 행, pt).
    pub row_heights: Vec<(u32, f32)>,
    /// 값이 바뀐 칸에 칠할 채우기 색(`0xRRGGBB`).
    pub mark: Option<u32>,
    pub history: Option<HistoryTable>,
}

impl ExportPlan {
    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
            && self.insert.is_none()
            && self.styles.is_empty()
            && self.merges_add.is_empty()
            && self.merges_remove.is_empty()
            && self.col_widths.is_empty()
            && self.row_heights.is_empty()
            && self.history.is_none()
    }

    /// 크기를 바꾼 행·열 수.
    pub fn resized(&self) -> u32 {
        (self.col_widths.len() + self.row_heights.len()) as u32
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ExportOptions {
    /// 값이 바뀐 칸을 이 색으로 칠한다(`0xRRGGBB`).
    #[ts(optional)]
    pub mark: Option<u32>,
    /// 앱에서 바꾼 서식(글꼴·채우기·맞춤·테두리)·병합·열 너비·행 높이도 반영한다.
    pub styles: bool,
    /// '변경 이력' 시트를 더한다.
    pub history: bool,
}

/// 병합 영역에서 왼쪽 위가 아닌 칸(값이 없다).
fn covered(model: &SheetModel) -> Vec<bool> {
    let mut out = vec![false; model.cells.len()];
    for m in &model.merges {
        for at in m.cells().skip(1) {
            if at.row < model.rows && at.col < model.cols {
                out[(at.row * model.cols + at.col) as usize] = true;
            }
        }
    }
    out
}

/// 여러 칸에 걸친 병합의 왼쪽 위 칸(블록에 섞으면 Excel이 '병합된 셀의 일부'라며 거절한다).
fn merge_heads(model: &SheetModel) -> HashSet<CellRef> {
    model.merges.iter().filter(|m| !m.is_single()).map(CellRange::top_left).collect()
}

/// 지금 시트가 기준 시트와 비교할 수 있는 모양인가: 열이 같고, 행은 같거나 아래에 붙여 늘어났다.
fn comparable(baseline: &SheetModel, current: &SheetModel) -> bool {
    baseline.cols == current.cols
        && current.rows >= baseline.rows
        && baseline.cells.len() == (baseline.rows * baseline.cols) as usize
        && current.cells.len() == (current.rows * current.cols) as usize
}

/// 값이 바뀐 칸: 수식이 아니고, 병합에 덮이지 않았고, 기준과 값이 다른 칸(행 우선). 붙인 행은 빈 칸과 비교한다.
pub fn changed_cells(baseline: &SheetModel, current: &SheetModel) -> Vec<CellRef> {
    if !comparable(baseline, current) {
        return Vec::new();
    }
    let cover = covered(current);
    let cols = current.cols.max(1);
    current
        .cells
        .iter()
        .enumerate()
        .filter(|(i, now)| {
            now.f.is_none()
                && !cover[*i]
                && match baseline.cells.get(*i) {
                    Some(then) => now.v != then.v,
                    None => !now.v.is_empty(),
                }
        })
        .map(|(i, _)| CellRef::new(i as u32 / cols, i as u32 % cols))
        .collect()
}

/// 서식이 바뀐 칸(서식 표의 번호가 아니라 내용으로 비교한다). 붙인 행은 Excel이 끼워 넣을 때 따르는
/// 기준 시트 마지막 행의 서식과 비교한다.
fn style_changes(baseline: &SheetModel, current: &SheetModel) -> Vec<(CellRef, CellStyle)> {
    if !comparable(baseline, current) {
        return Vec::new();
    }
    let cols = current.cols.max(1);
    let fallback = CellStyle::default();
    let last = baseline.rows.checked_sub(1);
    current
        .cells
        .iter()
        .enumerate()
        .filter_map(|(i, now)| {
            let at = CellRef::new(i as u32 / cols, i as u32 % cols);
            let then = match baseline.cells.get(i) {
                Some(c) => Some(c),
                None => last.and_then(|r| baseline.cell(CellRef::new(r, at.col))),
            };
            let a = current.styles.get(now.s as usize).unwrap_or(&fallback);
            let b = then.and_then(|c| baseline.styles.get(c.s as usize)).unwrap_or(&fallback);
            (a != b).then(|| (at, a.clone()))
        })
        .collect()
}

/// 표 아래에 붙인 행(기준 시트보다 늘어난 행).
fn row_insert(baseline: &SheetModel, current: &SheetModel) -> Option<RowInsert> {
    if !comparable(baseline, current) || current.rows == baseline.rows {
        return None;
    }
    let at = baseline.rows;
    let formula_cols = (0..current.cols)
        .filter(|&c| {
            at > 0
                && current.cell(CellRef::new(at, c)).is_some_and(|cell| cell.f.is_some())
                && baseline.cell(CellRef::new(at - 1, c)).is_some_and(|cell| cell.f.is_some())
        })
        .collect();
    Some(RowInsert { at, count: current.rows - at, formula_cols, heights: current.row_heights[at as usize..].to_vec() })
}

/// 원본에 저장한 뒤: 붙인 행을 기준 시트에도 붙인다(이제 원본에 있는 행이다). `current`는 저장 계획을 만든 시트.
/// 값·수식은 쓴 그대로, 서식은 Excel이 끼워 넣을 때 따른 위 행(기준 시트 마지막 행)의 것으로 둔다(서식까지 반영했으면
/// 쓰는 쪽이 따로 맞춘다).
pub fn adopt_rows(baseline: &mut SheetModel, current: &SheetModel) {
    if !comparable(baseline, current) || current.rows == baseline.rows {
        return;
    }
    let last = baseline.rows.checked_sub(1);
    let above: Vec<u32> = (0..baseline.cols)
        .map(|c| last.and_then(|r| baseline.cell(CellRef::new(r, c))).map_or(0, |cell| cell.s))
        .collect();
    let from = baseline.cells.len();
    for (i, cell) in current.cells[from..].iter().enumerate() {
        let col = i % current.cols.max(1) as usize;
        baseline.cells.push(Cell { v: cell.v.clone(), f: cell.f.clone(), s: above[col] });
    }
    baseline.row_heights.extend(current.row_heights[baseline.rows as usize..].iter().copied());
    baseline.rows = current.rows;
}

/// 칸 목록을 열마다 위아래로 이어진 묶음으로 나눈다. `alone`에 든 칸은 혼자 묶는다.
fn vertical_runs<T: PartialEq>(mut cells: Vec<(CellRef, T)>, alone: &HashSet<CellRef>) -> Vec<(CellRange, Vec<T>)> {
    cells.sort_by_key(|(at, _)| (at.col, at.row));
    let mut runs: Vec<(CellRange, Vec<T>, bool)> = Vec::new();
    for (at, item) in cells {
        let single = alone.contains(&at);
        if let Some((range, items, last_single)) = runs.last_mut()
            && !single
            && !*last_single
            && range.col == at.col
            && range.last_row() + 1 == at.row
        {
            range.rows += 1;
            items.push(item);
            continue;
        }
        runs.push((CellRange::cell(at), vec![item], single));
    }
    runs.into_iter().map(|(r, items, _)| (r, items)).collect()
}

/// 크기가 바뀐 줄(열 또는 행): 숨긴 줄(어느 쪽이든 크기 0)은 뺀다. 0.1pt 안쪽 차이는 같은 크기로 본다.
fn size_changes(before: &[f32], after: &[f32]) -> Vec<(u32, f32)> {
    after
        .iter()
        .zip(before)
        .enumerate()
        .filter(|(_, (now, then))| **now > 0.0 && **then > 0.0 && (**now - **then).abs() > 0.1)
        .map(|(i, (now, _))| (i as u32, *now))
        .collect()
}

/// 같은 서식이 위아래로 이어진 칸을 묶는다.
fn style_runs(changes: Vec<(CellRef, CellStyle)>) -> Vec<StyleBlock> {
    let mut sorted = changes;
    sorted.sort_by_key(|(at, _)| (at.col, at.row));
    let mut out: Vec<StyleBlock> = Vec::new();
    for (at, style) in sorted {
        if let Some(last) = out.last_mut()
            && last.range.col == at.col
            && last.range.last_row() + 1 == at.row
            && last.style == style
        {
            last.range.rows += 1;
            continue;
        }
        out.push(StyleBlock { range: CellRange::cell(at), style });
    }
    out
}

/// 유닉스 시각(ms) → Excel 날짜·시각 일련번호(현지 시각, `offset_minutes` = UTC와의 차이).
pub fn excel_time(at_ms: i64, offset_minutes: i32) -> f64 {
    const UNIX_EPOCH_SERIAL: f64 = 25569.0; // 1970-01-01
    (at_ms as f64 / 1000.0 + f64::from(offset_minutes) * 60.0) / 86_400.0 + UNIX_EPOCH_SERIAL
}

/// 이력 표: 시각 · 사용자 · 셀 · 항목 · 이전 값 · 새 값(오래된 것부터).
pub fn history_table(
    current: &SheetModel,
    labels: &[String],
    history: &[ChangeEntry],
    offset_minutes: i32,
) -> HistoryTable {
    let header = ["시각", "사용자", "셀", "항목", "이전 값", "새 값"].map(String::from).to_vec();
    let rows = history
        .iter()
        .map(|h| {
            let addr = CellRef::new(h.row + current.origin.row, h.col + current.origin.col);
            vec![
                CellValue::Number(excel_time(h.at_ms, offset_minutes)),
                CellValue::Text(h.user.clone()),
                CellValue::Text(addr.to_string()),
                CellValue::Text(labels.get(h.col as usize).cloned().unwrap_or_default()),
                h.old.clone(),
                h.new.clone(),
            ]
        })
        .collect();
    HistoryTable { header, rows, time_col: 0 }
}

/// 저장 계획. `skip`에 든 칸은 쓰지 않는다(충돌에서 'Excel 값 유지'를 고른 칸).
pub fn plan(
    baseline: &SheetModel,
    current: &SheetModel,
    labels: &[String],
    history: &[ChangeEntry],
    opts: ExportOptions,
    offset_minutes: i32,
    skip: &HashSet<CellRef>,
) -> ExportPlan {
    let cells: Vec<CellRef> = changed_cells(baseline, current).into_iter().filter(|at| !skip.contains(at)).collect();
    let text_format = |at: CellRef| {
        let s = current.cell(at).map_or(0, |c| c.s) as usize;
        current.styles.get(s).is_some_and(|st| st.number_format == "@")
    };
    let values = cells
        .iter()
        .map(|&at| {
            let value = current.cell(at).map(|c| c.v.clone()).unwrap_or_default();
            let literal = matches!(value, CellValue::Text(_)) && !text_format(at);
            (at, WriteValue { value, literal })
        })
        .collect();
    let blocks = vertical_runs(values, &merge_heads(current))
        .into_iter()
        .map(|(range, values)| ValueBlock { range, values })
        .collect();

    let (styles, merges_add, merges_remove) = if opts.styles {
        let before: HashSet<CellRange> = baseline.merges.iter().copied().collect();
        let after: HashSet<CellRange> = current.merges.iter().copied().collect();
        let mut add: Vec<CellRange> = after.difference(&before).copied().collect();
        let mut remove: Vec<CellRange> = before.difference(&after).copied().collect();
        add.sort_by_key(|m| (m.row, m.col));
        remove.sort_by_key(|m| (m.row, m.col));
        (style_runs(style_changes(baseline, current)), add, remove)
    } else {
        (Vec::new(), Vec::new(), Vec::new())
    };
    let (col_widths, row_heights) = if opts.styles && comparable(baseline, current) {
        (
            size_changes(&baseline.col_widths, &current.col_widths),
            size_changes(&baseline.row_heights, &current.row_heights),
        )
    } else {
        (Vec::new(), Vec::new())
    };

    let history =
        (opts.history && !history.is_empty()).then(|| history_table(current, labels, history, offset_minutes));
    ExportPlan {
        origin: current.origin,
        insert: row_insert(baseline, current),
        cells,
        blocks,
        styles,
        merges_add,
        merges_remove,
        col_widths,
        row_heights,
        mark: opts.mark,
        history,
    }
}

/// 저장 직전에 Excel에서 그사이 직접 고친 칸.
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[ts(export)]
pub struct Conflict {
    pub at: CellRef,
    /// 불러올 때 값.
    #[ts(as = "Option<CellValue>", optional)]
    pub original: CellValue,
    /// 지금 Excel 값.
    #[ts(as = "Option<CellValue>", optional)]
    pub excel: CellValue,
    /// 취합한 값.
    #[ts(as = "Option<CellValue>", optional)]
    pub collected: CellValue,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CheckReport {
    pub conflicts: Vec<Conflict>,
    /// 취합하지 않는 칸 중 Excel에서 값이 바뀐 칸(앞 몇 개).
    pub drift: Vec<CellRef>,
    pub drift_count: u32,
    /// 비교한 칸 수(기준이나 Excel에 값이 있는 칸).
    pub compared: u32,
    /// 바뀐 칸이 너무 많다: 행·열을 넣거나 지워 표 위치가 밀렸을 수 있다.
    pub suspicious: bool,
}

const DRIFT_SAMPLES: usize = 20;

/// `excel`은 불러온 범위를 지금 Excel에서 다시 읽은 값(기준 시트 크기, 행 우선). `cells`는 이번에 쓸 칸.
/// 표 아래에 붙인 행의 칸은 Excel에 아직 없으므로 충돌이 될 수 없다.
pub fn compare(baseline: &SheetModel, current: &SheetModel, excel: &[CellValue], cells: &[CellRef]) -> CheckReport {
    let mut report = CheckReport::default();
    if !comparable(baseline, current) || excel.len() != baseline.cells.len() {
        report.suspicious = true;
        return report;
    }
    let cols = baseline.cols.max(1);
    let idx = |at: CellRef| (at.row * cols + at.col) as usize;
    let writing: HashSet<CellRef> = cells.iter().copied().collect();
    for &at in cells.iter().filter(|at| at.row < baseline.rows) {
        let i = idx(at);
        let (original, now, collected) = (&baseline.cells[i].v, &excel[i], &current.cells[i].v);
        // Excel에서도 같은 값으로 고쳤으면 충돌이 아니다.
        if now != original && now != collected {
            report.conflicts.push(Conflict {
                at,
                original: original.clone(),
                excel: now.clone(),
                collected: collected.clone(),
            });
        }
    }
    let cover = covered(baseline);
    for (i, cell) in baseline.cells.iter().enumerate() {
        let at = CellRef::new(i as u32 / cols, i as u32 % cols);
        if cell.f.is_some() || cover[i] || writing.contains(&at) {
            continue;
        }
        if cell.v.is_empty() && excel[i].is_empty() {
            continue;
        }
        report.compared += 1;
        if excel[i] != cell.v {
            report.drift_count += 1;
            if report.drift.len() < DRIFT_SAMPLES {
                report.drift.push(at);
            }
        }
    }
    report.suspicious = report.drift_count > 20 && report.drift_count * 5 > report.compared;
    report
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PersonMissing {
    pub name: String,
    /// 명단 순서(색).
    pub color: i32,
    pub required: u32,
    pub filled: u32,
    /// 비어 있는 필수 칸(시트 주소, 앞 몇 개).
    pub missing: Vec<String>,
    /// 위 목록에 못 넣은 나머지 수.
    pub more: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PersonChanges {
    pub name: String,
    /// 이 사람이 바꾼 칸 수(같은 칸을 여러 번 바꿔도 하나).
    pub cells: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SaveSummary {
    pub required: u32,
    pub filled: u32,
    /// 담당자가 없는 필수 칸(누구나 입력할 수 있다).
    pub unassigned: u32,
    pub people: Vec<PersonMissing>,
    /// 불러올 때(또는 마지막으로 원본에 저장할 때)와 값이 다른 칸.
    pub changed_cells: u32,
    pub changed_by: Vec<PersonChanges>,
    #[ts(optional, type = "number")]
    pub last_change_ms: Option<i64>,
    pub history_entries: u32,
    /// 앱에서 서식이 바뀐 칸·병합 변화.
    pub style_cells: u32,
    pub merge_changes: u32,
    /// 앱에서 크기를 바꾼 열·행 수.
    pub size_changes: u32,
    /// 참여자가 표 아래에 추가한 행 수(저장할 때 Excel에 끼워 넣는다).
    pub added_rows: u32,
}

const MISSING_SAMPLES: usize = 12;

pub fn summary(
    baseline: &SheetModel,
    current: &SheetModel,
    config: &CollectConfig,
    history: &[ChangeEntry],
) -> SaveSummary {
    let access = Access::compile(current, config);
    let progress = access.progress(current);
    let mut people: Vec<PersonMissing> = progress
        .by_person
        .iter()
        .enumerate()
        .map(|(i, p)| PersonMissing {
            name: p.name.clone(),
            color: i as i32,
            required: p.required,
            filled: p.filled,
            ..Default::default()
        })
        .collect();
    for r in current.header_rows..current.rows {
        for c in 0..current.cols {
            let at = CellRef::new(r, c);
            if access.flags(at) & flag::REQUIRED == 0 || current.cell(at).is_some_and(|cell| !cell.v.is_empty()) {
                continue;
            }
            let addr = CellRef::new(r + current.origin.row, c + current.origin.col).to_string();
            for p in access.owners(at) {
                let person = &mut people[p];
                if person.missing.len() < MISSING_SAMPLES {
                    person.missing.push(addr.clone());
                } else {
                    person.more += 1;
                }
            }
        }
    }

    // 명단에 있는 사람만 센다('Excel 값 유지' 같은 호스트 기록은 빼고).
    let mut by_user: HashMap<&str, HashSet<(u32, u32)>> = HashMap::new();
    for h in history.iter().filter(|h| access.person(&h.user).is_some()) {
        by_user.entry(h.user.as_str()).or_default().insert((h.row, h.col));
    }
    let mut changed_by: Vec<PersonChanges> = by_user
        .into_iter()
        .map(|(name, cells)| PersonChanges { name: name.to_string(), cells: cells.len() as u32 })
        .collect();
    changed_by.sort_by(|a, b| b.cells.cmp(&a.cells).then_with(|| a.name.cmp(&b.name)));

    let before: HashSet<&CellRange> = baseline.merges.iter().collect();
    let after: HashSet<&CellRange> = current.merges.iter().collect();
    SaveSummary {
        required: progress.required,
        filled: progress.filled,
        unassigned: progress.unassigned,
        people,
        changed_cells: changed_cells(baseline, current).len() as u32,
        changed_by,
        last_change_ms: history.iter().map(|h| h.at_ms).max(),
        history_entries: history.len() as u32,
        style_cells: style_changes(baseline, current).len() as u32,
        merge_changes: before.symmetric_difference(&after).count() as u32,
        size_changes: if comparable(baseline, current) {
            (size_changes(&baseline.col_widths, &current.col_widths).len()
                + size_changes(&baseline.row_heights, &current.row_heights).len()) as u32
        } else {
            0
        },
        added_rows: row_insert(baseline, current).map_or(0, |ins| ins.count),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::tests::{assigned, budget};
    use crate::format::{FillPatch, FormatEdit, StylePatch, apply_format};
    use crate::history::{BatchOrigin, ChangeKind};

    fn at(a: &str) -> CellRef {
        CellRef::parse(a).unwrap()
    }

    fn set(m: &mut SheetModel, a: &str, v: CellValue) {
        m.cell_mut(at(a)).unwrap().v = v;
    }

    fn opts() -> ExportOptions {
        ExportOptions { mark: None, styles: true, history: true }
    }

    #[test]
    fn changed_cells_skip_formulas_and_covered_cells() {
        let base = budget();
        let mut now = base.clone();
        set(&mut now, "F3", CellValue::Number(9800.0));
        set(&mut now, "F4", CellValue::Empty); // 5000 → 지움
        set(&mut now, "G3", CellValue::Number(0.065)); // 수식 결과(재계산)는 쓰지 않는다
        set(&mut now, "H5", CellValue::Text("교재".into()));
        assert_eq!(changed_cells(&base, &now), [at("F3"), at("F4"), at("H5")]);
    }

    #[test]
    fn values_are_grouped_into_column_runs_but_merge_heads_stand_alone() {
        let base = budget();
        let mut now = base.clone();
        for a in ["F3", "F4", "F5", "F7", "H3", "C5"] {
            set(&mut now, a, CellValue::Text("x".into()));
        }
        let p = plan(&base, &now, &[], &[], opts(), 0, &HashSet::new());
        let ranges: Vec<String> = p.blocks.iter().map(|b| b.range.to_string()).collect();
        // C5는 C5:C6 병합의 머리 → 혼자. F3:F5는 한 번에, F7은 따로.
        assert_eq!(ranges, ["C5", "F3:F5", "F7", "H3"]);
        assert!(p.blocks.iter().all(|b| b.values.iter().all(|v| v.literal)));
    }

    #[test]
    fn text_formatted_cells_are_not_prefixed_and_numbers_never_are() {
        let base = budget();
        let mut now = base.clone();
        now.styles.push(CellStyle { number_format: "@".into(), ..Default::default() });
        now.cell_mut(at("H3")).unwrap().s = 2;
        set(&mut now, "H3", CellValue::Text("0012".into()));
        set(&mut now, "H5", CellValue::Text("=1+1".into()));
        set(&mut now, "F3", CellValue::Number(1.0));
        let p = plan(&base, &now, &[], &[], ExportOptions::default(), 0, &HashSet::new());
        let literal: HashMap<String, bool> =
            p.blocks.iter().map(|b| (b.range.to_string(), b.values[0].literal)).collect();
        assert_eq!(literal["H3"], false);
        assert_eq!(literal["H5"], true);
        assert_eq!(literal["F3"], false);
        assert!(p.styles.is_empty(), "서식 반영을 끄면 서식 변화(H3의 @)는 쓰지 않는다");
    }

    #[test]
    fn styles_and_merges_follow_the_option() {
        let base = budget();
        let mut now = base.clone();
        let fill = StylePatch { fill: Some(FillPatch::Color { rgb: 0xFFF3B0 }), ..Default::default() };
        apply_format(&mut now, FormatEdit::Style { range: CellRange::parse("H3:H5").unwrap(), patch: fill }).unwrap();
        apply_format(&mut now, FormatEdit::Merge { range: CellRange::parse("H5:I6").unwrap() }).unwrap();
        apply_format(&mut now, FormatEdit::Unmerge { range: CellRange::parse("B3").unwrap() }).unwrap();
        let p = plan(&base, &now, &[], &[], opts(), 0, &HashSet::new());
        assert_eq!(p.styles.len(), 1);
        assert_eq!(p.styles[0].range.to_string(), "H3:H5");
        assert_eq!(p.styles[0].style.fill, Some(0xFFF3B0));
        assert_eq!(p.merges_add, [CellRange::parse("H5:I6").unwrap()]);
        assert_eq!(p.merges_remove, [CellRange::parse("B3:B4").unwrap()]);
        assert!(p.cells.is_empty());
        let off = plan(&base, &now, &[], &[], ExportOptions::default(), 0, &HashSet::new());
        assert!(off.is_empty());
    }

    #[test]
    fn sizes_follow_the_style_option_and_skip_hidden_lines() {
        let base = budget();
        let mut now = base.clone();
        apply_format(&mut now, FormatEdit::ColWidth { from: 7, count: 1, width: 150.0 }).unwrap();
        apply_format(&mut now, FormatEdit::RowHeight { from: 2, count: 2, height: 30.0 }).unwrap();
        now.row_heights[5] = 0.0; // 숨긴 행(크기 0)은 쓰지 않는다
        let p = plan(&base, &now, &[], &[], opts(), 0, &HashSet::new());
        assert_eq!(p.col_widths, [(7, 150.0)]);
        assert_eq!(p.row_heights, [(2, 30.0), (3, 30.0)]);
        assert_eq!(p.resized(), 3);
        assert!(!p.is_empty());
        let off = plan(&base, &now, &[], &[], ExportOptions::default(), 0, &HashSet::new());
        assert!(off.col_widths.is_empty() && off.row_heights.is_empty() && off.is_empty());
    }

    #[test]
    fn history_sheet_rows_and_skip() {
        let base = budget();
        let mut now = base.clone();
        set(&mut now, "F3", CellValue::Number(9800.0));
        set(&mut now, "F5", CellValue::Number(1.0));
        let labels: Vec<String> = (0..9).map(|c| format!("열{c}")).collect();
        let history = vec![ChangeEntry {
            seq: 0,
            batch: String::new(),
            origin: BatchOrigin::Edit,
            kind: ChangeKind::Edit,
            at_ms: 0,
            user: "이서연".into(),
            row: 2,
            col: 5,
            old: CellValue::Empty,
            new: CellValue::Number(9800.0),
        }];
        let skip: HashSet<CellRef> = [at("F5")].into();
        let p = plan(&base, &now, &labels, &history, opts(), 540, &skip);
        assert_eq!(p.cells, [at("F3")]);
        let h = p.history.unwrap();
        assert_eq!(h.header[0], "시각");
        assert_eq!(h.rows[0][2], CellValue::Text("F3".into()));
        assert_eq!(h.rows[0][3], CellValue::Text("열5".into()));
        // 1970-01-01 09:00 (UTC+9)
        assert_eq!(h.rows[0][0], CellValue::Number(25569.375));
    }

    #[test]
    fn compare_finds_conflicts_and_structure_drift() {
        let base = budget();
        let mut now = base.clone();
        set(&mut now, "F3", CellValue::Number(9800.0));
        set(&mut now, "F5", CellValue::Number(1800.0));
        set(&mut now, "F6", CellValue::Number(2000.0));
        let cells = changed_cells(&base, &now);
        let mut excel: Vec<CellValue> = base.cells.iter().map(|c| c.v.clone()).collect();
        let i = |a: &str| (at(a).row * base.cols + at(a).col) as usize;
        excel[i("F3")] = CellValue::Number(1234.0); // Excel에서 직접 고침 → 충돌
        excel[i("F5")] = CellValue::Number(1800.0); // 같은 값으로 고침 → 충돌 아님
        excel[i("D8")] = CellValue::Text("교통비".into()); // 취합하지 않는 칸의 변화
        let r = compare(&base, &now, &excel, &cells);
        assert_eq!(r.conflicts.len(), 1);
        assert_eq!(r.conflicts[0].at, at("F3"));
        assert_eq!(
            (r.conflicts[0].excel.clone(), r.conflicts[0].collected.clone()),
            (CellValue::Number(1234.0), CellValue::Number(9800.0))
        );
        assert_eq!((r.drift_count, r.drift.clone()), (1, vec![at("D8")]));
        assert!(!r.suspicious);

        // 행을 하나 넣어 표가 밀린 경우: 거의 모든 칸이 달라진다.
        let cols = base.cols as usize;
        let mut big = base.clone();
        for _ in 0..4 {
            big.cells.extend(base.cells[2 * cols..].iter().cloned());
        }
        big.rows = (big.cells.len() / cols) as u32;
        let mut shifted_big = vec![CellValue::Empty; cols];
        shifted_big.extend(big.cells.iter().take(big.cells.len() - cols).map(|c| c.v.clone()));
        let r = compare(&big, &big, &shifted_big, &[]);
        assert!(r.suspicious, "{r:?}");
        assert!(compare(&base, &now, &excel[1..], &cells).suspicious, "크기가 다르면 의심");
    }

    #[test]
    fn summary_lists_missing_required_cells_per_person() {
        let base = budget();
        let mut cfg = assigned(&base);
        cfg.columns[5].required = true;
        let mut now = base.clone();
        set(&mut now, "F3", CellValue::Number(9800.0));
        let history = vec![
            ChangeEntry {
                seq: 0,
                batch: String::new(),
                origin: BatchOrigin::Edit,
                kind: ChangeKind::Edit,
                at_ms: 10,
                user: "이서연".into(),
                row: 2,
                col: 5,
                old: CellValue::Empty,
                new: CellValue::Number(1.0),
            },
            ChangeEntry {
                seq: 0,
                batch: String::new(),
                origin: BatchOrigin::Edit,
                kind: ChangeKind::Edit,
                at_ms: 20,
                user: "이서연".into(),
                row: 2,
                col: 5,
                old: CellValue::Number(1.0),
                new: CellValue::Number(9800.0),
            },
            // 'Excel 값 유지' 같은 호스트 기록은 이력 수에는 들지만 입력한 사람에는 들지 않는다.
            ChangeEntry {
                seq: 0,
                batch: String::new(),
                origin: BatchOrigin::Edit,
                kind: ChangeKind::Edit,
                at_ms: 30,
                user: "Excel(원본)".into(),
                row: 3,
                col: 5,
                old: CellValue::Number(5000.0),
                new: CellValue::Number(5100.0),
            },
        ];
        let s = summary(&base, &now, &cfg, &history);
        assert_eq!((s.required, s.filled), (6, 3));
        let lee = &s.people[0];
        assert_eq!((lee.name.as_str(), lee.required, lee.filled), ("이서연", 2, 2));
        let choi = &s.people[1];
        assert_eq!(choi.missing, ["F5", "F6"]);
        assert_eq!(s.changed_cells, 1);
        assert_eq!(s.changed_by, [PersonChanges { name: "이서연".into(), cells: 1 }]);
        assert_eq!((s.last_change_ms, s.history_entries), (Some(30), 3));
    }

    #[test]
    fn appended_rows_are_inserted_then_written() {
        let base = budget();
        let mut now = base.clone();
        let added = now.append_rows(2); // 9·10행(모델 8·9행)
        assert_eq!(added.row, 8);
        set(&mut now, "F9", CellValue::Number(700.0));
        set(&mut now, "H10", CellValue::Text("새 비목".into()));
        set(&mut now, "F3", CellValue::Number(1.0));
        assert_eq!(changed_cells(&base, &now), [at("F3"), at("F9"), at("H10")]);
        let p = plan(&base, &now, &[], &[], opts(), 0, &HashSet::new());
        let insert = p.insert.clone().unwrap();
        assert_eq!((insert.at, insert.count), (8, 2));
        assert_eq!(insert.formula_cols, [6], "G열(증감률) 수식을 채워 내린다");
        assert_eq!(insert.heights, [15.0, 15.0]);
        // 새 행은 마지막 행 서식을 그대로 따르므로 서식 변화가 아니다.
        assert!(p.styles.is_empty(), "{:?}", p.styles);
        assert!(p.row_heights.is_empty());
        // 그사이 Excel에서 고친 칸 비교는 기준 시트 크기로 한다(새 행은 충돌이 될 수 없다).
        let excel: Vec<CellValue> = base.cells.iter().map(|c| c.v.clone()).collect();
        let r = compare(&base, &now, &excel, &p.cells);
        assert!(!r.suspicious && r.conflicts.is_empty(), "{r:?}");
        // 원본에 저장하면 새 행이 기준이 된다.
        let mut adopted = base.clone();
        adopt_rows(&mut adopted, &now);
        assert_eq!((adopted.rows, adopted.cells.len(), adopted.row_heights.len()), (10, 90, 10));
        assert!(plan(&adopted, &now, &[], &[], opts(), 0, &HashSet::new()).insert.is_none());
        assert_eq!(changed_cells(&adopted, &now), [at("F3")]);
    }

    #[test]
    fn excel_time_serial() {
        assert_eq!(excel_time(0, 0), 25569.0);
        assert!((excel_time(86_400_000 / 2, 0) - 25569.5).abs() < 1e-12);
    }
}
