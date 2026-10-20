//! 앱에서 바꾸는 표 구조: 표 끝에 행·열 붙이기, 행 지우기(엑셀에서 불러온 행도), 앱에서 붙인 열 지우기, 행 취소 표시.
//!
//! 행은 표 아래에, 열은 표 오른쪽에만 붙인다. 행은 엑셀에서 불러온 행도 지울 수 있다(세션이 기준 시트에서도 지우고 그
//! 순서를 남겨 저장할 때 Excel에서도 지운다). 열은 앱에서 붙인(기준 시트에 없는) 열만 지운다. 지우면 그 뒤의 행·열이
//! 당겨진다([`SheetModel::layout`]이 오른다).
//!
//! 여기는 판정·좌표 계산만 한다. 시트·설정·세션 상태를 함께 바꾸는 일은 세션이 한다.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::a1::{CellRange, shift_after_removal};
use crate::access::Access;
use crate::config::CollectConfig;
use crate::history::ChangeEntry;
use crate::sheet::{CellValue, SheetModel};

/// 표 구조 바꾸기(호스트 1b·1d 오른쪽 클릭 메뉴, 참여자 행 번호 메뉴·취소 행).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "op", rename_all = "camelCase")]
#[ts(export)]
pub enum StructureEdit {
    /// 표 아래에 빈 행.
    AddRows { count: u32 },
    /// 표 오른쪽에 빈 열(호스트만).
    AddCols { count: u32 },
    /// 행 지우기(엑셀에서 불러온 행도 — 저장할 때 Excel에서도 지운다).
    DeleteRows { from: u32, count: u32 },
    /// 앱에서 붙인 열 지우기(호스트만).
    DeleteCols { from: u32, count: u32 },
    /// 행을 취소 표시하거나(`on`) 푼다.
    CancelRows { from: u32, count: u32, on: bool },
}

/// 한 번에 붙이는 최대 행·열 수.
pub const MAX_ADD: u32 = 20;
/// 엑셀에 저장하기 전까지(기준 시트보다) 붙일 수 있는 행 수.
pub const MAX_ADDED_ROWS: u32 = 1_000;
/// 엑셀에 저장하기 전까지 붙일 수 있는 열 수.
pub const MAX_ADDED_COLS: u32 = 50;
/// 행·열을 붙여도 넘지 않는 시트 크기(칸).
pub const MAX_SHEET_CELLS: u64 = 1_000_000;

/// 누가 지우나. 참여자는 자기가 추가한 행과 자기에게 배정된 행만 지운다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Actor<'a> {
    Host,
    Person(&'a str),
}

fn same_name(a: &str, b: &str) -> bool {
    a.trim() == b.trim()
}

/// 행·열을 붙일 수 있나(한 번에 [`MAX_ADD`]개, 저장 전까지 붙인 수, 시트 크기).
pub fn check_add(model: &SheetModel, baseline: &SheetModel, edit: StructureEdit) -> Result<u32, String> {
    match edit {
        StructureEdit::AddRows { count } => {
            let count = count.clamp(1, MAX_ADD);
            if model.rows.saturating_sub(baseline.rows) + count > MAX_ADDED_ROWS {
                return Err(format!("저장하기 전까지 한 시트에 행은 {MAX_ADDED_ROWS}개까지 추가할 수 있습니다"));
            }
            if u64::from(model.rows + count) * u64::from(model.cols) > MAX_SHEET_CELLS {
                return Err("시트가 너무 커서 행을 더 추가할 수 없습니다".into());
            }
            Ok(count)
        }
        StructureEdit::AddCols { count } => {
            let count = count.clamp(1, MAX_ADD);
            if model.cols.saturating_sub(baseline.cols) + count > MAX_ADDED_COLS {
                return Err(format!("저장하기 전까지 한 시트에 열은 {MAX_ADDED_COLS}개까지 추가할 수 있습니다"));
            }
            if u64::from(model.rows) * u64::from(model.cols + count) > MAX_SHEET_CELLS {
                return Err("시트가 너무 커서 열을 더 추가할 수 없습니다".into());
            }
            Ok(count)
        }
        _ => Err("추가하는 동작이 아닙니다".into()),
    }
}

/// 여러 행에 걸친 병합 중 첫 행이 지울 범위(`from`부터 `count`개) 안에 있고 아래로 그 범위를 넘는 것(지우면 병합의 값이
/// 사라진다).
pub fn cut_merge(model: &SheetModel, from: u32, count: u32) -> Option<CellRange> {
    let end = from.saturating_add(count);
    model.merges.iter().find(|m| !m.is_single() && from <= m.row && m.row < end && m.last_row() >= end).copied()
}

/// 행 `from`부터 `count`개를 지울 수 있나. 엑셀에서 불러온 행도 지운다(저장할 때 Excel에서도 지운다).
///
/// - 머리글 행은 지우지 않는다. 여러 행에 걸친 병합의 첫 행만 지우는 것([`cut_merge`])도 거절한다(병합의 값이 사라진다).
/// - 호스트: 담당자가 배정된 행(행 배정 기준 칸에 이름이 있음)이나 범위 배정이 걸친 행은 지우지 않는다.
/// - 참여자: 자기가 추가한 행이나 자기에게 배정된 행(행 배정)만, 남의 범위 배정이 걸친 행은 지우지 않는다.
pub fn check_delete_rows(
    model: &SheetModel,
    config: &CollectConfig,
    access: &Access,
    from: u32,
    count: u32,
    actor: Actor,
) -> Result<(), String> {
    let end = from.saturating_add(count);
    if count == 0 || end > model.rows {
        return Err("없는 행입니다".into());
    }
    if from < model.header_rows {
        return Err("머리글 행은 지울 수 없습니다".into());
    }
    if let Some(m) = cut_merge(model, from, count) {
        let o = model.origin;
        let at = CellRange::new(m.row + o.row, m.col + o.col, m.rows, m.cols);
        return Err(format!("병합된 칸({at})의 첫 행만 지울 수는 없습니다. 병합된 행을 모두 함께 지우세요"));
    }
    let me = match actor {
        Actor::Person(name) => access.person(name),
        Actor::Host => None,
    };
    for r in from..end {
        let ranges = config.assignment.ranges.iter().filter(|a| a.range.row <= r && r <= a.range.last_row());
        match actor {
            Actor::Host => {
                if let Some(p) = access.row_owner(r) {
                    let name = access.roster().get(p).map_or("", String::as_str);
                    return Err(format!(
                        "담당자({name})가 배정된 행은 지울 수 없습니다. 행 배정 기준 칸을 비운 뒤 지우세요"
                    ));
                }
                if let Some(a) = ranges.into_iter().next() {
                    return Err(format!("담당자({})가 배정된 범위가 걸친 행은 지울 수 없습니다", a.person.trim()));
                }
            }
            Actor::Person(name) => {
                let added = model.added_by(r).is_some_and(|by| same_name(by, name));
                let mine = me.is_some() && access.row_owner(r) == me;
                if !added && !mine {
                    return Err("내가 추가한 행이나 내게 배정된 행만 지울 수 있습니다".into());
                }
                if let Some(a) = ranges.into_iter().find(|a| !same_name(&a.person, name)) {
                    return Err(format!("{} 님에게 배정된 범위가 걸친 행은 지울 수 없습니다", a.person.trim()));
                }
            }
        }
    }
    Ok(())
}

/// 열 `from`부터 `count`개를 지울 수 있나(호스트만): 앱에서 붙인 열만, 행 배정 기준 열과 범위 배정이 걸친 열은 지우지
/// 않는다.
pub fn check_delete_cols(
    model: &SheetModel,
    baseline_cols: u32,
    config: &CollectConfig,
    from: u32,
    count: u32,
) -> Result<(), String> {
    let end = from.saturating_add(count);
    if count == 0 || end > model.cols {
        return Err("없는 열입니다".into());
    }
    if from < baseline_cols {
        return Err("엑셀에서 불러온 열은 지울 수 없습니다(앱에서 추가한 열만 지울 수 있습니다)".into());
    }
    if config.assignment.key_col.is_some_and(|k| (from..end).contains(&k)) {
        return Err("행 배정 기준 열은 지울 수 없습니다".into());
    }
    let cols = CellRange::new(0, from, model.rows.max(1), count);
    if let Some(a) = config.assignment.ranges.iter().find(|a| a.range.intersect(&cols).is_some()) {
        return Err(format!("담당자({})가 배정된 열은 지울 수 없습니다", a.person.trim()));
    }
    Ok(())
}

/// 지울 행·열에 값이 있는 칸 수(지우기 전에 확인을 묻는다).
pub fn filled_cells(model: &SheetModel, range: CellRange) -> u32 {
    range.cells().filter(|&at| model.cell(at).is_some_and(|c| c.f.is_none() && !c.v.is_empty())).count() as u32
}

/// 셀 값 배열(행 우선, 열 `cols`개 — 스냅샷)을 구조 바꾸기에 맞춘다. 행은 배열보다 적을 수 있다(스냅샷 뒤에 붙인
/// 행은 없다). 모양이 맞지 않는 배열은 그대로 둔다.
pub fn reshape_values(values: &mut Vec<CellValue>, cols: u32, edit: StructureEdit) {
    let cols = cols as usize;
    if cols == 0 || !values.len().is_multiple_of(cols) {
        return;
    }
    let rows = values.len() / cols;
    match edit {
        StructureEdit::AddCols { count } => {
            let count = count as usize;
            let mut out = Vec::with_capacity(rows * (cols + count));
            for row in values.chunks(cols) {
                out.extend_from_slice(row);
                out.extend(std::iter::repeat_n(CellValue::Empty, count));
            }
            *values = out;
        }
        StructureEdit::DeleteRows { from, count } => {
            let (from, end) = ((from as usize).min(rows), (from.saturating_add(count) as usize).min(rows));
            values.drain(from * cols..end * cols);
        }
        StructureEdit::DeleteCols { from, count } => {
            let (from, end) = (from as usize, from as usize + count as usize);
            let mut i = 0;
            values.retain(|_| {
                let c = i % cols;
                i += 1;
                c < from || c >= end
            });
        }
        StructureEdit::AddRows { .. } | StructureEdit::CancelRows { .. } => {}
    }
}

/// 변경 기록의 좌표를 지운 행·열에 맞춘다: 지운 칸의 기록은 [`ChangeEntry::gone`]으로 두고, 뒤의 칸은 당긴다.
/// 바뀐 기록이 있으면 `true`.
pub fn remap_entries(log: &mut [ChangeEntry], sheet: u32, edit: StructureEdit) -> bool {
    let mut changed = false;
    for e in log.iter_mut().filter(|e| e.sheet == sheet && !e.gone) {
        let (pos, from, count) = match edit {
            StructureEdit::DeleteRows { from, count } => (&mut e.row, from, count),
            StructureEdit::DeleteCols { from, count } => (&mut e.col, from, count),
            _ => return false,
        };
        match shift_after_removal(*pos, from, count) {
            Some(p) if p == *pos => {}
            Some(p) => {
                *pos = p;
                changed = true;
            }
            None => {
                e.gone = true;
                changed = true;
            }
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CellRef;
    use crate::config::tests::{assigned, budget};
    use crate::history::{BatchOrigin, ChangeKind};
    use crate::sheet::AddedRow;

    fn entry(row: u32, col: u32) -> ChangeEntry {
        ChangeEntry {
            seq: 1,
            at_ms: 0,
            user: "가".into(),
            batch: "b".into(),
            origin: BatchOrigin::Edit,
            kind: ChangeKind::Input,
            sheet: 0,
            row,
            col,
            old: CellValue::Empty,
            new: CellValue::Number(1.0),
            gone: false,
        }
    }

    #[test]
    fn deletable_rows_follow_header_owner_range_and_merge_rules() {
        let base = budget();
        let mut m = base.clone();
        let cfg = assigned(&m);
        m.append_rows(3);
        let key = cfg.assignment.key_col.unwrap();
        m.cell_mut(CellRef::new(base.rows, key)).unwrap().v = CellValue::Text("김민지".into());
        m.added_rows = vec![
            AddedRow { row: base.rows, by: "김민지".into() },
            AddedRow { row: base.rows + 1, by: "김민지".into() },
            AddedRow { row: base.rows + 2, by: "이서연".into() },
        ];
        let a = Access::compile(&m, &cfg);
        let host = |from, count| check_delete_rows(&m, &cfg, &a, from, count, Actor::Host);
        assert!(host(1, 1).unwrap_err().contains("머리글"));
        assert!(host(3, 1).unwrap_err().contains("담당자(이서연)"), "불러온 행도 담당자가 있으면 지우지 않는다");
        assert!(host(base.rows, 1).unwrap_err().contains("담당자(김민지)"), "담당자 칸에 이름이 있는 행");
        assert!(host(base.rows + 1, 2).is_ok());
        assert!(host(base.rows + 2, 2).unwrap_err().contains("없는 행"));
        let person = |name, from, count| check_delete_rows(&m, &cfg, &a, from, count, Actor::Person(name));
        assert!(person("김민지", base.rows, 2).is_ok(), "내가 추가한 행은 담당자가 나여도 지운다");
        assert!(person("김민지", base.rows, 3).unwrap_err().contains("내가 추가한 행이나 내게 배정된 행만"));
        assert!(person("이서연", base.rows + 2, 1).is_ok());
        // 불러온 행: 내게 배정된 행(C 담당자 = 나)은 지운다. 5·6행(최도윤)은 C5:C6 병합이라 함께.
        assert!(person("최도윤", 4, 2).is_ok());
        assert!(person("최도윤", 4, 1).unwrap_err().contains("병합된 칸(B5:B6)의 첫 행만"));
        assert!(person("최도윤", 5, 1).is_ok(), "병합의 아래 행만 지우는 것은 된다(병합이 줄어든다)");
        assert!(person("김민지", 4, 2).unwrap_err().contains("내게 배정된 행만"));
        assert!(person("이서연", 2, 2).unwrap_err().contains("병합된 칸(A3:A6)"), "A3:A6 병합의 머리를 포함");
    }

    #[test]
    fn imported_rows_without_owner_can_be_deleted_by_the_host() {
        let m = budget();
        let cfg = crate::config::tests::configured(&m); // 행 배정 없음
        let a = Access::compile(&m, &cfg);
        assert!(check_delete_rows(&m, &cfg, &a, 6, 2, Actor::Host).is_ok());
        assert!(check_delete_rows(&m, &cfg, &a, 0, 1, Actor::Host).unwrap_err().contains("머리글"));
        assert!(check_delete_rows(&m, &cfg, &a, 7, 1, Actor::Person("가")).unwrap_err().contains("내게 배정된"));
        assert_eq!(cut_merge(&m, 2, 1), m.merges.iter().find(|r| r.row == 2 && !r.is_single()).copied());
    }

    #[test]
    fn only_cols_added_in_the_app_and_without_assignments_can_be_deleted() {
        let base = budget();
        let mut m = base.clone();
        let mut cfg = assigned(&m);
        m.append_cols(2);
        cfg.on_cols_appended(m.cols);
        let c = base.cols;
        assert!(check_delete_cols(&m, base.cols, &cfg, 0, 1).unwrap_err().contains("엑셀에서 불러온 열"));
        assert!(check_delete_cols(&m, base.cols, &cfg, c, 2).is_ok());
        cfg.assignment.ranges.push(crate::config::RangeAssignment {
            range: CellRange::new(3, c + 1, 2, 1),
            person: "이서연".into(),
            note: String::new(),
        });
        assert!(check_delete_cols(&m, base.cols, &cfg, c, 2).unwrap_err().contains("담당자(이서연)"));
        cfg.assignment.ranges.clear();
        cfg.assignment.key_col = Some(c);
        assert!(check_delete_cols(&m, base.cols, &cfg, c, 1).unwrap_err().contains("기준 열"));
        assert!(check_delete_cols(&m, base.cols, &cfg, c + 1, 1).is_ok());
    }

    #[test]
    fn adding_is_limited() {
        let base = budget();
        let m = base.clone();
        assert_eq!(check_add(&m, &base, StructureEdit::AddRows { count: 99 }), Ok(MAX_ADD));
        assert_eq!(check_add(&m, &base, StructureEdit::AddCols { count: 0 }), Ok(1));
        let mut wide = base.clone();
        wide.append_cols(MAX_ADDED_COLS);
        assert!(check_add(&wide, &base, StructureEdit::AddCols { count: 1 }).is_err());
    }

    #[test]
    fn snapshot_values_follow_structure_edits() {
        let v = |n: &[f64]| n.iter().map(|x| CellValue::Number(*x)).collect::<Vec<_>>();
        let mut values = v(&[1.0, 2.0, 3.0, 4.0]); // 2행 × 2열
        reshape_values(&mut values, 2, StructureEdit::AddCols { count: 1 });
        assert_eq!(values, [v(&[1.0, 2.0]), vec![CellValue::Empty], v(&[3.0, 4.0]), vec![CellValue::Empty]].concat());
        reshape_values(&mut values, 3, StructureEdit::DeleteCols { from: 0, count: 1 });
        assert_eq!(values, [v(&[2.0]), vec![CellValue::Empty], v(&[4.0]), vec![CellValue::Empty]].concat());
        reshape_values(&mut values, 2, StructureEdit::DeleteRows { from: 0, count: 1 });
        assert_eq!(values, [v(&[4.0]), vec![CellValue::Empty]].concat());
        reshape_values(&mut values, 2, StructureEdit::DeleteRows { from: 5, count: 1 });
        assert_eq!(values.len(), 2, "스냅샷에 없는 행은 그대로");
    }

    #[test]
    fn history_of_deleted_lines_is_marked_gone_and_the_rest_moves() {
        let mut log = vec![entry(3, 0), entry(4, 1), entry(5, 2), ChangeEntry { sheet: 1, ..entry(5, 2) }];
        assert!(remap_entries(&mut log, 0, StructureEdit::DeleteRows { from: 4, count: 1 }));
        assert_eq!((log[0].row, log[0].gone), (3, false));
        assert_eq!((log[1].row, log[1].gone), (4, true), "지운 행의 기록은 그 자리에 남긴다");
        assert_eq!((log[2].row, log[2].gone), (4, false));
        assert_eq!(log[3].row, 5, "다른 시트는 그대로");
        assert!(remap_entries(&mut log, 0, StructureEdit::DeleteCols { from: 1, count: 1 }));
        assert_eq!((log[1].col, log[2].col), (1, 1), "지운 기록은 다시 옮기지 않는다");
        assert!(!remap_entries(&mut log, 0, StructureEdit::DeleteRows { from: 9, count: 1 }));
    }
}
