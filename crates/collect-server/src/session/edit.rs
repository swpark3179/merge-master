//! 편집: 셀 잠금, 셀 수정 묶음(권한 → 형식 → 마감 → 적용 → 이력 → 알림), 진행률 증분, 서식, 수식 결과.

use std::time::{SystemTime, UNIX_EPOCH};

use collect_core::access::{Viewer, flag};
use collect_core::config::FormatPolicy;
use collect_core::format::{FormatEdit, StylePatch, apply_format};
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

const READ_ONLY: &str = "마감되어 입력할 수 없습니다";
const NOT_YOURS: &str = "입력할 수 없는 칸입니다";

pub(super) fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

impl State {
    fn idx(&self, at: CellRef) -> usize {
        (at.row * self.model.cols + at.col) as usize
    }

    fn at_of(&self, i: usize) -> CellRef {
        let cols = self.model.cols.max(1);
        CellRef::new(i as u32 / cols, i as u32 % cols)
    }

    fn in_bounds(&self, at: CellRef) -> bool {
        at.row < self.model.rows && at.col < self.model.cols
    }

    /// 그 칸이 보이는 사람에게만 보낸다(`except` 빼고).
    fn send_where_visible(&self, at: CellRef, msg: &ServerMsg, except: Option<ConnId>) {
        for (id, p) in self.attached() {
            if Some(id) != except && self.row_visible(p, at.row) {
                self.send(id, msg.clone());
            }
        }
    }

    // ------------------------------------------------------------ 잠금

    pub(super) fn lock(&mut self, conn: ConnId, at: CellRef) {
        let Some((name, p)) = self.person_of(conn) else { return };
        self.touch(conn);
        let refuse = |by: Option<String>, reason: Option<&str>| ServerMsg::LockResult {
            at,
            ok: false,
            by,
            reason: reason.map(String::from),
        };
        if self.read_only {
            return self.send(conn, refuse(None, Some(READ_ONLY)));
        }
        if !self.in_bounds(at) || !self.access.can_edit(Viewer::Person(p), at) {
            return self.send(conn, refuse(None, Some(NOT_YOURS)));
        }
        let i = self.idx(at);
        if let Some(l) = self.locks.get(&i)
            && l.owner != name
        {
            return self.send(conn, refuse(Some(l.owner.clone()), None));
        }
        // 한 사람은 한 칸만 편집한다: 앞서 잡은 다른 칸은 푼다.
        let others: Vec<usize> =
            self.locks.iter().filter(|(j, l)| **j != i && l.owner == name).map(|(j, _)| *j).collect();
        for j in others {
            self.release(j);
        }
        let fresh = self.locks.insert(i, Lock { owner: name.clone(), since: Instant::now(), orphaned: None }).is_none();
        self.send(conn, ServerMsg::LockResult { at, ok: true, by: None, reason: None });
        if fresh {
            self.send_where_visible(at, &ServerMsg::LockChanged { at, by: Some(name) }, Some(conn));
        }
    }

    pub(super) fn unlock(&mut self, conn: ConnId, at: CellRef) {
        let Some((name, _)) = self.person_of(conn) else { return };
        if !self.in_bounds(at) {
            return;
        }
        let i = self.idx(at);
        if self.locks.get(&i).is_some_and(|l| l.owner == name) {
            self.release(i);
        }
    }

    /// 잠금 하나를 풀고 알린다(잡았던 사람에게도: 호스트가 풀었으면 편집기를 닫아야 한다).
    fn release(&mut self, i: usize) {
        if self.locks.remove(&i).is_some() {
            let at = self.at_of(i);
            self.send_where_visible(at, &ServerMsg::LockChanged { at, by: None }, None);
        }
    }

    pub(super) fn release_locks_of(&mut self, name: &str) -> usize {
        let mine: Vec<usize> = self.locks.iter().filter(|(_, l)| l.owner == name).map(|(i, _)| *i).collect();
        for &i in &mine {
            self.release(i);
        }
        mine.len()
    }

    pub(super) fn host_unlock(&mut self, name: &str) -> Result<(), String> {
        if self.access.person(name).is_none() {
            return Err("명단에 없는 이름입니다".into());
        }
        self.release_locks_of(name.trim());
        Ok(())
    }

    /// 끊긴 지 [`LOCK_GRACE`](super::LOCK_GRACE)가 지났거나 [`LOCK_MAX`](super::LOCK_MAX)보다 오래 잡은 잠금을 푼다.
    pub(super) fn expire_locks(&mut self) {
        let stale: Vec<usize> = self
            .locks
            .iter()
            .filter(|(_, l)| {
                l.orphaned.is_some_and(|t| t.elapsed() >= super::LOCK_GRACE) || l.since.elapsed() >= super::LOCK_MAX
            })
            .map(|(i, _)| *i)
            .collect();
        for i in stale {
            self.release(i);
        }
    }

    // ------------------------------------------------------------ 셀 수정

    pub(super) fn set_cells(&mut self, conn: ConnId, batch: String, cells: Vec<CellEdit>, origin: BatchOrigin) {
        // 참여자는 복원·'Excel 값 유지'를 할 수 없다(표시만 바뀌는 값이라 일반 입력으로 본다).
        let origin = match origin {
            BatchOrigin::Restore | BatchOrigin::Excel => BatchOrigin::Edit,
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
        // 다시 연결한 뒤 같은 묶음을 또 보냈다(오프라인 대기열): 이미 반영했으니 지금 값만 돌려준다.
        let key = super::persist::applied_key(&name, &batch);
        if !self.applied.insert(key) {
            let cells = cells
                .iter()
                .map(|e| CellRef::new(e.row, e.col))
                .filter(|&at| self.in_bounds(at))
                .map(|at| self.update_of(at))
                .collect();
            return self.send(conn, ServerMsg::Applied { batch, cells, conflicts: vec![], rejected: vec![] });
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
            if !self.in_bounds(at) {
                continue;
            }
            let i = self.idx(at);
            let reject = |s: &State, reason: String| {
                let u = s.update_of(at);
                Rejection { row: at.row, col: at.col, reason, value: u.value, ver: u.ver }
            };
            if self.read_only {
                rejected.push(reject(self, READ_ONLY.into()));
                continue;
            }
            if !self.access.can_edit(Viewer::Person(p), at) {
                rejected.push(reject(self, NOT_YOURS.into()));
                continue;
            }
            if let Some(l) = self.locks.get(&i).filter(|l| l.owner != name) {
                let reason = format!("{} 님이 입력 중입니다", l.owner);
                rejected.push(reject(self, reason));
                continue;
            }
            let value = match self.config.columns.get(at.col as usize).map(|c| &c.format) {
                Some(format) => match normalize(format, &edit.value) {
                    Ok(v) => v,
                    Err(e) => {
                        rejected.push(reject(self, e.0));
                        continue;
                    }
                },
                None => edit.value,
            };
            let old = self.model.cell(at).map(|c| c.v.clone()).unwrap_or_default();
            if old == value {
                applied.push(self.update_of(at));
                continue;
            }
            // 내가 본 뒤 다른 사람이 바꿨다: 나중 입력(내 값)으로 덮어쓰고 알려 준다.
            if edit.base < self.versions[i] && self.writer.get(&i).is_some_and(|w| *w != name) {
                conflicts.push(at);
            }
            if let Some(cell) = self.model.cell_mut(at) {
                cell.v = value.clone();
            }
            self.versions[i] += 1;
            self.writer.insert(i, name.clone());
            self.log.push(ChangeEntry {
                seq: self.next_seq,
                at_ms,
                user: name.clone(),
                batch: batch.clone(),
                origin,
                kind: ChangeKind::of(origin, &old, &value),
                row: at.row,
                col: at.col,
                old: old.clone(),
                new: value.clone(),
            });
            self.next_seq += 1;
            self.count_progress(at, &old, &value, &mut touched_people);
            let u = CellUpdate { row: at.row, col: at.col, value, ver: self.versions[i] };
            applied.push(u.clone());
            changed.push(u);
        }

        // 입력을 끝냈으니 그 칸의 잠금은 푼다.
        let held: Vec<usize> = changed
            .iter()
            .chain(applied.iter())
            .map(|u| self.idx(CellRef::new(u.row, u.col)))
            .filter(|i| self.locks.get(i).is_some_and(|l| l.owner == name))
            .collect();
        for i in held {
            self.release(i);
        }

        if let Some(store) = &self.store {
            store.append(&self.log[logged..]);
        }
        self.send(conn, ServerMsg::Applied { batch, cells: applied, conflicts, rejected });
        self.broadcast_cells(&name, changed.clone(), Some(conn));
        self.send_progress(&touched_people);
        self.request_recalc(changed);
    }

    /// 바뀐 입력 칸을 수식 재계산에 넘긴다(수식이 있고 재계산을 맡은 곳이 있을 때).
    fn request_recalc(&mut self, changed: Vec<CellUpdate>) {
        if self.has_formulas
            && !changed.is_empty()
            && let Some(tx) = &self.recalc
        {
            let writes = changed.into_iter().map(|u| (CellRef::new(u.row, u.col), u.value)).collect();
            if tx.send(RecalcRequest { writes }).is_err() {
                self.recalc = None;
            }
        }
    }

    /// 호스트가 바꾼 칸([`Shared::host_set`](super::Shared::host_set)): 권한·형식 검사 없이 반영하고 모두에게 알린다.
    pub(super) fn host_writes(&mut self, user: &str, origin: BatchOrigin, values: Vec<(CellRef, CellValue)>) {
        let at_ms = now_ms();
        let logged = self.log.len();
        let batch = format!("host-{}", self.next_seq);
        let mut changed = Vec::new();
        let mut touched = Vec::new();
        for (at, value) in values {
            if !self.in_bounds(at) {
                continue;
            }
            let Some(cell) = self.model.cell(at).filter(|c| c.f.is_none() && c.v != value) else { continue };
            let old = cell.v.clone();
            if let Some(cell) = self.model.cell_mut(at) {
                cell.v = value.clone();
            }
            let i = self.idx(at);
            self.versions[i] += 1;
            self.writer.insert(i, user.to_string());
            self.log.push(ChangeEntry {
                seq: self.next_seq,
                at_ms,
                user: user.to_string(),
                batch: batch.clone(),
                origin,
                kind: ChangeKind::of(origin, &old, &value),
                row: at.row,
                col: at.col,
                old: old.clone(),
                new: value.clone(),
            });
            self.next_seq += 1;
            self.count_progress(at, &old, &value, &mut touched);
            changed.push(CellUpdate { row: at.row, col: at.col, value, ver: self.versions[i] });
        }
        if let Some(store) = &self.store {
            store.append(&self.log[logged..]);
        }
        self.broadcast_cells(user, changed.clone(), None);
        self.send_progress(&touched);
        self.request_recalc(changed);
    }

    fn update_of(&self, at: CellRef) -> CellUpdate {
        let value = self.model.cell(at).map(|c| c.v.clone()).unwrap_or_default();
        CellUpdate { row: at.row, col: at.col, value, ver: self.versions[self.idx(at)] }
    }

    /// 바뀐 칸을 다른 사람에게 보낸다('남의 행 숨김'이면 그 행이 보이는 사람에게만).
    fn broadcast_cells(&self, by: &str, cells: Vec<CellUpdate>, except: Option<ConnId>) {
        if cells.is_empty() {
            return;
        }
        self.feed(HostEvent::Cells { by: by.to_string(), cells: cells.clone() });
        let all = ServerMsg::Cells { by: by.to_string(), cells: cells.clone() };
        for (id, p) in self.attached() {
            if Some(id) == except {
                continue;
            }
            if !self.visible.contains_key(&p) {
                self.send(id, all.clone());
                continue;
            }
            let mine: Vec<CellUpdate> = cells.iter().filter(|u| self.row_visible(p, u.row)).cloned().collect();
            if !mine.is_empty() {
                self.send(id, ServerMsg::Cells { by: by.to_string(), cells: mine });
            }
        }
    }

    /// 필수 칸이 채워지거나 비워졌으면 진행률을 고친다(전체·열·담당자).
    fn count_progress(&mut self, at: CellRef, old: &CellValue, new: &CellValue, touched: &mut Vec<usize>) {
        if self.access.flags(at) & flag::REQUIRED == 0 || old.is_empty() == new.is_empty() {
            return;
        }
        let up = !new.is_empty();
        let bump = |n: &mut u32| *n = if up { *n + 1 } else { n.saturating_sub(1) };
        bump(&mut self.progress.filled);
        if let Some(c) = self.progress.by_column.get_mut(at.col as usize) {
            bump(&mut c.filled);
        }
        for p in self.access.owners(at) {
            if let Some(pp) = self.progress.by_person.get_mut(p) {
                bump(&mut pp.filled);
            }
            if !touched.contains(&p) {
                touched.push(p);
            }
        }
    }

    fn send_progress(&self, people: &[usize]) {
        for (id, p) in self.attached() {
            if people.contains(&p)
                && let Some(progress) = self.progress.by_person.get(p)
            {
                self.send(id, ServerMsg::Progress { progress: progress.clone() });
            }
        }
    }

    // ------------------------------------------------------------ 이력·서식·수식

    pub(super) fn row_history(&mut self, conn: ConnId, row: u32) {
        let Some((_, p)) = self.person_of(conn) else { return };
        if row >= self.model.rows || !self.row_visible(p, row) {
            return self.send(conn, ServerMsg::History { row, entries: vec![] });
        }
        let mut entries: Vec<ChangeEntry> =
            self.log.iter().rev().filter(|e| e.row == row).take(MAX_ROW_HISTORY).cloned().collect();
        entries.reverse();
        self.send(conn, ServerMsg::History { row, entries });
    }

    pub(super) fn format(&mut self, conn: ConnId, range: CellRange, patch: StylePatch) {
        let Some((_, p)) = self.person_of(conn) else { return };
        self.touch(conn);
        let error = |s: &State, message: &str| s.send(conn, ServerMsg::Error { message: message.into() });
        let fill_only = StylePatch { fill: patch.fill, ..Default::default() };
        match self.config.permissions.format {
            FormatPolicy::None => return error(self, "이 취합에서는 서식을 바꿀 수 없습니다"),
            FormatPolicy::FillOnly if patch != fill_only => {
                return error(self, "이 취합에서는 배경색만 바꿀 수 있습니다");
            }
            _ => {}
        }
        if self.read_only {
            return error(self, READ_ONLY);
        }
        let Some(range) = range.intersect(&self.model.bounds()) else { return };
        if range.cells().any(|at| !self.access.can_edit(Viewer::Person(p), at)) {
            return error(self, "내가 입력할 수 있는 칸만 서식을 바꿀 수 있습니다");
        }
        // 테두리 없애기는 범위 바깥 이웃 칸도 고칠 수 있어 한 칸 넓혀 비교한다.
        let (r0, c0) = (range.row.saturating_sub(1), range.col.saturating_sub(1));
        let wide = CellRange::new(r0, c0, range.last_row() + 2 - r0, range.last_col() + 2 - c0)
            .intersect(&self.model.bounds())
            .unwrap_or(range);
        let before: Vec<u32> = wide.cells().map(|at| self.model.cell(at).map_or(0, |c| c.s)).collect();
        if let Err(e) = apply_format(&mut self.model, FormatEdit::Style { range, patch }) {
            return error(self, &e.to_string());
        }
        let changes: Vec<StyleChange> = wide
            .cells()
            .zip(before)
            .filter_map(|(at, old)| {
                let s = self.model.cell(at)?.s;
                (s != old).then_some(StyleChange { row: at.row, col: at.col, s })
            })
            .collect();
        self.persist_sheet();
        self.feed(HostEvent::Styles { styles: self.model.styles.clone(), changes: changes.clone() });
        let msg = ServerMsg::Styles { styles: self.model.styles.clone(), changes };
        self.broadcast(&msg, None);
    }

    /// 호스트(Excel)가 다시 계산한 수식 칸. 바뀐 칸만 보낸다(이력에는 남기지 않는다).
    pub(super) fn formulas(&mut self, values: Vec<(CellRef, CellValue)>) {
        let mut changed = Vec::new();
        for (at, v) in values {
            if !self.in_bounds(at) {
                continue;
            }
            let i = self.idx(at);
            let Some(cell) = self.model.cell_mut(at).filter(|c| c.f.is_some() && c.v != v) else { continue };
            cell.v = v.clone();
            self.versions[i] += 1;
            changed.push(CellUpdate { row: at.row, col: at.col, value: v, ver: self.versions[i] });
        }
        self.broadcast_cells("", changed, None);
    }
}
