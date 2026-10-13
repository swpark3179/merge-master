//! 여러 시트 취합: 합집합 명단, 시트별 편집·권한·잠금, 공통 설정, 소통·스냅샷, 기록 파일(옛 한 시트 파일 포함).

use base64::Engine;
use collect_core::access::flag;
use collect_core::config::{ColumnRole, DeadlineAction, OthersRows, Permissions, suggested_key_col};
use collect_core::history::BatchOrigin;
use collect_store::{pack, pack_json};

use super::tests::{Client, connect, fixture, host, join};
use super::*;
use crate::protocol::CellEdit;

const F: u32 = 5; // 예산신청서: 신청 금액(필수)
const E: u32 = 4; // 인력계획: 신규 요청(명)

fn staff() -> (SheetModel, CollectConfig) {
    let model: SheetModel = serde_json::from_slice(include_bytes!("../../../../fixtures/staff.sheet.json")).unwrap();
    let mut config = CollectConfig::suggest(&model);
    config.assignment.key_col = suggested_key_col(&model); // B 담당자
    config.columns[E as usize].required = true;
    (model, config)
}

/// 예산신청서(행 배정 C열, F 필수) + 인력계획(행 배정 B열, E 필수).
fn two_sheets() -> SessionInit {
    let mut init = fixture();
    init.sheets.push(staff());
    init
}

fn set(st: &mut State, c: &Client, sheet: u32, batch: &str, row: u32, col: u32, value: CellValue) {
    let cells = vec![CellEdit { row, col, value, base: 0 }];
    st.handle(Command::Client {
        conn: c.conn,
        msg: ClientMsg::SetCells { sheet, batch: batch.into(), cells, origin: None, layout: None },
    });
}

/// 그 시트에서 그 사람이 맡은 첫 행.
fn row_of(st: &State, sheet: usize, name: &str) -> u32 {
    let p = st.person(name).unwrap();
    st.rt[sheet].access.rows_of(p).into_iter().find(|&r| st.rt[sheet].access.row_owner(r) == Some(p)).unwrap()
}

fn num(n: f64) -> CellValue {
    CellValue::Number(n)
}

#[tokio::test]
async fn welcome_has_every_sheet_with_one_shared_roster() {
    let mut st = State::new(two_sheets());
    assert_eq!(st.sheets.len(), 2);
    // 명단 = 예산신청서 담당자 6명 + 인력계획에만 있는 신예준
    assert_eq!(st.roster.len(), 7);
    assert_eq!(st.roster.last().map(String::as_str), Some("신예준"));
    assert_eq!(st.rt[0].access.roster(), st.rt[1].access.roster(), "시트마다 사람 인덱스가 같다");

    let mut shin = connect(&mut st, 1);
    join(&mut st, &shin, "신예준", None);
    let w = shin.welcome();
    assert_eq!((w.color, w.roster.len(), w.sheets.len()), (6, 7, 2));
    assert_eq!(w.sheets[1].model.sheet, "인력계획");
    assert_eq!(w.sheets[0].progress.required, 0, "예산신청서에는 맡은 행이 없다");
    assert_eq!(w.sheets[1].progress.required, 1);
    let flags = base64::engine::general_purpose::STANDARD.decode(&w.sheets[1].flags).unwrap();
    let mine = |r: u32, c: u32| flags[(r * w.sheets[1].model.cols + c) as usize] & flag::MINE != 0;
    let r = row_of(&st, 1, "신예준");
    assert!(mine(r, E) && !mine(1, E));
    // 담당자가 없는 '공통' 행(9행)은 명단의 누구나 입력한다.
    assert!(mine(8, E));

    let snap = st.snapshot();
    assert_eq!(snap.sheets.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["예산신청서", "인력계획"]);
    let shin_row = snap.participants.iter().find(|p| p.name == "신예준").unwrap();
    assert_eq!(shin_row.assignment, "인력계획: 행 8");
    let lee = snap.participants.iter().find(|p| p.name == "이서연").unwrap();
    assert!(lee.assignment.starts_with("예산신청서: 행 ") && lee.assignment.contains(" · 인력계획: 행 2–3"));
    assert_eq!(snap.required, st.rt[0].progress.required + st.rt[1].progress.required);
}

#[tokio::test]
async fn edits_land_on_their_own_sheet() {
    let mut st = State::new(two_sheets());
    let mut lee = connect(&mut st, 1);
    join(&mut st, &lee, "이서연", None);
    let mut kim = connect(&mut st, 2);
    join(&mut st, &kim, "김민지", None);
    lee.take();
    kim.take();
    let r = row_of(&st, 1, "이서연");
    let before = st.sheets[0].model.cell(CellRef::new(r, E)).unwrap().v.clone();
    set(&mut st, &lee, 1, "b1", r, E, num(2.0));

    assert_eq!(st.sheets[1].model.cell(CellRef::new(r, E)).unwrap().v, num(2.0));
    assert_eq!(st.sheets[0].model.cell(CellRef::new(r, E)).unwrap().v, before, "다른 시트의 같은 주소는 그대로");
    let e = st.log.last().unwrap();
    assert_eq!((e.sheet, e.row, e.col), (1, r, E));
    assert!(lee.take().iter().any(|m| matches!(m, ServerMsg::Applied { sheet: 1, cells, .. } if cells.len() == 1)));
    assert!(kim.take().iter().any(|m| matches!(m, ServerMsg::Cells { sheet: 1, by, .. } if by == "이서연")));
    assert_eq!(st.rt[1].versions[(r * st.sheets[1].model.cols + E) as usize], 1);
    assert_eq!(st.rt[1].progress.filled, 1);
    assert_eq!(st.total_progress(st.person("이서연").unwrap()).filled, st.rt[0].progress.by_person[0].filled + 1);

    // 이서연은 인력계획에서 김민지 행을 못 쓴다(시트마다 권한).
    let kim_row = row_of(&st, 1, "김민지");
    set(&mut st, &lee, 1, "b2", kim_row, E, num(1.0));
    let msgs = lee.take();
    assert!(msgs.iter().any(|m| matches!(m, ServerMsg::Applied { rejected, .. } if rejected.len() == 1)));
    // 행 이력은 그 시트 것만.
    st.handle(Command::Client { conn: 1, msg: ClientMsg::RowHistory { sheet: 0, row: r } });
    assert!(lee.take().iter().any(|m| matches!(m, ServerMsg::History { sheet: 0, entries, .. } if entries.is_empty())));
}

#[tokio::test]
async fn one_lock_per_person_across_sheets() {
    let mut st = State::new(two_sheets());
    let mut lee = connect(&mut st, 1);
    join(&mut st, &lee, "이서연", None);
    let mut kim = connect(&mut st, 2);
    join(&mut st, &kim, "김민지", None);
    let r0 = row_of(&st, 0, "이서연");
    let r1 = row_of(&st, 1, "이서연");
    st.handle(Command::Client { conn: 1, msg: ClientMsg::Lock { sheet: 0, at: CellRef::new(r0, F), layout: None } });
    assert_eq!(st.snapshot().participants[0].editing, Some(SheetCell::new(0, CellRef::new(r0, F))));
    lee.take();
    kim.take();
    st.handle(Command::Client { conn: 1, msg: ClientMsg::Lock { sheet: 1, at: CellRef::new(r1, E), layout: None } });
    assert!(lee.take().iter().any(|m| matches!(m, ServerMsg::LockResult { sheet: 1, ok: true, .. })));
    let kim_msgs = kim.take();
    assert!(kim_msgs.iter().any(|m| matches!(m, ServerMsg::LockChanged { sheet: 0, by: None, .. })), "{kim_msgs:?}");
    assert!(kim_msgs.iter().any(|m| matches!(m, ServerMsg::LockChanged { sheet: 1, by: Some(_), .. })));
    assert!(st.rt[0].locks.is_empty() && st.rt[1].locks.len() == 1);
    assert_eq!(st.snapshot().participants[0].editing, Some(SheetCell::new(1, CellRef::new(r1, E))));
    // 김민지는 이서연이 인력계획에서 잡은 칸과 상관없이 예산신청서 자기 칸을 잡는다.
    let k0 = row_of(&st, 0, "김민지");
    st.handle(Command::Client { conn: 2, msg: ClientMsg::Lock { sheet: 0, at: CellRef::new(k0, F), layout: None } });
    assert!(kim.take().iter().any(|m| matches!(m, ServerMsg::LockResult { sheet: 0, ok: true, .. })));
    assert_eq!((st.rt[0].locks.len(), st.rt[1].locks.len()), (1, 1));
}

#[tokio::test]
async fn unknown_sheet_is_refused_without_panicking() {
    let mut st = State::new(two_sheets());
    let mut lee = connect(&mut st, 1);
    join(&mut st, &lee, "이서연", None);
    lee.take();
    set(&mut st, &lee, 9, "x", 2, F, num(1.0));
    st.handle(Command::Client { conn: 1, msg: ClientMsg::Lock { sheet: 9, at: CellRef::new(2, F), layout: None } });
    st.handle(Command::Client { conn: 1, msg: ClientMsg::Cursor { sheet: 9, at: Some(CellRef::new(2, F)) } });
    st.handle(Command::Client { conn: 1, msg: ClientMsg::AddRows { sheet: 9, count: 1 } });
    st.handle(Command::Client { conn: 1, msg: ClientMsg::RowHistory { sheet: 9, row: 2 } });
    st.handle(Command::Formulas(vec![(9, CellRef::new(2, 6), num(1.0))]));
    let msgs = lee.take();
    assert!(msgs.iter().any(|m| matches!(m, ServerMsg::Error { message } if message == NO_SHEET)));
    assert!(msgs.iter().any(|m| matches!(m, ServerMsg::LockResult { ok: false, .. })));
    assert!(st.log.is_empty());
}

#[tokio::test]
async fn participant_permissions_are_per_sheet() {
    let mut init = two_sheets();
    init.sheets[1].1.permissions.others_rows = OthersRows::Hidden;
    init.sheets[1].1.permissions.add_rows = true;
    let mut st = State::new(init);
    let mut lee = connect(&mut st, 1);
    join(&mut st, &lee, "이서연", None);
    let mut kim = connect(&mut st, 2);
    join(&mut st, &kim, "김민지", None);
    let w = kim.welcome();
    assert!(w.sheets[0].model.hidden_rows.is_empty(), "예산신청서는 남의 행도 보인다");
    let lee_row = row_of(&st, 1, "이서연");
    assert!(w.sheets[1].model.hidden_rows.contains(&lee_row), "인력계획은 남의 행을 숨긴다");
    lee.take();
    set(&mut st, &lee, 1, "b1", lee_row, E, num(3.0));
    assert!(!kim.take().iter().any(|m| matches!(m, ServerMsg::Cells { .. })), "숨긴 행의 값은 보내지 않는다");

    // 행 추가는 인력계획에서만.
    st.handle(Command::Client { conn: 2, msg: ClientMsg::AddRows { sheet: 0, count: 1 } });
    assert!(kim.take().iter().any(|m| matches!(m, ServerMsg::Error { .. })));
    let rows = st.sheets[1].model.rows;
    st.handle(Command::Client { conn: 2, msg: ClientMsg::AddRows { sheet: 1, count: 1 } });
    assert_eq!(st.sheets[1].model.rows, rows + 1);
    assert_eq!(st.rt[1].versions.len(), st.sheets[1].model.cells.len());
    assert!(kim.take().iter().any(|m| matches!(m, ServerMsg::RowsAdded { sheet: 1, .. })));
    assert_eq!(st.sheets[1].model.cell(CellRef::new(rows, 1)).unwrap().v, CellValue::Text("김민지".into()));
    assert_eq!(st.log.last().unwrap().sheet, 1);
}

#[tokio::test]
async fn common_settings_follow_the_first_sheet() {
    let mut st = State::new(two_sheets());
    host(&mut st, |s| {
        let meta = ConfigEdit::SetMeta {
            title: "인력·예산".into(),
            description: "안내".into(),
            requester: "박지훈".into(),
            deadline: Some("2026-10-31T18:00".into()),
        };
        s.apply(1, meta).unwrap();
        let mut p = Permissions {
            add_rows: true,
            deadline_action: DeadlineAction::StopSharing,
            ..s.sheets[1].config.permissions
        };
        p.format = collect_core::config::FormatPolicy::All;
        s.apply(1, ConfigEdit::SetPermissions { permissions: p }).unwrap();
        s.apply(1, ConfigEdit::AddPerson { name: "윤새봄".into() }).unwrap();
        s.apply(
            1,
            ConfigEdit::Brush { range: CellRange::parse("F2:F9").unwrap(), brush: collect_core::edit::Brush::Input },
        )
        .unwrap();
        assert!(s.apply(5, ConfigEdit::AddPerson { name: "x".into() }).is_err());
    });
    for slot in &st.sheets {
        assert_eq!(slot.config.title, "인력·예산");
        assert_eq!(slot.config.deadline.as_deref(), Some("2026-10-31T18:00"));
        assert_eq!(slot.config.permissions.deadline_action, DeadlineAction::StopSharing);
        assert_eq!(slot.config.assignment.extra_people, ["윤새봄"]);
    }
    assert!(st.sheets[1].config.permissions.add_rows && !st.sheets[0].config.permissions.add_rows, "시트별 권한");
    assert_eq!(st.sheets[1].config.columns[5].role, ColumnRole::Input);
    assert_eq!(st.sheets[0].config.columns[5].role, ColumnRole::Input); // 예산신청서 F는 원래 입력
    assert_eq!(st.info().title, "인력·예산");
    assert!(st.person("윤새봄").is_some());
    // 첫 시트가 아닌 시트에서 직접 고친 공통 설정은 첫 시트 값으로 돌아간다.
    host(&mut st, |s| {
        s.sheets[1].config.title = "다른 제목".into();
        s.changed();
    });
    assert_eq!(st.sheets[1].config.title, "인력·예산");
    host(&mut st, |s| s.apply(0, ConfigEdit::RemovePerson { name: "윤새봄".into() }).unwrap());
    assert!(st.sheets.iter().all(|s| s.config.assignment.extra_people.is_empty()));
    assert!(st.person("윤새봄").is_none());
}

#[tokio::test]
async fn notices_highlights_and_approvals_know_the_sheet() {
    let mut st = State::new(two_sheets());
    let mut shin = connect(&mut st, 1);
    join(&mut st, &shin, "신예준", None);
    let mut lee = connect(&mut st, 2);
    join(&mut st, &lee, "이서연", None);
    shin.take();
    lee.take();
    // 미완료: 신예준은 인력계획 필수 칸만 남았다(예산신청서에는 맡은 칸이 없다).
    host(&mut st, |s| s.send_notice("서둘러 주세요", false, NoticeTarget::Incomplete));
    assert!(shin.take().iter().any(|m| matches!(m, ServerMsg::Notice(_))));
    // 인력계획 신예준 행의 강조는 그 사람(담당자)에게만, 시트 번호와 함께.
    let r = row_of(&st, 1, "신예준");
    host(&mut st, move |s| s.highlight(1, CellRange::new(r, E, 1, 1), "확인", HighlightAudience::Owners));
    assert!(shin.take().iter().any(|m| matches!(m, ServerMsg::Highlight(h) if h.sheet == 1)));
    assert!(!lee.take().iter().any(|m| matches!(m, ServerMsg::Highlight(_))));
    assert_eq!(st.comms_snapshot().highlights[0].sheet, 1);

    // 승인하며 둘째 시트에 범위를 배정한다.
    let mut kang = connect(&mut st, 3);
    super::tests::join_unlisted(&mut st, &kang, "강다온", None);
    st.approve("강다온", Some((1, CellRange::parse("F2:F9").unwrap())), String::new()).unwrap();
    assert_eq!(st.sheets[1].config.assignment.ranges[0].person, "강다온");
    assert!(st.sheets[0].config.assignment.ranges.is_empty());
    assert!(kang.take().iter().any(|m| matches!(m, ServerMsg::Welcome(_))));
    assert!(st.approve("강다온", Some((7, CellRange::parse("F2").unwrap())), String::new()).is_err());
}

#[tokio::test]
async fn snapshots_and_restore_cover_every_sheet() {
    let mut st = State::new(two_sheets());
    let lee = connect(&mut st, 1);
    join(&mut st, &lee, "이서연", None);
    let (r0, r1) = (row_of(&st, 0, "이서연"), row_of(&st, 1, "이서연"));
    set(&mut st, &lee, 0, "a", r0, F, num(1.0));
    set(&mut st, &lee, 1, "b", r1, E, num(2.0));
    let original = st.snapshots.list[0].0.id;
    let n = host_get_restore(&mut st, original);
    assert_eq!(n, 2);
    assert_eq!(st.sheets[1].model.cell(CellRef::new(r1, E)).unwrap().v, CellValue::Empty);
    let restored: Vec<(u32, BatchOrigin)> =
        st.log.iter().filter(|e| e.origin == BatchOrigin::Restore).map(|e| (e.sheet, e.origin)).collect();
    assert_eq!(restored.iter().map(|(s, _)| *s).collect::<Vec<_>>(), [0, 1]);
    let batches: std::collections::HashSet<&str> =
        st.log.iter().filter(|e| e.origin == BatchOrigin::Restore).map(|e| e.batch.as_str()).collect();
    assert_eq!(batches.len(), 2, "복원은 시트마다 한 묶음");
}

fn host_get_restore(st: &mut State, id: u32) -> usize {
    super::tests::host_get(st, move |s| s.restore(id, "박지훈").unwrap())
}

#[tokio::test]
async fn recalc_runs_for_edits_on_sheets_without_formulas() {
    let mut st = State::new(two_sheets());
    assert!(st.rt[0].has_formulas && !st.rt[1].has_formulas && st.any_formulas);
    let (tx, mut rx) = mpsc::unbounded_channel();
    st.handle(Command::SetRecalc(Some(tx)));
    let mut lee = connect(&mut st, 1);
    join(&mut st, &lee, "이서연", None);
    let r = row_of(&st, 1, "이서연");
    set(&mut st, &lee, 1, "b", r, E, num(2.0));
    let req = rx.try_recv().expect("수식은 다른 시트를 볼 수 있어 다시 계산한다");
    assert_eq!(req.writes, [(1, CellRef::new(r, E), num(2.0))]);
    // 결과는 시트마다 보낸다.
    lee.take();
    st.handle(Command::Formulas(vec![(0, CellRef::new(2, 6), num(0.5))]));
    assert!(lee.take().iter().any(|m| matches!(m, ServerMsg::Cells { sheet: 0, by, .. } if by.is_empty())));
}

fn temp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("collect-sheets-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join(name);
    for ext in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{ext}", p.display()));
    }
    p
}

#[tokio::test]
async fn multi_sheet_session_comes_back_from_its_file() {
    let path = temp("multi.sqlite");
    let mut init = two_sheets();
    let store = Store::open(&path).unwrap();
    store.meta("workbook", r"C:\work\budget.xlsx");
    store.meta(META_SHEETS, r#"["예산신청서","인력계획"]"#);
    init.store = Some(store);
    let mut st = State::new(init);
    let lee = connect(&mut st, 1);
    join(&mut st, &lee, "이서연", None);
    let (r0, r1) = (row_of(&st, 0, "이서연"), row_of(&st, 1, "이서연"));
    set(&mut st, &lee, 1, "b1", r1, E, num(2.0));
    host(&mut st, |s| s.take_snapshot("중간"));
    set(&mut st, &lee, 0, "b2", r0, F, num(9800.0));
    set(&mut st, &lee, 1, "b3", r1, 5, CellValue::Text("증원".into())); // 스냅샷 뒤(되짚어야 한다)
    host(&mut st, move |s| {
        s.sheets[1].config.permissions.add_rows = true;
        s.sheets[1].baseline.cells[0].v = CellValue::Text("저장한 기준".into());
        s.changed();
    });
    st.store.as_ref().unwrap().flush().unwrap();
    let before: Vec<SheetSlot> = st.sheets.clone();
    let log = st.log.len();
    drop(st);

    let rec = recover(&path).unwrap();
    assert_eq!(rec.sheets, ["예산신청서", "인력계획"]);
    assert_eq!(rec.changes, log);
    let mut st = State::new(rec.init);
    assert_eq!(st.sheets.len(), 2);
    for (a, b) in st.sheets.iter().zip(&before) {
        assert_eq!(a.model, b.model);
        assert_eq!(a.config, b.config);
        assert_eq!(a.baseline, b.baseline);
    }
    assert_eq!(st.rt[1].versions[(r1 * st.sheets[1].model.cols + E) as usize], 1);
    assert_eq!(st.rt[0].versions[(r1 * st.sheets[0].model.cols + E) as usize], 0);
    let snap = st.snapshots.list[1].0.id;
    let values = super::tests::host_get(&mut st, move |s| s.snapshot_values(snap).unwrap());
    assert_eq!(values.len(), 2);
    assert_eq!(values[1][(r1 * 6 + E) as usize], num(2.0));
    // 다시 보낸 묶음은 한 번만.
    let mut again = connect(&mut st, 2);
    join(&mut st, &again, "이서연", None);
    again.take();
    set(&mut st, &again, 1, "b3", r1, 5, CellValue::Text("증원".into()));
    assert_eq!(st.log.len(), log);
}

/// 여러 시트 전(v1.2)에 만든 기록 파일: 블롭은 시트 하나, 스냅샷은 한 시트 값 배열, 변경 기록에 sheet 열이 없다.
#[tokio::test]
async fn single_sheet_files_from_before_still_recover() {
    let path = temp("old.sqlite");
    let SessionInit { sheets, .. } = fixture();
    let (model, config) = sheets.into_iter().next().unwrap();
    {
        let store = Store::open(&path).unwrap();
        store.meta("workbook", r"C:\work\budget.xlsx");
        store.meta("sheet", "예산신청서");
        store.blob(BLOB_SHEET, 0, pack_json(&model));
        store.blob(BLOB_BASELINE, 0, pack_json(&model));
        store.blob(BLOB_CONFIG, 0, pack_json(&config));
        let values: Vec<CellValue> = model.cells.iter().map(|c| c.v.clone()).collect();
        let meta =
            SnapshotMeta { id: 1, at_ms: 1, kind: SnapshotKind::Original, seq: 0, label: "불러온 원본".into() };
        store.snapshot(&meta, &pack(&values));
        store.append(&[ChangeEntry {
            seq: 1,
            at_ms: 2,
            user: "이서연".into(),
            batch: "b1".into(),
            origin: BatchOrigin::Edit,
            kind: collect_core::history::ChangeKind::Input,
            sheet: 0,
            row: 2,
            col: F,
            old: CellValue::Empty,
            new: num(9800.0),
            gone: false,
        }]);
        store.flush().unwrap();
    }
    let rec = recover(&path).unwrap();
    assert_eq!(rec.sheets, ["예산신청서"]);
    let mut st = State::new(rec.init);
    assert_eq!(st.sheets.len(), 1);
    assert_eq!(st.sheets[0].model.cell(CellRef::new(2, F)).unwrap().v, num(9800.0));
    let n = host_get_restore(&mut st, 1);
    assert_eq!(n, 1, "옛 스냅샷으로도 복원한다");
    assert_eq!(st.sheets[0].model.cell(CellRef::new(2, F)).unwrap().v, CellValue::Empty);
}

#[tokio::test]
async fn replacing_with_more_sheets_starts_over() {
    let mut st = State::new(fixture());
    let mut lee = connect(&mut st, 1);
    join(&mut st, &lee, "이서연", None);
    let r = row_of(&st, 0, "이서연");
    set(&mut st, &lee, 0, "a", r, F, num(1.0));
    let first = st.sheets[0].clone();
    lee.take();
    host(&mut st, move |s| s.replace(vec![(first.baseline.clone(), first.config.clone()), staff()]));
    assert_eq!((st.sheets.len(), st.rt.len(), st.log.len()), (2, 2, 0));
    let w = lee.welcome();
    assert_eq!(w.sheets.len(), 2);
}
