//! 표 구조 바꾸기: 표 아래에 행·오른쪽에 열 붙이기, 앱에서 붙인 행·열 지우기, 행 취소 표시.
//!
//! 판정은 [`collect_core::structure`]가 하고, 여기서는 시트·설정과 함께 세션 상태(칸 버전·잠금·마지막 작성자, 변경 기록,
//! 스냅샷, 강조, 커서)를 맞춘 뒤 모두에게 새 시트를 보낸다. 붙인 행·열만 지우므로 엑셀에서 불러온 칸의 좌표와 기준
//! 시트·계산 사본은 그대로다.

use std::collections::HashMap;

use collect_core::a1::shift_after_removal;
use collect_core::config::{CellException, ColumnRole, ExceptionKind};
use collect_core::format::Axis;
use collect_core::history::{BatchOrigin, ChangeKind};
use collect_core::structure::{Actor, StructureEdit, check_add, check_delete_rows, remap_entries, reshape_values};
use collect_core::{AddedRow, CellRange, CellRef, CellValue, SheetCell};
use collect_store::{pack_sheets, unpack_sheets};

use super::edit::{READ_ONLY, STALE_LAYOUT, now_ms};
use super::{ConnId, HostEvent, Lock, State};
use crate::protocol::{ChangeEntry, ServerMsg};

/// 참여자가 추가한 행에 거는 셀 예외의 메모(호스트 1b '셀 예외'에 보인다).
pub const ADDED_ROWS_NOTE: &str = "참여자가 추가한 행";

impl State {
    /// 참여자 요청: 행 추가(권한 '행 추가'), 내가 추가한 행 지우기(권한 '행 삭제'), 취소 표시(내가 입력할 수 있는 칸이
    /// 있는 행). 열은 호스트만 바꾼다.
    pub(super) fn participant_structure(&mut self, conn: ConnId, sheet: u32, edit: StructureEdit, layout: Option<u32>) {
        let Some((name, p)) = self.person_of(conn) else { return };
        self.touch(conn);
        let error = |s: &State, message: String| s.send(conn, ServerMsg::Error { message });
        let Some(si) = self.sheet_index(sheet) else { return error(self, super::NO_SHEET.into()) };
        if self.read_only {
            return error(self, READ_ONLY.into());
        }
        if self.stale_layout(si, layout) {
            return error(self, STALE_LAYOUT.into());
        }
        let slot = &self.sheets[si];
        let permissions = slot.config.permissions;
        let checked = match edit {
            StructureEdit::AddRows { .. } if !permissions.add_rows => {
                Err("이 시트에서는 행을 추가할 수 없습니다".to_string())
            }
            StructureEdit::AddRows { .. } => check_add(&slot.model, &slot.baseline, edit).map(drop),
            StructureEdit::DeleteRows { .. } if !permissions.delete_rows => {
                Err("이 시트에서는 행을 지울 수 없습니다".to_string())
            }
            StructureEdit::DeleteRows { from, count } => {
                let access = &self.rt[si].access;
                check_delete_rows(
                    &slot.model,
                    slot.baseline.rows,
                    &slot.config,
                    access,
                    from,
                    count,
                    Actor::Person(&name),
                )
            }
            StructureEdit::CancelRows { from, count, .. } => {
                let model = &slot.model;
                let end = from.saturating_add(count);
                if count == 0 || end > model.rows || from < model.header_rows {
                    Err("데이터 행만 취소 표시할 수 있습니다".to_string())
                } else if (from..end).any(|r| !self.row_visible(si, p, r) || !self.can_edit_row(si, p, r)) {
                    Err("내가 입력할 수 있는 행만 취소 표시할 수 있습니다".to_string())
                } else {
                    Ok(())
                }
            }
            StructureEdit::AddCols { .. } | StructureEdit::DeleteCols { .. } => {
                Err("열은 호스트만 추가하거나 지울 수 있습니다".to_string())
            }
        };
        match checked {
            Ok(()) => self.apply_structure(si, edit, &name, Some(p)),
            Err(message) => error(self, message),
        }
    }

    /// 그 사람이 그 행에서 입력할 수 있는 칸이 있다.
    fn can_edit_row(&self, sheet: usize, p: usize, row: u32) -> bool {
        let access = &self.rt[sheet].access;
        (0..self.sheets[sheet].model.cols)
            .any(|c| access.can_edit(collect_core::access::Viewer::Person(p), CellRef::new(row, c)))
    }

    /// 검사를 마친 구조 바꾸기를 한다. `person`은 참여자가 했을 때 그 사람(명단 인덱스).
    pub(super) fn apply_structure(&mut self, si: usize, edit: StructureEdit, by: &str, person: Option<usize>) {
        let sheet = si as u32;
        match edit {
            StructureEdit::AddRows { .. } => {
                let Ok(count) = check_add(&self.sheets[si].model, &self.sheets[si].baseline, edit) else { return };
                let range = self.sheets[si].model.append_rows(count);
                let model = &mut self.sheets[si].model;
                model.added_rows.extend((range.row..=range.last_row()).map(|row| AddedRow { row, by: by.to_string() }));
                let cells = model.cells.len();
                self.rt[si].versions.resize(cells, 0);
                if person.is_some() {
                    self.open_added_rows(si, range);
                    self.write_adder(si, range, by);
                }
                self.restructured(si, true);
                self.broadcast(&ServerMsg::RowsAdded { by: by.to_string(), sheet, range }, None);
            }
            StructureEdit::AddCols { .. } => {
                let Ok(count) = check_add(&self.sheets[si].model, &self.sheets[si].baseline, edit) else { return };
                let before = self.sheets[si].model.cols;
                let slot = &mut self.sheets[si];
                slot.model.append_cols(count);
                slot.config.on_cols_appended(slot.model.cols);
                let after = slot.model.cols;
                self.reindex(si, before, after, Some);
                self.reshape_snapshots(si, before, StructureEdit::AddCols { count });
                self.restructured(si, true);
            }
            StructureEdit::DeleteRows { from, count } => {
                let cols = self.sheets[si].model.cols;
                let slot = &mut self.sheets[si];
                slot.model.remove_rows(from, count);
                slot.config.on_rows_removed(from, count);
                self.reindex(si, cols, cols, |at| {
                    Some(CellRef::new(shift_after_removal(at.row, from, count)?, at.col))
                });
                self.reshape_snapshots(si, cols, edit);
                self.remap_log(si, Axis::Row, from, count, edit);
                self.remap_highlights(si, |r| r.without_rows(from, count));
                self.move_cursors(si, |at| Some(CellRef::new(shift_after_removal(at.row, from, count)?, at.col)));
                self.restructured(si, true);
                self.broadcast(&ServerMsg::RowsDeleted { by: by.to_string(), sheet, from, count }, None);
            }
            StructureEdit::DeleteCols { from, count } => {
                let before = self.sheets[si].model.cols;
                let slot = &mut self.sheets[si];
                slot.model.remove_cols(from, count);
                slot.config.on_cols_removed(from, count);
                let after = slot.model.cols;
                self.reindex(si, before, after, |at| {
                    Some(CellRef::new(at.row, shift_after_removal(at.col, from, count)?))
                });
                self.reshape_snapshots(si, before, edit);
                self.remap_log(si, Axis::Col, from, count, edit);
                self.remap_highlights(si, |r| r.without_cols(from, count));
                self.move_cursors(si, |at| Some(CellRef::new(at.row, shift_after_removal(at.col, from, count)?)));
                self.restructured(si, true);
            }
            StructureEdit::CancelRows { from, count, on } => {
                if self.sheets[si].model.set_cancelled(from, count, on) {
                    self.restructured(si, false);
                }
            }
        }
    }

    /// 시트가 바뀐 뒤: 권한·진행률을 다시 세고, 모두에게 새 시트를 보내고, 호스트 화면이 다시 읽게 하고, 파일에 남긴다.
    fn restructured(&mut self, _sheet: usize, config: bool) {
        self.recompile();
        self.resync();
        self.feed(HostEvent::Reset);
        self.persist_sheet();
        if config {
            self.persist(true); // 설정(셀 예외·범위 배정·열 규칙)과 강조도 남긴다
        }
    }

    /// 칸 버전·편집 잠금·마지막 작성자를 새 좌표로 옮긴다(평면 인덱스 = 행 × 열 수 + 열). `map`이 `None`인 칸(지운 칸)은
    /// 버린다 — 잠금도 풀린다(새 시트를 받은 편집기는 닫힌다).
    fn reindex(&mut self, si: usize, old_cols: u32, new_cols: u32, map: impl Fn(CellRef) -> Option<CellRef>) {
        let (old_cols, new_cols) = (old_cols.max(1), new_cols.max(1));
        let cells = self.sheets[si].model.cells.len();
        let at_of = |i: usize| CellRef::new(i as u32 / old_cols, i as u32 % old_cols);
        let index = |at: CellRef| (at.row * new_cols + at.col) as usize;
        let rt = &mut self.rt[si];
        let mut versions = vec![0; cells];
        for (i, v) in rt.versions.iter().enumerate().filter(|(_, v)| **v > 0) {
            if let Some(j) = map(at_of(i)).map(index).filter(|&j| j < cells) {
                versions[j] = *v;
            }
        }
        rt.versions = versions;
        let locks: HashMap<usize, Lock> = std::mem::take(&mut rt.locks)
            .into_iter()
            .filter_map(|(i, l)| Some((map(at_of(i)).map(index).filter(|&j| j < cells)?, l)))
            .collect();
        rt.locks = locks;
        let writer: HashMap<usize, String> = std::mem::take(&mut rt.writer)
            .into_iter()
            .filter_map(|(i, w)| Some((map(at_of(i)).map(index).filter(|&j| j < cells)?, w)))
            .collect();
        rt.writer = writer;
    }

    /// 스냅샷 값을 지금 시트 모양에 맞춘다(메모리와 파일). `cols`는 바꾸기 전 열 수.
    fn reshape_snapshots(&mut self, si: usize, cols: u32, edit: StructureEdit) {
        for (meta, data) in &mut self.snapshots.list {
            let Ok(mut sheets) = unpack_sheets(data) else { continue };
            let Some(values) = sheets.get_mut(si) else { continue };
            reshape_values(values, cols, edit);
            *data = pack_sheets(&sheets);
            if let Some(store) = &self.store {
                store.replace_snapshot(meta.id, data.clone());
            }
        }
    }

    /// 변경 기록: 지운 줄의 기록은 `gone`, 뒤의 기록은 당긴다(메모리와 파일).
    fn remap_log(&mut self, si: usize, axis: Axis, from: u32, count: u32, edit: StructureEdit) {
        if remap_entries(&mut self.log, si as u32, edit)
            && let Some(store) = &self.store
        {
            store.remap(si as u32, axis, from, count);
        }
    }

    /// 다른 사람 커서를 새 좌표로(지운 칸이면 지운다). 새 위치는 다음 커서 이동·접속 상태에 실린다.
    fn move_cursors(&mut self, si: usize, map: impl Fn(CellRef) -> Option<CellRef>) {
        for p in self.people.values_mut() {
            if let Some(c) = p.cursor.filter(|c| c.sheet as usize == si) {
                p.cursor = map(c.at()).map(|at| SheetCell::new(c.sheet, at));
            }
        }
    }

    /// 참여자가 추가한 행을 입력 칸으로 여는 셀 예외(입력 가능 + 행 배정 기준 칸·잠금 열의 수식 칸은 입력 금지). 바로 위에
    /// 이어지는 같은 예외가 있으면 그 범위를 늘린다(행을 여러 번 추가해도 예외는 늘지 않는다).
    fn open_added_rows(&mut self, sheet: usize, range: CellRange) {
        let slot = &mut self.sheets[sheet];
        let key = slot.config.assignment.key_col.filter(|&k| k < slot.model.cols);
        // 잠금 열의 수식(위 행에서 내려 쓴 것)은 새 행에서도 Excel이 계산하게 둔다.
        let formula_locked: Vec<u32> = (0..slot.model.cols)
            .filter(|&c| {
                slot.config.columns.get(c as usize).is_some_and(|r| r.role == ColumnRole::Locked)
                    && slot.model.cell(CellRef::new(range.row, c)).is_some_and(|cell| cell.f.is_some())
            })
            .collect();
        let mut add = |kind: ExceptionKind, r: CellRange| {
            let list = &mut slot.config.exceptions;
            let prev = list.iter_mut().rev().find(|e| {
                e.kind == kind
                    && e.note == ADDED_ROWS_NOTE
                    && e.range.col == r.col
                    && e.range.cols == r.cols
                    && e.range.last_row() + 1 == r.row
            });
            match prev {
                Some(e) => e.range.rows += r.rows,
                None => list.push(CellException { range: r, kind, note: ADDED_ROWS_NOTE.into() }),
            }
        };
        add(ExceptionKind::Input, range);
        if let Some(key) = key {
            add(ExceptionKind::Locked, CellRange::new(range.row, key, range.rows, 1));
        }
        for c in formula_locked.into_iter().filter(|&c| Some(c) != key) {
            add(ExceptionKind::Locked, CellRange::new(range.row, c, range.rows, 1));
        }
    }

    /// 참여자가 추가한 행: 행 배정 기준 열이 있으면 그 칸에 추가한 사람 이름을 넣어 그 사람의 행으로 둔다(기록 출처
    /// '행 추가'). 없으면 담당자 없는 행(누구나 입력)이다.
    fn write_adder(&mut self, si: usize, range: CellRange, name: &str) {
        let cols = self.sheets[si].model.cols;
        let Some(key) = self.sheets[si].config.assignment.key_col.filter(|&k| k < cols) else { return };
        let at_ms = now_ms();
        let logged = self.log.len();
        let batch = format!("rows-{}", self.next_seq);
        let value = CellValue::Text(name.to_string());
        for row in range.row..=range.last_row() {
            let at = CellRef::new(row, key);
            let Some(cell) = self.sheets[si].model.cell_mut(at).filter(|c| c.f.is_none()) else { continue };
            cell.v = value.clone();
            let i = (row * cols + key) as usize;
            self.rt[si].versions[i] += 1;
            self.rt[si].writer.insert(i, name.to_string());
            self.log.push(ChangeEntry {
                seq: self.next_seq,
                at_ms,
                user: name.to_string(),
                batch: batch.clone(),
                origin: BatchOrigin::AddRow,
                kind: ChangeKind::Input,
                sheet: si as u32,
                row,
                col: key,
                old: CellValue::Empty,
                new: value.clone(),
                gone: false,
            });
            self.next_seq += 1;
        }
        if let Some(store) = &self.store {
            store.append(&self.log[logged..]);
        }
    }
}
