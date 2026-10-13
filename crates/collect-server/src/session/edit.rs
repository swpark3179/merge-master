//! 편집: 셀 잠금, 셀 수정 묶음(권한 → 형식 → 마감 → 적용 → 이력 → 알림), 진행률 증분, 서식, 수식 결과.

use std::time::{SystemTime, UNIX_EPOCH};

use collect_core::access::{Viewer, flag};
use collect_core::config::FormatPolicy;
use collect_core::format::{Axis, FormatEdit, StylePatch, apply_format, resize};
use collect_core::validate::normalize;
use collect_core::{CellRange, CellRef, CellValue};
use tokio::time::Instant;

use super::{ConnId, Lock, RecalcRequest, State};
use collect_core::history::{BatchOrigin, ChangeKind};

use super::HostEvent;
use crate::protocol::{CellEdit, CellUpdate, ChangeEntry, Rejection, ServerMsg, StyleChange};

/// 한 묶음에 받는 최대 칸 수(붙여넣기 1000행 × 20열).
pub const MAX_BATCH: usize = 20_000;
/// 행 이력으로 돌려주는 최대 항목 수(최근 것부터).
const MAX_ROW_HISTORY: usize = 200;
pub(super) const READ_ONLY: &str = "마감되어 입력할 수 없습니다";
const NOT_YOURS: &str = "입력할 수 없는 칸입니다";
/// 클라이언트가 본 뒤 앱에서 붙인 행·열을 지워 칸 좌표가 밀렸다.
pub(super) const STALE_LAYOUT: &str = "표 구조가 바뀌어 반영하지 못했습니다. 바뀐 표에서 다시 입력해 주세요";

pub(super) fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

impl State {
    fn idx(&self, sheet: usize, at: CellRef) -> usize {
        (at.row * self.sheets[sheet].model.cols + at.col) as usize
    }

    fn at_of(&self, sheet: usize, i: usize) -> CellRef {
        self.rt[sheet].cell_of(i, self.sheets[sheet].model.cols)
    }

    fn in_bounds(&self, sheet: usize, at: CellRef) -> bool {
        let m = &self.sheets[sheet].model;
        at.row < m.rows && at.col < m.cols
    }

    /// 클라이언트가 본 구조 판이 지금과 다르다(없으면 옛 화면이라 검사하지 않는다).
    pub(super) fn stale_layout(&self, sheet: usize, layout: Option<u32>) -> bool {
        layout.is_some_and(|l| l != self.sheets[sheet].model.layout)
    }

    /// 그 칸이 보이는 사람에게만 보낸다(`except` 빼고).
    fn send_where_visible(&self, sheet: usize, at: CellRef, msg: &ServerMsg, except: Option<ConnId>) {
        for (id, p) in self.attached() {
            if Some(id) != except && self.row_visible(sheet, p, at.row) {
                self.send(id, msg.clone());
            }
        }
    }

    // ------------------------------------------------------------ 잠금

    pub(super) fn lock(&mut self, conn: ConnId, sheet: u32, at: CellRef, layout: Option<u32>) {
        let Some((name, p)) = self.person_of(conn) else { return };
        self.touch(conn);
        let refuse = |by: Option<String>, reason: Option<&str>| ServerMsg::LockResult {
            sheet,
            at,
            ok: false,
            by,
            reason: reason.map(String::from),
        };
        if self.read_only {
            return self.send(conn, refuse(None, Some(READ_ONLY)));
        }
        let Some(si) = self.sheet_index(sheet) else { return self.send(conn, refuse(None, Some(NOT_YOURS))) };
        if self.stale_layout(si, layout) {
            return self.send(conn, refuse(None, Some(STALE_LAYOUT)));
        }
        if !self.in_bounds(si, at) || !self.rt[si].access.can_edit(Viewer::Person(p), at) {
            return self.send(conn, refuse(None, Some(NOT_YOURS)));
        }
        let i = self.idx(si, at);
        if let Some(l) = self.rt[si].locks.get(&i)
            && l.owner != name
        {
            return self.send(conn, refuse(Some(l.owner.clone()), None));
        }
        // 한 사람은 한 칸만 편집한다(시트를 넘어서도): 앞서 잡은 다른 칸은 푼다.
        let others: Vec<(usize, usize)> = self
            .rt
            .iter()
            .enumerate()
            .flat_map(|(s, rt)| rt.locks.iter().map(move |(j, l)| (s, *j, l)))
            .filter(|(s, j, l)| (*s, *j) != (si, i) && l.owner == name)
            .map(|(s, j, _)| (s, j))
            .collect();
        for (s, j) in others {
            self.release(s, j);
        }
        let lock = Lock { owner: name.clone(), since: Instant::now(), orphaned: None };
        let fresh = self.rt[si].locks.insert(i, lock).is_none();
        self.send(conn, ServerMsg::LockResult { sheet, at, ok: true, by: None, reason: None });
        if fresh {
            self.send_where_visible(si, at, &ServerMsg::LockChanged { sheet, at, by: Some(name) }, Some(conn));
        }
    }

    pub(super) fn unlock(&mut self, conn: ConnId, sheet: u32, at: CellRef) {
        let Some((name, _)) = self.person_of(conn) else { return };
        let Some(si) = self.sheet_index(sheet) else { return };
        if !self.in_bounds(si, at) {
            return;
        }
        let i = self.idx(si, at);
        if self.rt[si].locks.get(&i).is_some_and(|l| l.owner == name) {
            self.release(si, i);
        }
    }

    /// 잠금 하나를 풀고 알린다(잡았던 사람에게도: 호스트가 풀었으면 편집기를 닫아야 한다).
    fn release(&mut self, sheet: usize, i: usize) {
        if self.rt[sheet].locks.remove(&i).is_some() {
            let at = self.at_of(sheet, i);
            let msg = ServerMsg::LockChanged { sheet: sheet as u32, at, by: None };
            self.send_where_visible(sheet, at, &msg, None);
        }
    }

    pub(super) fn release_locks_of(&mut self, name: &str) -> usize {
        let mine: Vec<(usize, usize)> = self
            .rt
            .iter()
            .enumerate()
            .flat_map(|(s, rt)| rt.locks.iter().filter(|(_, l)| l.owner == name).map(move |(i, _)| (s, *i)))
            .collect();
        for &(s, i) in &mine {
            self.release(s, i);
        }
        mine.len()
    }

    pub(super) fn host_unlock(&mut self, name: &str) -> Result<(), String> {
        if self.person(name).is_none() {
            return Err("명단에 없는 이름입니다".into());
        }
        self.release_locks_of(name.trim());
        Ok(())
    }

    /// 끊긴 지 [`LOCK_GRACE`](super::LOCK_GRACE)가 지났거나 [`LOCK_MAX`](super::LOCK_MAX)보다 오래 잡은 잠금을 푼다.
    pub(super) fn expire_locks(&mut self) {
        let stale: Vec<(usize, usize)> = self
            .rt
            .iter()
            .enumerate()
            .flat_map(|(s, rt)| {
                rt.locks
                    .iter()
                    .filter(|(_, l)| {
                        l.orphaned.is_some_and(|t| t.elapsed() >= super::LOCK_GRACE)
                            || l.since.elapsed() >= super::LOCK_MAX
                    })
                    .map(move |(i, _)| (s, *i))
            })
            .collect();
        for (s, i) in stale {
            self.release(s, i);
        }
    }

    // ------------------------------------------------------------ 셀 수정

    pub(super) fn set_cells(
        &mut self,
        conn: ConnId,
        sheet: u32,
        batch: String,
        cells: Vec<CellEdit>,
        origin: BatchOrigin,
        layout: Option<u32>,
    ) {
        // 참여자는 복원·'Excel 값 유지'를 할 수 없다(표시만 바뀌는 값이라 일반 입력으로 본다).
        let origin = match origin {
            BatchOrigin::Restore | BatchOrigin::Excel | BatchOrigin::AddRow => BatchOrigin::Edit,
            o => o,
        };
        let Some((name, p)) = self.person_of(conn) else { return };
        self.touch(conn);
        if batch.is_empty() || batch.chars().count() > super::MAX_BATCH_ID_CHARS {
            return self.send(conn, ServerMsg::Error { message: "묶음 ID가 잘못되었습니다".into() });
        }
        if cells.len() > MAX_BATCH {
            let message = format!("한 번에 {MAX_BATCH}칸까지 바꿀 수 있습니다");
            return self.send(conn, ServerMsg::Error { message });
        }
        let Some(si) = self.sheet_index(sheet) else {
            return self.send(conn, ServerMsg::Error { message: super::NO_SHEET.into() });
        };
        // 묶음을 만든 뒤 앱에서 붙인 행·열을 지웠다: 좌표가 밀렸을 수 있어 묶음 전체를 받지 않는다(반영하지 않았으니
        // 같은 묶음으로 다시 보내도 된다). 거절 칸은 그 좌표의 지금 값으로 돌려준다.
        if self.stale_layout(si, layout) {
            let rejected = cells
                .iter()
                .map(|e| CellRef::new(e.row, e.col))
                .filter(|&at| self.in_bounds(si, at))
                .map(|at| {
                    let u = self.update_of(si, at);
                    Rejection { row: at.row, col: at.col, reason: STALE_LAYOUT.into(), value: u.value, ver: u.ver }
                })
                .collect();
            return self.send(conn, ServerMsg::Applied { sheet, batch, cells: vec![], conflicts: vec![], rejected });
        }
        // 다시 연결한 뒤 같은 묶음을 또 보냈다(오프라인 대기열): 이미 반영했으니 지금 값만 돌려준다.
        let key = super::persist::applied_key(&name, &batch);
        if !self.applied.insert(key) {
            let cells = cells
                .iter()
                .map(|e| CellRef::new(e.row, e.col))
                .filter(|&at| self.in_bounds(si, at))
                .map(|at| self.update_of(si, at))
                .collect();
            return self.send(conn, ServerMsg::Applied { sheet, batch, cells, conflicts: vec![], rejected: vec![] });
        }

        let at_ms = now_ms();
        let logged = self.log.len();
        let mut applied = Vec::new();
        let mut changed = Vec::new();
        let mut conflicts = Vec::new();
        let mut rejected = Vec::new();
        let mut touched_people = Vec::new();
        for edit in cells {
            let at = CellRef::new(edit.row, edit.col);
            if !self.in_bounds(si, at) {
                continue;
            }
            let i = self.idx(si, at);
            let reject = |s: &State, reason: String| {
                let u = s.update_of(si, at);
                Rejection { row: at.row, col: at.col, reason, value: u.value, ver: u.ver }
            };
            if self.read_only {
                rejected.push(reject(self, READ_ONLY.into()));
                continue;
            }
            if !self.rt[si].access.can_edit(Viewer::Person(p), at) {
                rejected.push(reject(self, NOT_YOURS.into()));
                continue;
            }
            if let Some(l) = self.rt[si].locks.get(&i).filter(|l| l.owner != name) {
                let reason = format!("{} 님이 입력 중입니다", l.owner);
                rejected.push(reject(self, reason));
                continue;
            }
            let value = match self.sheets[si].config.columns.get(at.col as usize).map(|c| &c.format) {
                Some(format) => match normalize(format, &edit.value) {
                    Ok(v) => v,
                    Err(e) => {
                        rejected.push(reject(self, e.0));
                        continue;
                    }
                },
                None => edit.value,
            };
            let old = self.sheets[si].model.cell(at).map(|c| c.v.clone()).unwrap_or_default();
            if old == value {
                applied.push(self.update_of(si, at));
                continue;
            }
            // 내가 본 뒤 다른 사람이 바꿨다: 나중 입력(내 값)으로 덮어쓰고 알려 준다.
            let rt = &mut self.rt[si];
            if edit.base < rt.versions[i] && rt.writer.get(&i).is_some_and(|w| *w != name) {
                conflicts.push(at);
            }
            rt.versions[i] += 1;
            rt.writer.insert(i, name.clone());
            let ver = rt.versions[i];
            if let Some(cell) = self.sheets[si].model.cell_mut(at) {
                cell.v = value.clone();
            }
            self.log.push(ChangeEntry {
                seq: self.next_seq,
                at_ms,
                user: name.clone(),
                batch: batch.clone(),
                origin,
                kind: ChangeKind::of(origin, &old, &value),
                sheet,
                row: at.row,
                col: at.col,
                old: old.clone(),
                new: value.clone(),
                gone: false,
            });
            self.next_seq += 1;
            self.count_progress(si, at, &old, &value, &mut touched_people);
            let u = CellUpdate { row: at.row, col: at.col, value, ver };
            applied.push(u.clone());
            changed.push(u);
        }

        // 입력을 끝냈으니 그 칸의 잠금은 푼다.
        let held: Vec<usize> = changed
            .iter()
            .chain(applied.iter())
            .map(|u| self.idx(si, CellRef::new(u.row, u.col)))
            .filter(|i| self.rt[si].locks.get(i).is_some_and(|l| l.owner == name))
            .collect();
        for i in held {
            self.release(si, i);
        }

        if let Some(store) = &self.store {
            store.append(&self.log[logged..]);
        }
        self.send(conn, ServerMsg::Applied { sheet, batch, cells: applied, conflicts, rejected });
        self.broadcast_cells(si, &name, changed.clone(), Some(conn));
        self.send_progress(si, &touched_people);
        self.request_recalc(si, changed);
    }

    /// 바뀐 입력 칸을 수식 재계산에 넘긴다(어느 시트에든 수식이 있고 재계산을 맡은 곳이 있을 때 — 수식은 다른 시트를
    /// 볼 수 있다). 계산 사본에 없는 행·열(앱에서 붙인 것)은 빼고 넘긴다.
    fn request_recalc(&mut self, sheet: usize, changed: Vec<CellUpdate>) {
        let (rows, cols) = (self.rt[sheet].recalc_rows, self.rt[sheet].recalc_cols);
        if self.any_formulas
            && !changed.is_empty()
            && let Some(tx) = &self.recalc
        {
            let writes: Vec<(u32, CellRef, CellValue)> = changed
                .into_iter()
                .filter(|u| u.row < rows && u.col < cols)
                .map(|u| (sheet as u32, CellRef::new(u.row, u.col), u.value))
                .collect();
            if writes.is_empty() {
                return;
            }
            if tx.send(RecalcRequest { writes }).is_err() {
                self.recalc = None;
            }
        }
    }

    /// 호스트가 바꾼 칸([`Shared::host_set`](super::Shared::host_set)): 권한·형식 검사 없이 반영하고 모두에게 알린다.
    pub(super) fn host_writes(
        &mut self,
        user: &str,
        origin: BatchOrigin,
        sheet: usize,
        values: Vec<(CellRef, CellValue)>,
    ) {
        if sheet >= self.sheets.len() {
            return;
        }
        let at_ms = now_ms();
        let logged = self.log.len();
        let batch = format!("host-{}", self.next_seq);
        let mut changed = Vec::new();
        let mut touched = Vec::new();
        for (at, value) in values {
            if !self.in_bounds(sheet, at) {
                continue;
            }
            let model = &mut self.sheets[sheet].model;
            let Some(cell) = model.cell_mut(at).filter(|c| c.f.is_none() && c.v != value) else { continue };
            let old = std::mem::replace(&mut cell.v, value.clone());
            let i = self.idx(sheet, at);
            let rt = &mut self.rt[sheet];
            rt.versions[i] += 1;
            rt.writer.insert(i, user.to_string());
            let ver = rt.versions[i];
            self.log.push(ChangeEntry {
                seq: self.next_seq,
                at_ms,
                user: user.to_string(),
                batch: batch.clone(),
                origin,
                kind: ChangeKind::of(origin, &old, &value),
                sheet: sheet as u32,
                row: at.row,
                col: at.col,
                old: old.clone(),
                new: value.clone(),
                gone: false,
            });
            self.next_seq += 1;
            self.count_progress(sheet, at, &old, &value, &mut touched);
            changed.push(CellUpdate { row: at.row, col: at.col, value, ver });
        }
        if let Some(store) = &self.store {
            store.append(&self.log[logged..]);
        }
        self.broadcast_cells(sheet, user, changed.clone(), None);
        self.send_progress(sheet, &touched);
        self.request_recalc(sheet, changed);
    }

    fn update_of(&self, sheet: usize, at: CellRef) -> CellUpdate {
        let value = self.sheets[sheet].model.cell(at).map(|c| c.v.clone()).unwrap_or_default();
        CellUpdate { row: at.row, col: at.col, value, ver: self.rt[sheet].versions[self.idx(sheet, at)] }
    }

    /// 바뀐 칸을 다른 사람에게 보낸다('남의 행 숨김'이면 그 행이 보이는 사람에게만).
    fn broadcast_cells(&self, sheet: usize, by: &str, cells: Vec<CellUpdate>, except: Option<ConnId>) {
        if cells.is_empty() {
            return;
        }
        let s = sheet as u32;
        self.feed(HostEvent::Cells { sheet: s, by: by.to_string(), cells: cells.clone() });
        let all = ServerMsg::Cells { sheet: s, by: by.to_string(), cells: cells.clone() };
        for (id, p) in self.attached() {
            if Some(id) == except {
                continue;
            }
            if !self.rt[sheet].visible.contains_key(&p) {
                self.send(id, all.clone());
                continue;
            }
            let mine: Vec<CellUpdate> = cells.iter().filter(|u| self.row_visible(sheet, p, u.row)).cloned().collect();
            if !mine.is_empty() {
                self.send(id, ServerMsg::Cells { sheet: s, by: by.to_string(), cells: mine });
            }
        }
    }

    /// 필수 칸이 채워지거나 비워졌으면 진행률을 고친다(전체·열·담당자).
    fn count_progress(
        &mut self,
        sheet: usize,
        at: CellRef,
        old: &CellValue,
        new: &CellValue,
        touched: &mut Vec<usize>,
    ) {
        let rt = &mut self.rt[sheet];
        if rt.access.flags(at) & flag::REQUIRED == 0 || old.is_empty() == new.is_empty() {
            return;
        }
        let up = !new.is_empty();
        let bump = |n: &mut u32| *n = if up { *n + 1 } else { n.saturating_sub(1) };
        bump(&mut rt.progress.filled);
        if let Some(c) = rt.progress.by_column.get_mut(at.col as usize) {
            bump(&mut c.filled);
        }
        for p in rt.access.owners(at) {
            if let Some(pp) = rt.progress.by_person.get_mut(p) {
                bump(&mut pp.filled);
            }
            if !touched.contains(&p) {
                touched.push(p);
            }
        }
    }

    fn send_progress(&self, sheet: usize, people: &[usize]) {
        for (id, p) in self.attached() {
            if people.contains(&p)
                && let Some(progress) = self.rt[sheet].progress.by_person.get(p)
            {
                self.send(id, ServerMsg::Progress { sheet: sheet as u32, progress: progress.clone() });
            }
        }
    }

    // ------------------------------------------------------------ 이력·서식·수식

    pub(super) fn row_history(&mut self, conn: ConnId, sheet: u32, row: u32) {
        let Some((_, p)) = self.person_of(conn) else { return };
        let visible =
            self.sheet_index(sheet).is_some_and(|si| row < self.sheets[si].model.rows && self.row_visible(si, p, row));
        if !visible {
            return self.send(conn, ServerMsg::History { sheet, row, entries: vec![] });
        }
        let mut entries: Vec<ChangeEntry> = self
            .log
            .iter()
            .rev()
            .filter(|e| e.sheet == sheet && e.row == row && !e.gone)
            .take(MAX_ROW_HISTORY)
            .cloned()
            .collect();
        entries.reverse();
        self.send(conn, ServerMsg::History { sheet, row, entries });
    }

    pub(super) fn format(
        &mut self,
        conn: ConnId,
        sheet: u32,
        range: CellRange,
        patch: StylePatch,
        layout: Option<u32>,
    ) {
        let Some((_, p)) = self.person_of(conn) else { return };
        self.touch(conn);
        let error = |s: &State, message: &str| s.send(conn, ServerMsg::Error { message: message.into() });
        let Some(si) = self.sheet_index(sheet) else { return error(self, super::NO_SHEET) };
        let fill_only = StylePatch { fill: patch.fill, ..Default::default() };
        match self.sheets[si].config.permissions.format {
            FormatPolicy::None => return error(self, "이 시트에서는 서식을 바꿀 수 없습니다"),
            FormatPolicy::FillOnly if patch != fill_only => {
                return error(self, "이 시트에서는 배경색만 바꿀 수 있습니다");
            }
            _ => {}
        }
        if self.read_only {
            return error(self, READ_ONLY);
        }
        if self.stale_layout(si, layout) {
            return error(self, STALE_LAYOUT);
        }
        let bounds = self.sheets[si].model.bounds();
        let Some(range) = range.intersect(&bounds) else { return };
        // 병합에 덮인 칸(값이 없다)은 병합 머리 칸을 따른다.
        let access = &self.rt[si].access;
        if range.cells().any(|at| access.flags(at) & flag::COVERED == 0 && !access.can_edit(Viewer::Person(p), at)) {
            return error(self, "내가 입력할 수 있는 칸만 서식을 바꿀 수 있습니다");
        }
        // 테두리 없애기는 범위 바깥 이웃 칸도 고칠 수 있어 한 칸 넓혀 비교한다.
        let (r0, c0) = (range.row.saturating_sub(1), range.col.saturating_sub(1));
        let wide = CellRange::new(r0, c0, range.last_row() + 2 - r0, range.last_col() + 2 - c0)
            .intersect(&bounds)
            .unwrap_or(range);
        let model = &mut self.sheets[si].model;
        let before: Vec<u32> = wide.cells().map(|at| model.cell(at).map_or(0, |c| c.s)).collect();
        if let Err(e) = apply_format(model, FormatEdit::Style { range, patch }) {
            return error(self, &e.to_string());
        }
        let changes: Vec<StyleChange> = wide
            .cells()
            .zip(before)
            .filter_map(|(at, old)| {
                let s = model.cell(at)?.s;
                (s != old).then_some(StyleChange { row: at.row, col: at.col, s })
            })
            .collect();
        let styles = model.styles.clone();
        self.persist_sheet();
        self.feed(HostEvent::Styles { sheet, styles: styles.clone(), changes: changes.clone() });
        self.broadcast(&ServerMsg::Styles { sheet, styles, changes }, None);
    }

    /// 참여자가 열 너비·행 높이를 바꾼다(그 시트의 참여자 권한 '크기 조절'). 행은 그 사람에게 보이는 행만(남의 행 숨김).
    /// 모두에게 `resized`를 보낸다(보낸 사람도 서버가 맞춘 크기로 그린다).
    pub(super) fn resize(&mut self, conn: ConnId, sheet: u32, axis: Axis, from: u32, count: u32, size: f32) {
        let Some((name, p)) = self.person_of(conn) else { return };
        self.touch(conn);
        let error = |s: &State, message: &str| s.send(conn, ServerMsg::Error { message: message.into() });
        let Some(si) = self.sheet_index(sheet) else { return error(self, super::NO_SHEET) };
        if !self.sheets[si].config.permissions.resize {
            return error(self, "이 시트에서는 열 너비·행 높이를 바꿀 수 없습니다");
        }
        if self.read_only {
            return error(self, READ_ONLY);
        }
        let model = &self.sheets[si].model;
        let end = from.saturating_add(count.max(1)).min(if axis == Axis::Col { model.cols } else { model.rows });
        if axis == Axis::Row && (from..end).any(|r| !self.row_visible(si, p, r)) {
            return error(self, "보이는 행만 높이를 바꿀 수 있습니다");
        }
        match resize(&mut self.sheets[si].model, axis, from, count, size) {
            Ok(size) => self.resized(si, &name, axis, from, count, size),
            Err(e) => error(self, &e.to_string()),
        }
    }

    /// 열 너비·행 높이가 바뀌었다(시트에는 이미 반영): 시트를 남기고 호스트 화면과 모든 참여자에게 알린다.
    pub(super) fn resized(&mut self, sheet: usize, by: &str, axis: Axis, from: u32, count: u32, size: f32) {
        let sheet = sheet as u32;
        self.persist_sheet();
        self.feed(HostEvent::Resized { sheet, axis, from, count, size });
        self.broadcast(&ServerMsg::Resized { by: by.to_string(), sheet, axis, from, count, size }, None);
    }

    /// 호스트(Excel)가 다시 계산한 수식 칸. 바뀐 칸만 시트마다 보낸다(이력에는 남기지 않는다).
    pub(super) fn formulas(&mut self, values: Vec<(u32, CellRef, CellValue)>) {
        let mut changed: Vec<Vec<CellUpdate>> = vec![Vec::new(); self.sheets.len()];
        for (sheet, at, v) in values {
            let Some(si) = self.sheet_index(sheet) else { continue };
            if !self.in_bounds(si, at) {
                continue;
            }
            let i = self.idx(si, at);
            let Some(cell) = self.sheets[si].model.cell_mut(at).filter(|c| c.f.is_some() && c.v != v) else {
                continue;
            };
            cell.v = v.clone();
            self.rt[si].versions[i] += 1;
            changed[si].push(CellUpdate { row: at.row, col: at.col, value: v, ver: self.rt[si].versions[i] });
        }
        for (si, cells) in changed.into_iter().enumerate() {
            self.broadcast_cells(si, "", cells, None);
        }
    }
}
