//! 7단계 세션 복구: 기록 파일에 남긴 상태로 세션을 되살린다.

use collect_core::format::{FillPatch, FormatEdit, StylePatch, apply_format};
use collect_core::history::BatchOrigin;

use super::tests::{Client, connect, fixture, host, join};
use super::*;
use crate::protocol::{CellEdit, ChatTo};

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
    let msg = ClientMsg::SetCells { batch: batch.into(), cells, origin: Some(BatchOrigin::Paste) };
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
    let r = st.access.rows_of(st.access.person("이서연").unwrap())[0];

    set(&mut st, &lee, "b1", r, F, CellValue::Number(9800.0));
    set(&mut st, &lee, "b2", r, H, CellValue::Text("간담회 4회".into()));
    host(&mut st, |s| s.take_snapshot("점심 전"));
    set(&mut st, &lee, "b3", r + 1, F, CellValue::Number(1.0)); // 스냅샷 뒤의 기록(되짚어야 한다)
    host(&mut st, move |s| {
        s.config.columns[7].required = true;
        let fill = StylePatch { fill: Some(FillPatch::Color { rgb: 0xFFF3B0 }), ..Default::default() };
        apply_format(s.model, FormatEdit::Style { range: CellRange::new(r, H, 1, 1), patch: fill }).unwrap();
        s.changed();
        s.send_notice("17시까지", true, NoticeTarget::All);
        s.highlight(CellRange::new(r, H, 1, 1), "확인", HighlightAudience::All);
        s.chat(ChatTo::All, "안녕하세요");
        s.set_meta(META_SERVER, r#"{"port":8091,"slug":"q4","running":true}"#);
        s.baseline.cells[0].v = CellValue::Text("저장한 기준".into());
        s.set_closed(true);
    });
    set(&mut st, &lee, "late", r + 2, F, CellValue::Number(2.0)); // 마감 뒤: 거절(기록 없음)
    st.store.as_ref().unwrap().flush().unwrap();
    let before_model = st.model.clone();
    let before_config = st.config.clone();
    let before_log = st.log.len();
    let before_snaps = st.snapshots.list.len();
    drop(st); // 앱이 꺼짐

    let rec = recover(&path).unwrap();
    assert_eq!(rec.workbook_id, r"C:\work\budget.xlsx");
    assert_eq!(rec.sheet, "예산신청서");
    assert_eq!(rec.server, Some(ServerMeta { port: 8091, slug: "q4".into(), running: true }));
    assert_eq!(rec.changes, before_log);
    let mut st = State::new(rec.init);
    assert_eq!(values(&st.model), values(&before_model), "되짚은 값이 같다");
    assert_eq!(st.model.styles, before_model.styles, "서식도 같다");
    assert_eq!(st.config, before_config);
    assert_eq!(st.baseline.cells[0].v, CellValue::Text("저장한 기준".into()));
    assert!(st.closed && st.read_only);
    assert_eq!(st.log.len(), before_log);
    assert_eq!(st.next_seq, before_log as u64 + 1);
    assert_eq!(st.snapshots.list.len(), before_snaps);
    let comms = st.comms_snapshot();
    assert_eq!((comms.notices.len(), comms.highlights.len(), comms.chat.len()), (1, 1, 1));
    assert_eq!(st.versions[(r * st.model.cols + F) as usize], 1);

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
    assert_eq!(st.model.cell(CellRef::new(r + 3, F)).unwrap().v, CellValue::Number(3.0));
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
    let (model, config) = (st.model.clone(), st.config.clone());
    let store = Store::open(&second).unwrap();
    host(&mut st, move |s| {
        s.replace(model, config);
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
    init.config.permissions.add_rows = true;
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
    let rows = st.model.rows;
    st.handle(Command::Client { conn: 1, msg: ClientMsg::AddRows { count: 1 } });
    set(&mut st, &a, "n1", rows, F, CellValue::Number(7.0));
    st.store.as_ref().unwrap().flush().unwrap();
    drop(st);

    let mut st = State::new(recover(&path).unwrap().init);
    assert_eq!(st.model.rows, rows + 1);
    assert_eq!(st.model.cell(CellRef::new(rows, F)).unwrap().v, CellValue::Number(7.0));
    assert_eq!(st.access.row_owner(rows), st.access.person("김민수"));
    let mut back = connect(&mut st, 1);
    join_unlisted(&mut st, &back, "김민수", Some(&token));
    assert_eq!(back.welcome().token, token, "다시 승인받지 않는다");
    let mut other = connect(&mut st, 2);
    join_unlisted(&mut st, &other, "김민수", Some("다른 토큰"));
    assert!(matches!(other.take()[..], [ServerMsg::Pending { .. }]));
}
