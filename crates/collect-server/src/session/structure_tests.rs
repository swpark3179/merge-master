//! 표 구조 바꾸기 단위 테스트: 행·열 붙이기, 행 지우기(엑셀에서 불러온 행 포함 — 누가·어느 행, 기준 시트·계산 사본·
//! 저장 중), 앱에서 붙인 열 지우기, 구조 판, 취소 행, 마감 동작.

use collect_core::config::DeadlineAction;
use collect_core::structure::StructureEdit;

use super::tests::{Client, connect, fixture, host, host_get, join};
use super::*;
use crate::protocol::CellEdit;

const F: u32 = 5; // 신청 금액(숫자, 필수)
const H: u32 = 7; // 산출 근거(글자)

fn setup(names: &[&str], f: impl FnOnce(&mut CollectConfig)) -> (State, Vec<Client>) {
    let mut init = fixture();
    f(&mut init.sheets[0].1);
    let mut st = State::new(init);
    let mut clients = Vec::new();
    for (i, name) in names.iter().enumerate() {
        let c = connect(&mut st, i as u64 + 1);
        join(&mut st, &c, name, None);
        clients.push(c);
    }
    for c in &mut clients {
        c.take();
    }
    (st, clients)
}

fn client(st: &mut State, c: &Client, msg: ClientMsg) {
    st.handle(Command::Client { conn: c.conn, msg });
}

fn set(st: &mut State, c: &Client, batch: &str, row: u32, col: u32, v: CellValue, layout: Option<u32>) {
    let cells = vec![CellEdit { row, col, value: v, base: 0 }];
    client(st, c, ClientMsg::SetCells { sheet: 0, batch: batch.into(), cells, origin: None, layout });
}

fn errors(msgs: &[ServerMsg]) -> Vec<String> {
    msgs.iter()
        .filter_map(|m| match m {
            ServerMsg::Error { message } => Some(message.clone()),
            _ => None,
        })
        .collect()
}

fn rejected(msgs: &[ServerMsg]) -> Vec<String> {
    msgs.iter()
        .find_map(|m| match m {
            ServerMsg::Applied { rejected, .. } => Some(rejected.iter().map(|r| r.reason.clone()).collect()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("applied가 와야 함: {msgs:?}"))
}

fn value(st: &State, row: u32, col: u32) -> CellValue {
    st.sheets[0].model.cell(CellRef::new(row, col)).unwrap().v.clone()
}

fn text(s: &str) -> CellValue {
    CellValue::Text(s.into())
}

#[tokio::test]
async fn participants_delete_only_rows_they_added() {
    let (mut st, mut c) = setup(&["정하늘", "최도윤"], |cfg| {
        cfg.permissions.add_rows = true;
    });
    let rows = st.sheets[0].model.rows;
    client(&mut st, &c[0], ClientMsg::AddRows { sheet: 0, count: 2 });
    client(&mut st, &c[1], ClientMsg::AddRows { sheet: 0, count: 1 });
    assert_eq!(st.sheets[0].model.rows, rows + 3);
    let adders: Vec<&str> = st.sheets[0].model.added_rows.iter().map(|a| a.by.as_str()).collect();
    assert_eq!(adders, ["정하늘", "정하늘", "최도윤"]);
    set(&mut st, &c[1], "b1", rows + 2, H, text("최도윤 행"), Some(0));
    c[0].take();
    c[1].take();

    let delete = |from, count, layout| ClientMsg::DeleteRows { sheet: 0, from, count, layout };
    client(&mut st, &c[0], delete(rows, 1, Some(0)));
    assert!(errors(&c[0].take())[0].contains("행을 지울 수 없습니다"), "권한 '행 삭제'가 꺼져 있다");
    host(&mut st, |s| {
        s.sheets[0].config.permissions.delete_rows = true;
        s.changed();
    });
    c[0].take();
    client(&mut st, &c[0], delete(rows + 1, 2, Some(0)));
    assert!(errors(&c[0].take())[0].contains("내가 추가한 행이나 내게 배정된 행만"));
    client(&mut st, &c[0], delete(rows - 1, 1, Some(0)));
    assert!(errors(&c[0].take())[0].contains("내게 배정된 행만"), "남(김민지)의 불러온 행");

    let mut feed = st.host_feed.subscribe();
    client(&mut st, &c[0], delete(rows, 2, Some(0)));
    assert_eq!(st.sheets[0].model.rows, rows + 1);
    assert_eq!(st.sheets[0].model.layout, 1);
    assert_eq!(value(&st, rows, H), text("최도윤 행"), "아래 행이 당겨진다");
    assert_eq!(st.sheets[0].model.added_rows.len(), 1);
    assert_eq!(st.rt[0].versions.len(), st.sheets[0].model.cells.len());
    assert_eq!(st.rt[0].versions[(rows * st.sheets[0].model.cols + H) as usize], 1, "버전도 따라간다");
    let msgs = c[1].take();
    assert!(msgs.iter().any(|m| matches!(m, ServerMsg::Welcome(w) if w.sheets[0].model.layout == 1)));
    assert!(
        msgs.iter()
            .any(|m| matches!(m, ServerMsg::RowsDeleted { by, from, count, .. } if by == "정하늘" && *from == rows && *count == 2))
    );
    assert!(matches!(feed.try_recv(), Ok(HostEvent::Reset)));
    // 지운 행에 남긴 기록(담당자 칸 이름)은 '지움'으로 남고, 남의 행 기록은 새 좌표로 옮겨진다.
    assert!(st.log.iter().filter(|e| e.user == "정하늘").all(|e| e.gone));
    assert!(st.log.iter().any(|e| e.user == "최도윤" && e.row == rows && e.col == H && !e.gone));
    let key = st.sheets[0].config.assignment.key_col.unwrap();
    assert_eq!(st.rt[0].access.row_owner(rows), st.rt[0].access.person("최도윤"));
    assert_eq!(value(&st, rows, key), text("최도윤"));

    // 지우기 전 판으로 보낸 편집(오프라인 대기열)은 좌표가 밀렸을 수 있어 받지 않는다.
    set(&mut st, &c[1], "late", rows, H, text("늦게"), Some(0));
    assert!(rejected(&c[1].take())[0].contains("표 구조가 바뀌어"));
    assert_eq!(value(&st, rows, H), text("최도윤 행"));
    set(&mut st, &c[1], "now", rows, H, text("지금"), Some(1));
    assert!(rejected(&c[1].take()).is_empty());
}

#[tokio::test]
async fn host_deletes_only_unassigned_added_rows_and_cols() {
    let (mut st, mut c) = setup(&["정하늘"], |cfg| cfg.permissions.add_rows = true);
    let (rows, cols) = (st.sheets[0].model.rows, st.sheets[0].model.cols);
    client(&mut st, &c[0], ClientMsg::AddRows { sheet: 0, count: 1 });
    let r = host_get(&mut st, |s| s.structure(0, StructureEdit::AddRows { count: 2 }));
    assert_eq!(r, Ok(()));
    assert_eq!(st.sheets[0].model.rows, rows + 3);
    assert!(st.rt[0].access.row_owner(rows + 1).is_none(), "호스트가 붙인 행은 담당자 없음");

    let del = |from, count| move |s: &mut Shared<'_>| s.structure(0, StructureEdit::DeleteRows { from, count });
    assert!(host_get(&mut st, del(rows, 1)).unwrap_err().contains("담당자(정하늘)"));
    assert!(host_get(&mut st, del(1, 1)).unwrap_err().contains("머리글"));
    assert_eq!(host_get(&mut st, del(rows + 1, 2)), Ok(()));
    assert_eq!(st.sheets[0].model.rows, rows + 1);

    // 열: 붙인 열은 입력 열이고 행 담당자가 입력한다. 행 배정 기준 열·불러온 열은 지우지 않는다.
    set(&mut st, &c[0], "before", rows, F, CellValue::Number(5.0), None);
    let ver = st.rt[0].versions[(rows * cols + F) as usize];
    assert_eq!(host_get(&mut st, |s| s.structure(0, StructureEdit::AddCols { count: 2 })), Ok(()));
    assert_eq!(st.sheets[0].model.cols, cols + 2);
    assert_eq!(st.sheets[0].config.columns.len() as u32, cols + 2);
    assert_eq!(st.rt[0].versions[(rows * (cols + 2) + F) as usize], ver, "버전이 새 열 수로 옮겨진다");
    c[0].take();
    let layout = st.sheets[0].model.layout;
    set(&mut st, &c[0], "new col", rows, cols, text("비고"), Some(layout));
    assert!(rejected(&c[0].take()).is_empty());
    let key = st.sheets[0].config.assignment.key_col.unwrap();
    let del_col =
        move |from, count| move |s: &mut Shared<'_>| s.structure(0, StructureEdit::DeleteCols { from, count });
    assert!(host_get(&mut st, del_col(key, 1)).unwrap_err().contains("엑셀에서 불러온 열"));
    assert_eq!(host_get(&mut st, del_col(cols, 1)), Ok(()));
    assert_eq!(st.sheets[0].model.cols, cols + 1);
    assert!(st.log.iter().any(|e| e.batch == "new col" && e.gone));
    // 스냅샷은 지금 시트 모양으로 옮겨져 복원할 수 있다.
    let n = host_get(&mut st, |s| {
        let id = s.snapshots()[0].id;
        s.restore(id, "호스트")
    });
    assert!(n.is_ok(), "{n:?}");
}

#[tokio::test]
async fn host_deletes_imported_rows_and_the_baseline_follows() {
    let (mut st, mut c) = setup(&["이서연"], |cfg| cfg.assignment.key_col = None); // 행 배정 없음
    let (tx, mut rx) = mpsc::unbounded_channel();
    st.handle(Command::SetRecalc(Some(tx)));
    let (rows, base_rows) = (st.sheets[0].model.rows, st.sheets[0].baseline.rows);
    set(&mut st, &c[0], "b1", rows - 1, H, text("마지막 행"), None);
    set(&mut st, &c[0], "b2", rows - 2, H, text("지울 행"), None);
    host(&mut st, |s| s.take_snapshot("지우기 전"));
    rx.try_recv().unwrap();
    rx.try_recv().unwrap();
    c[0].take();
    let epoch = st.calc_layout().epoch;

    // 가운데 불러온 행 하나(병합 없는 행)를 지운다.
    let r = rows - 2;
    assert_eq!(host_get(&mut st, move |s| s.structure(0, StructureEdit::DeleteRows { from: r, count: 1 })), Ok(()));
    assert_eq!((st.sheets[0].model.rows, st.sheets[0].baseline.rows), (rows - 1, base_rows - 1));
    assert_eq!(st.sheets[0].baseline.deleted_rows, [RowCut { from: r, count: 1 }]);
    assert_eq!(value(&st, rows - 2, H), text("마지막 행"), "아래 행이 당겨진다");
    assert!(st.log.iter().any(|e| e.batch == "b2" && e.gone));
    // 계산 사본에서도 지우고, 지우기 전 판의 결과는 버린다.
    let req = rx.try_recv().expect("계산 사본 행 지우기");
    assert_eq!(req.cuts, [(0, RowCut { from: r, count: 1 })]);
    assert_eq!(req.epoch, epoch + 1);
    let g = CellRef::new(rows - 2, 6);
    let before = value(&st, rows - 2, 6);
    st.handle(Command::Formulas { values: vec![(0, g, CellValue::Number(42.0))], epoch: Some(epoch) });
    assert_eq!(value(&st, rows - 2, 6), before, "옛 판 결과는 버린다");
    st.handle(Command::Formulas { values: vec![(0, g, CellValue::Number(42.0))], epoch: Some(epoch + 1) });
    assert_eq!(value(&st, rows - 2, 6), CellValue::Number(42.0));
    // 사본을 다시 열 때 맞출 것: 지운 행 순서, 수식 칸은 불러온 범위 안, 기록 재생은 지운 기록 빼고.
    let layout = st.calc_layout();
    assert_eq!(layout.epoch, epoch + 1);
    assert_eq!(layout.sheets[0].cuts, [RowCut { from: r, count: 1 }]);
    assert!(layout.sheets[0].formulas.iter().all(|at| at.row < base_rows - 1));
    assert_eq!(layout.replay, [(0, CellRef::new(rows - 2, H), text("마지막 행"))]);
    // 저장 계획: 먼저 Excel에서 그 행을 지운다.
    let parts = host_get(&mut st, |s| s.save_parts());
    let plan = parts.plan(&["예산신청서".into()], ExportOptions::default(), 0, &[]);
    assert_eq!(plan[0].1.row_cuts, [RowCut { from: r, count: 1 }]);
    // 스냅샷도 새 모양이라 복원할 수 있다.
    let n = host_get(&mut st, |s| {
        let id = s.snapshots()[0].id;
        s.restore(id, "호스트")
    });
    assert!(n.is_ok(), "{n:?}");
}

/// 그 사람에게 배정된, 담당자 칸이 병합된 행 묶음 중 통째로 지울 수 있는 것(첫 행, 행 수).
fn merged_group_of(st: &State, name: &str) -> (u32, u32) {
    let (model, access) = (&st.sheets[0].model, &st.rt[0].access);
    let key = st.sheets[0].config.assignment.key_col.unwrap();
    let p = access.person(name).unwrap();
    model
        .merges
        .iter()
        .find(|m| {
            m.col == key
                && m.rows >= 2
                && access.row_owner(m.row) == Some(p)
                && collect_core::structure::cut_merge(model, m.row, m.rows).is_none()
        })
        .map(|m| (m.row, m.rows))
        .expect("배정된 병합 행")
}

#[tokio::test]
async fn participants_delete_their_assigned_imported_rows_and_stay() {
    let (mut st, mut c) = setup(&["최도윤", "이서연"], |cfg| cfg.permissions.delete_rows = true);
    let (from, count) = merged_group_of(&st, "최도윤");
    let other = merged_group_of(&st, "이서연");
    let delete = |from, count| ClientMsg::DeleteRows { sheet: 0, from, count, layout: Some(0) };
    client(&mut st, &c[0], delete(from, 1));
    assert!(errors(&c[0].take())[0].contains("첫 행만 지울 수는 없습니다"));
    client(&mut st, &c[0], delete(other.0, other.1));
    assert!(errors(&c[0].take())[0].contains("내게 배정된 행만"));
    let base_rows = st.sheets[0].baseline.rows;
    client(&mut st, &c[0], delete(from, count));
    let msgs = c[0].take();
    assert!(errors(&msgs).is_empty());
    assert_eq!(st.sheets[0].baseline.deleted_rows, [RowCut { from, count }]);
    assert_eq!(st.sheets[0].baseline.rows, base_rows - count);
    assert!(st.rt[0].access.person("최도윤").is_some(), "명단에 남는다");
    assert!(!c[0].closed);
    assert!(msgs.iter().any(|m| matches!(m, ServerMsg::Welcome(_))));
}

#[tokio::test]
async fn rows_and_cols_cannot_be_deleted_while_saving() {
    let (mut st, mut c) = setup(&["최도윤"], |cfg| cfg.permissions.delete_rows = true);
    let (r, n) = merged_group_of(&st, "최도윤");
    st.handle(Command::SetSaving(true));
    assert!(
        host_get(&mut st, move |s| s.structure(0, StructureEdit::DeleteCols { from: 0, count: 1 }))
            .unwrap_err()
            .contains("저장하는 중")
    );
    client(&mut st, &c[0], ClientMsg::DeleteRows { sheet: 0, from: r, count: n, layout: Some(0) });
    assert!(errors(&c[0].take())[0].contains("저장하는 중"));
    st.handle(Command::SetSaving(false));
    client(&mut st, &c[0], ClientMsg::DeleteRows { sheet: 0, from: r, count: n, layout: Some(0) });
    assert!(errors(&c[0].take()).is_empty());
}

#[tokio::test]
async fn cancelled_rows_drop_required_cells_and_only_editors_can_cancel() {
    let (mut st, mut c) = setup(&["이서연", "최도윤"], |_| {});
    let access = &st.rt[0].access;
    let mine = access.rows_of(access.person("이서연").unwrap())[0];
    let theirs = access.rows_of(access.person("최도윤").unwrap())[0];
    let required = st.rt[0].progress.required;
    let cancel = |from, on| ClientMsg::CancelRows { sheet: 0, from, count: 1, on, layout: Some(0) };
    client(&mut st, &c[0], cancel(theirs, true));
    assert!(errors(&c[0].take())[0].contains("내가 입력할 수 있는 행만"));
    client(&mut st, &c[0], cancel(mine, true));
    assert_eq!(st.sheets[0].model.cancelled_rows, [mine]);
    assert_eq!(st.rt[0].progress.required, required - 1, "취소 행의 필수 칸은 세지 않는다");
    let w = c[1].welcome();
    assert_eq!(w.sheets[0].model.cancelled_rows, [mine], "모두에게 보인다");
    client(&mut st, &c[0], cancel(mine, false));
    assert!(st.sheets[0].model.cancelled_rows.is_empty());
    assert_eq!(st.rt[0].progress.required, required);
    // 호스트는 어느 데이터 행이든.
    assert_eq!(
        host_get(&mut st, move |s| s.structure(0, StructureEdit::CancelRows { from: theirs, count: 1, on: true })),
        Ok(())
    );
    assert_eq!(st.sheets[0].model.cancelled_rows, [theirs]);
    assert!(host_get(&mut st, |s| s.structure(0, StructureEdit::CancelRows { from: 0, count: 1, on: true })).is_err());
}

#[tokio::test]
async fn assignment_and_host_clock_reach_participants() {
    let (mut st, mut c) = setup(&["이서연"], |cfg| cfg.deadline = Some("2999-01-02T03:04".into()));
    host(&mut st, |s| s.changed());
    let w = c[0].welcome();
    assert!(w.sheets[0].assigned, "행 배정 기준 열이 있다");
    assert!((w.server_now - edit::now_ms()).abs() < 5_000);
    let at = chrono::NaiveDate::from_ymd_opt(2999, 1, 2).unwrap().and_hms_opt(3, 4, 0).unwrap();
    use chrono::TimeZone;
    assert_eq!(w.session.deadline_at, chrono::Local.from_local_datetime(&at).earliest().map(|t| t.timestamp_millis()));
    host(&mut st, |s| {
        s.sheets[0].config.assignment.key_col = None;
        s.sheets[0].config.assignment.extra_people = vec!["이서연".into()]; // 명단에서 빠지지 않게
        s.changed();
    });
    assert!(!c[0].welcome().sheets[0].assigned);
    client(&mut st, &c[0], ClientMsg::Ping);
    assert!(matches!(&c[0].take()[..], [ServerMsg::Pong { now }] if (*now - edit::now_ms()).abs() < 5_000));
}

#[tokio::test]
async fn moving_the_deadline_into_the_past_tells_participants_at_once() {
    let (mut st, mut c) = setup(&["이서연"], |_| {});
    host(&mut st, |s| {
        s.sheets[0].config.deadline = Some("2000-01-01T09:00".into());
        s.changed();
    });
    let msgs = c[0].take();
    let i = msgs.iter().position(|m| matches!(m, ServerMsg::ReadOnly { read_only: true, .. }));
    let j = msgs.iter().position(|m| matches!(m, ServerMsg::Welcome(_)));
    assert!(i.is_some() && i < j, "읽기 전용을 알린 뒤 새 시트: {msgs:?}");
}

#[tokio::test]
async fn stop_sharing_at_the_deadline_fires_once_per_deadline() {
    let (mut st, mut c) = setup(&["이서연"], |cfg| cfg.permissions.deadline_action = DeadlineAction::StopSharing);
    let mut feed = st.host_feed.subscribe();
    host(&mut st, |s| {
        s.sheets[0].config.deadline = Some("2000-01-01T09:00".into());
        s.changed();
    });
    while feed.try_recv().is_ok() {}
    assert!(!st.read_only, "공유 닫기면 읽기 전용으로 바꾸지 않는다");
    st.handle(Command::DeadlineTick);
    assert_eq!(feed.try_recv(), Ok(HostEvent::DeadlineClose));
    assert_eq!(st.host_meta.get(META_DEADLINE_FIRED).map(String::as_str), Some("2000-01-01T09:00"));
    st.handle(Command::DeadlineTick);
    assert!(feed.try_recv().is_err(), "같은 마감으로는 한 번만");
    // 호스트가 마감을 바꾸면 다시 닫을 수 있다.
    host(&mut st, |s| {
        s.sheets[0].config.deadline = Some("2000-01-02T09:00".into());
        s.changed();
    });
    while feed.try_recv().is_ok() {}
    st.handle(Command::DeadlineTick);
    assert_eq!(feed.try_recv(), Ok(HostEvent::DeadlineClose));
    c[0].take();
}

#[tokio::test]
async fn open_rows_by_lists_unassigned_rows_holding_each_persons_values() {
    let (mut st, mut c) = setup(&["이서연", "최도윤"], |_| {});
    let rows = st.sheets[0].model.rows;
    assert_eq!(host_get(&mut st, |s| s.structure(0, StructureEdit::AddRows { count: 3 })), Ok(()));
    let seo = st.rt[0].access.person("이서연").unwrap();
    let choi = st.rt[0].access.person("최도윤").unwrap();
    let own = st.rt[0].access.rows_of(seo)[0];
    assert!(st.rt[0].access.row_owner(rows).is_none(), "호스트가 붙인 행은 담당자 없음");

    // 담당자 없는 행에 넣은 값만 센다(제 행은 행 담당자로 이미 보인다).
    set(&mut st, &c[0], "a", rows, H, text("이서연 입력"), None);
    set(&mut st, &c[0], "b", own, H, text("제 행"), None);
    // 남이 덮어쓴 칸은 덮어쓴 사람 것, 지운 칸은 아무의 것도 아니다.
    set(&mut st, &c[1], "c", rows + 1, H, text("최도윤 입력"), None);
    set(&mut st, &c[0], "d", rows + 1, H, text("이서연이 고침"), None);
    set(&mut st, &c[1], "e", rows + 2, H, text("잠깐"), None);
    set(&mut st, &c[1], "f", rows + 2, H, CellValue::Empty, None);
    // 호스트(명단 밖 이름)가 넣은 값은 사람별로 세지 않는다.
    host(&mut st, move |s| {
        let user = s.host_name().to_string();
        s.host_set(&user, BatchOrigin::Edit, 0, vec![(CellRef::new(rows + 2, H), text("호스트"))]);
    });
    for x in &mut c {
        assert!(errors(&x.take()).is_empty());
    }
    assert_eq!(value(&st, rows + 2, H), text("호스트"));

    let by = host_get(&mut st, |s| s.open_rows_by(0));
    assert_eq!(by.len(), st.rt[0].access.roster().len());
    assert_eq!(by[seo], [rows, rows + 1]);
    assert!(by[choi].is_empty());
    assert!(host_get(&mut st, |s| s.open_rows_by(9)).is_empty(), "없는 시트");
}
