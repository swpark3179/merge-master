//! 7단계 세션 복구: 기록 파일에 남긴 상태로 세션을 되살린다.

use collect_core::format::{FillPatch, FormatEdit, StylePatch, apply_format};
use collect_core::history::BatchOrigin;

use super::tests::{Client, connect, fixture, host, join};
use super::*;
use crate::protocol::{CellEdit, ChatReaction, ChatTo};

const F: u32 = 5;
const H: u32 = 7;

fn temp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("collect-recover-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join(name);
    for ext in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{ext}", p.display()));
    }
    p
}

fn set(st: &mut State, c: &Client, batch: &str, row: u32, col: u32, value: CellValue) {
    let cells = vec![CellEdit { row, col, value, base: 0 }];
    let msg =
        ClientMsg::SetCells { sheet: 0, batch: batch.into(), cells, origin: Some(BatchOrigin::Paste), layout: None };
    st.handle(Command::Client { conn: c.conn, msg });
}

fn values(m: &SheetModel) -> Vec<CellValue> {
    m.cells.iter().map(|c| c.v.clone()).collect()
}

#[tokio::test]
async fn session_comes_back_from_its_file() {
    let path = temp("a.sqlite");
    let mut init = fixture();
    let store = Store::open(&path).unwrap();
    store.meta("workbook", r"C:\work\budget.xlsx");
    store.meta("sheet", "예산신청서");
    init.store = Some(store);
    let mut st = State::new(init);
    let mut lee = connect(&mut st, 1);
    join(&mut st, &lee, "이서연", None);
    lee.take();
    let r = st.rt[0].access.rows_of(st.rt[0].access.person("이서연").unwrap())[0];

    set(&mut st, &lee, "b1", r, F, CellValue::Number(9800.0));
    set(&mut st, &lee, "b2", r, H, CellValue::Text("간담회 4회".into()));
    host(&mut st, |s| s.take_snapshot("점심 전"));
    set(&mut st, &lee, "b3", r + 1, F, CellValue::Number(1.0)); // 스냅샷 뒤의 기록(되짚어야 한다)
    host(&mut st, move |s| {
        s.sheets[0].config.columns[7].required = true;
        let fill = StylePatch { fill: Some(FillPatch::Color { rgb: 0xFFF3B0 }), ..Default::default() };
        apply_format(&mut s.sheets[0].model, FormatEdit::Style { range: CellRange::new(r, H, 1, 1), patch: fill })
            .unwrap();
        s.changed();
        s.send_notice("17시까지", true, NoticeTarget::All);
        s.highlight(0, CellRange::new(r, H, 1, 1), "확인", HighlightAudience::All);
        s.chat(ChatTo::All, "안녕하세요");
        s.set_meta(META_SERVER, r#"{"port":8091,"slug":"q4","running":true}"#);
        s.sheets[0].baseline.cells[0].v = CellValue::Text("저장한 기준".into());
        s.set_closed(true);
    });
    let hello = st.comms_snapshot().chat[0].id;
    let msg = ClientMsg::ChatReact { id: hello, kind: ChatReaction::Thanks };
    st.handle(Command::Client { conn: lee.conn, msg });
    set(&mut st, &lee, "late", r + 2, F, CellValue::Number(2.0)); // 마감 뒤: 거절(기록 없음)
    st.store.as_ref().unwrap().flush().unwrap();
    let before_model = st.sheets[0].model.clone();
    let before_config = st.sheets[0].config.clone();
    let before_log = st.log.len();
    let before_snaps = st.snapshots.list.len();
    drop(st); // 앱이 꺼짐

    let rec = recover(&path).unwrap();
    assert_eq!(rec.workbook_id, r"C:\work\budget.xlsx");
    assert_eq!(rec.sheets, ["예산신청서"]);
    assert_eq!(rec.server, Some(ServerMeta { port: 8091, slug: "q4".into(), running: true }));
    assert_eq!(rec.changes, before_log);
    let mut st = State::new(rec.init);
    assert_eq!(values(&st.sheets[0].model), values(&before_model), "되짚은 값이 같다");
    assert_eq!(st.sheets[0].model.styles, before_model.styles, "서식도 같다");
    assert_eq!(st.sheets[0].config, before_config);
    assert_eq!(st.sheets[0].baseline.cells[0].v, CellValue::Text("저장한 기준".into()));
    assert!(st.closed && st.read_only);
    assert_eq!(st.log.len(), before_log);
    assert_eq!(st.next_seq, before_log as u64 + 1);
    assert_eq!(st.snapshots.list.len(), before_snaps);
    let comms = st.comms_snapshot();
    assert_eq!((comms.notices.len(), comms.highlights.len(), comms.chat.len()), (1, 1, 1));
    assert_eq!(comms.chat[0].reactions[0].kind, ChatReaction::Thanks, "채팅 리액션도 남는다");
    assert_eq!(st.rt[0].versions[(r * st.sheets[0].model.cols + F) as usize], 1);

    // 참여자가 다시 들어오면 공지·강조·채팅을 받고, 보내지 못했던 묶음(b3)을 다시 보내도 한 번만 반영된다.
    host(&mut st, |s| s.set_closed(false));
    let mut again = connect(&mut st, 2);
    join(&mut st, &again, "이서연", Some("옛 토큰"));
    let w = again.welcome();
    assert_eq!((w.notices.len(), w.highlights.len(), w.chat.len()), (1, 1, 1));
    set(&mut st, &again, "b3", r + 1, F, CellValue::Number(1.0));
    assert_eq!(st.log.len(), before_log, "같은 묶음은 다시 반영하지 않는다");
    set(&mut st, &again, "b4", r + 3, F, CellValue::Number(3.0));
    assert_eq!(st.log.last().unwrap().seq, before_log as u64 + 1, "기록 번호는 이어진다");
    st.store.as_ref().unwrap().flush().unwrap();
    drop(st);

    // 한 번 더 되살려도 이어서 남긴 것이 있다.
    let rec = recover(&path).unwrap();
    assert_eq!(rec.changes, before_log + 1);
    let st = State::new(rec.init);
    assert!(!st.closed);
    assert_eq!(st.sheets[0].model.cell(CellRef::new(r + 3, F)).unwrap().v, CellValue::Number(3.0));
}

#[tokio::test]
async fn overwritten_formulas_stay_values_after_recover() {
    let path = temp("formula.sqlite");
    let mut init = fixture();
    init.sheets[0].1.columns[6].role = collect_core::config::ColumnRole::Input;
    init.store = Some(Store::open(&path).unwrap());
    let mut st = State::new(init);
    let mut lee = connect(&mut st, 1);
    join(&mut st, &lee, "이서연", None);
    lee.take();
    let r = st.rt[0].access.rows_of(st.rt[0].access.person("이서연").unwrap())[0];
    set(&mut st, &lee, "b1", r, 6, CellValue::Number(3.0));
    // 시트를 남기기 전에 꺼진 경우: 기록만 있는 수식 칸 쓰기(기록 재생)도 수식을 버린다.
    let mut late = st.log.last().unwrap().clone();
    (late.seq, late.row, late.new) = (st.next_seq, r + 1, CellValue::Number(4.0));
    st.store.as_ref().unwrap().append(&[late]);
    st.store.as_ref().unwrap().flush().unwrap();
    drop(st);
    let rec = recover(&path).unwrap();
    let m = &rec.init.sheets[0].0;
    for (row, v) in [(r, 3.0), (r + 1, 4.0)] {
        let g = m.cell(CellRef::new(row, 6)).unwrap();
        assert_eq!((g.v.clone(), g.f.is_none()), (CellValue::Number(v), true), "{row}행");
    }
    assert!(m.cell(CellRef::new(r + 2, 6)).unwrap().f.is_some(), "고치지 않은 수식 칸은 그대로");
}

#[test]
fn files_without_a_sheet_cannot_be_recovered() {
    let path = temp("empty.sqlite");
    let store = Store::open(&path).unwrap();
    store.flush().unwrap();
    drop(store);
    assert!(recover(&path).is_err());
}

#[tokio::test]
async fn recommitting_while_serving_keeps_the_service_in_the_new_file() {
    let (first, second) = (temp("first.sqlite"), temp("second.sqlite"));
    let mut init = fixture();
    init.store = Some(Store::open(&first).unwrap());
    let mut st = State::new(init);
    host(&mut st, |s| s.set_meta(META_SERVER, r#"{"port":8091,"slug":"q4","running":true}"#));
    // 서비스 중에 시트를 다시 확정 → 새 기록 파일
    let (model, config) = (st.sheets[0].model.clone(), st.sheets[0].config.clone());
    let store = Store::open(&second).unwrap();
    host(&mut st, move |s| {
        s.replace(vec![(model, config)]);
        s.set_store(store);
    });
    st.store.as_ref().unwrap().flush().unwrap();
    drop(st);
    let rec = recover(&second).unwrap();
    assert_eq!(rec.server, Some(ServerMeta { port: 8091, slug: "q4".into(), running: true }));
    // 되살린 세션이 또 새 파일을 받아도 서비스 상태를 옮긴다.
    let third = temp("third.sqlite");
    let mut st = State::new(rec.init);
    let store = Store::open(&third).unwrap();
    host(&mut st, move |s| s.set_store(store));
    st.store.as_ref().unwrap().flush().unwrap();
    drop(st);
    assert_eq!(
        collect_store::peek(&third).unwrap().meta.get(META_SERVER).map(String::as_str),
        Some(r#"{"port":8091,"slug":"q4","running":true}"#)
    );
}

/// 승인받은 브라우저(토큰)와 참여자가 추가한 행은 앱을 다시 켜도 남는다.
#[tokio::test]
async fn approved_tokens_and_added_rows_come_back() {
    use super::tests::join_unlisted;
    let path = temp("tokens.sqlite");
    let mut init = fixture();
    init.store = Some(Store::open(&path).unwrap());
    init.sheets[0].1.permissions.add_rows = true;
    let mut st = State::new(init);
    let mut a = connect(&mut st, 1);
    join_unlisted(&mut st, &a, "김민수", None);
    st.handle(Command::Approve {
        name: "김민수".into(),
        range: None,
        note: String::new(),
        reply: tokio::sync::oneshot::channel().0,
    });
    let token = a.welcome().token;
    let rows = st.sheets[0].model.rows;
    st.handle(Command::Client { conn: 1, msg: ClientMsg::AddRows { sheet: 0, count: 1 } });
    set(&mut st, &a, "n1", rows, F, CellValue::Number(7.0));
    st.store.as_ref().unwrap().flush().unwrap();
    drop(st);

    let mut st = State::new(recover(&path).unwrap().init);
    assert_eq!(st.sheets[0].model.rows, rows + 1);
    assert_eq!(st.sheets[0].model.cell(CellRef::new(rows, F)).unwrap().v, CellValue::Number(7.0));
    assert_eq!(st.rt[0].access.row_owner(rows), st.rt[0].access.person("김민수"));
    let mut back = connect(&mut st, 1);
    join_unlisted(&mut st, &back, "김민수", Some(&token));
    assert_eq!(back.welcome().token, token, "다시 승인받지 않는다");
    let mut other = connect(&mut st, 2);
    join_unlisted(&mut st, &other, "김민수", Some("다른 토큰"));
    assert!(matches!(other.take()[..], [ServerMsg::Pending { .. }]));
}

/// 업무 id(호스트가 남긴 메타)는 되살린 세션에 남고, 다시 확정해 새 파일로 옮겨도 따라간다.
#[tokio::test]
async fn job_id_survives_recover_and_new_store() {
    let path = temp("job.sqlite");
    let mut init = fixture();
    init.store = Some(Store::open(&path).unwrap());
    let mut st = State::new(init);
    assert_eq!(st.info().job, None, "업무 id 없음(개발 서버·옛 파일)");
    host(&mut st, |s| s.set_meta(META_JOB, "20261011-093012-123"));
    assert_eq!(st.info().job.as_deref(), Some("20261011-093012-123"));
    st.store.as_ref().unwrap().flush().unwrap();
    drop(st);

    let rec = recover(&path).unwrap();
    assert_eq!(rec.meta.get(META_JOB).map(String::as_str), Some("20261011-093012-123"));
    let mut st = State::new(rec.init);
    assert_eq!(st.info().job.as_deref(), Some("20261011-093012-123"), "되살린 세션도 같은 업무");

    // 다시 확정: 새 기록 파일에도 업무 id가 남는다.
    let next = temp("job-next.sqlite");
    let store = Store::open(&next).unwrap();
    host(&mut st, move |s| s.set_store(store));
    st.store.as_ref().unwrap().flush().unwrap();
    drop(st);
    let peek = collect_store::peek(&next).unwrap();
    assert_eq!(peek.meta.get(META_JOB).map(String::as_str), Some("20261011-093012-123"));
}

#[tokio::test]
async fn backup_copy_can_be_recovered_elsewhere() {
    // 에이전트로 넘길 때: 지금 상태를 모두 남긴 사본을 만들고, 그 사본으로 다른 곳에서 되살린다.
    let path = temp("backup-src.sqlite");
    let copy = temp("backup-copy.sqlite");
    let mut init = fixture();
    let store = Store::open(&path).unwrap();
    store.meta("workbook", r"C:\work\budget.xlsx");
    init.store = Some(store);
    let session = SessionHandle::spawn(init);
    session
        .with(|s| {
            s.sheets[0].config.title = "넘길 업무".into();
            s.changed();
            let at = CellRef::new(2, F);
            s.host_set("박지훈", BatchOrigin::Edit, 0, vec![(at, CellValue::Number(42.0))]);
        })
        .await
        .unwrap();
    session.backup_store(copy.clone()).await.unwrap();
    assert!(session.backup_store(copy.clone()).await.is_err(), "같은 대상에 다시 만들지 않는다");
    let rec = recover(&copy).unwrap();
    assert_eq!(rec.workbook_id, r"C:\work\budget.xlsx");
    let (model, config) = &rec.init.sheets[0];
    assert_eq!(config.title, "넘길 업무");
    assert_eq!(model.cell(CellRef::new(2, F)).unwrap().v, CellValue::Number(42.0));
    let parts = SessionHandle::spawn(rec.init).save_parts().await.unwrap();
    assert_eq!(parts.history.len(), 1);
    assert_eq!(parts.sheets[0].current.cell(CellRef::new(2, F)).unwrap().v, CellValue::Number(42.0));
    let names = vec![parts.sheets[0].current.sheet.clone()];
    let plans = parts.plan(&names, ExportOptions { history: true, ..Default::default() }, 0, &[]);
    assert_eq!(plans[0].1.cells, [CellRef::new(2, F)]);
    assert!(plans[0].1.history.is_some());

    // 기록 파일이 없는 세션은 넘길 수 없다.
    let memory = SessionHandle::spawn(fixture());
    assert!(memory.backup_store(temp("none.sqlite")).await.is_err());
}
