//! 6단계 세션 기능: 공지·셀 강조·채팅, 스냅샷·복원, 묶음 출처, 저장소, 호스트 피드.

use collect_core::history::{BatchOrigin, ChangeKind};
use collect_store::SnapshotKind;

use super::tests::{Client, connect, fixture, host, host_get, join};
use super::*;
use crate::protocol::{CellEdit, ChatTo};

const F: u32 = 5;

fn setup(names: &[&str]) -> (State, Vec<Client>) {
    setup_with(fixture(), names)
}

fn setup_with(init: SessionInit, names: &[&str]) -> (State, Vec<Client>) {
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

fn first_row(st: &State, name: &str) -> u32 {
    st.rt[0].access.rows_of(st.rt[0].access.person(name).unwrap())[0]
}

fn set(st: &mut State, c: &Client, batch: &str, cells: Vec<CellEdit>, origin: BatchOrigin) {
    let msg = ClientMsg::SetCells { sheet: 0, batch: batch.into(), cells, origin: Some(origin), layout: None };
    st.handle(Command::Client { conn: c.conn, msg });
}

fn edit(row: u32, value: CellValue) -> CellEdit {
    CellEdit { row, col: F, value, base: 0 }
}

fn notices(msgs: &[ServerMsg]) -> Vec<NoticeView> {
    msgs.iter()
        .filter_map(|m| match m {
            ServerMsg::Notice(n) => Some(n.clone()),
            _ => None,
        })
        .collect()
}

fn chats(msgs: &[ServerMsg]) -> Vec<ChatMessage> {
    msgs.iter()
        .filter_map(|m| match m {
            ServerMsg::Chat(c) => Some(c.clone()),
            _ => None,
        })
        .collect()
}

use crate::protocol::{ChatMessage, ChatReaction, ChatReactionMark, NoticeView};

#[tokio::test]
async fn notices_reach_targets_and_count_reads() {
    let (mut st, mut c) = setup(&["이서연", "최도윤"]);
    // 이서연의 필수 칸을 모두 채워 '미완료자'에서 빠지게 한다.
    let lee = st.rt[0].access.person("이서연").unwrap();
    let rows: Vec<u32> = st.rt[0].access.rows_of(lee);
    let cells: Vec<(CellRef, CellValue)> = rows.iter().map(|&r| (CellRef::new(r, F), CellValue::Number(1.0))).collect();
    host(&mut st, move |s| s.host_set("호스트", BatchOrigin::Revert, 0, cells));
    for x in &mut c {
        x.take();
    }

    host(&mut st, |s| s.send_notice("오늘 17시까지 입력해 주세요", true, NoticeTarget::Incomplete));
    assert!(notices(&c[0].take()).is_empty(), "다 채운 사람은 받지 않는다");
    let got = notices(&c[1].take());
    assert_eq!(got.len(), 1);
    assert!(got[0].important && !got[0].read);
    assert_eq!(got[0].from, "박지훈");
    let id = got[0].id;

    host(&mut st, |s| s.send_notice("전체 공지", false, NoticeTarget::All));
    assert_eq!(notices(&c[0].take()).len(), 1);
    host(&mut st, |s| s.send_notice("  ", false, NoticeTarget::All));
    assert!(notices(&c[1].take()).iter().all(|n| n.text == "전체 공지"), "빈 공지는 보내지 않는다");

    st.handle(Command::Client { conn: c[1].conn, msg: ClientMsg::AckNotice { id } });
    let snap = st.comms_snapshot();
    assert_eq!(snap.notices[0].recipients.len(), 5);
    assert!(!snap.notices[0].recipients.contains(&"이서연".to_string()));
    assert_eq!(snap.notices[0].read, ["최도윤"]);
    // 나중에 들어온 사람도 자기에게 온 공지를 받는다(읽음 표시와 함께).
    let mut again = connect(&mut st, 9);
    join(&mut st, &again, "김민지", None);
    let w = again.welcome();
    assert_eq!(
        w.notices.iter().map(|n| n.text.as_str()).collect::<Vec<_>>(),
        ["오늘 17시까지 입력해 주세요", "전체 공지"]
    );

    host(&mut st, |s| {
        s.send_notice("한 사람에게", false, NoticeTarget::People { names: vec!["김민지".into(), "없는 이름".into()] })
    });
    assert_eq!(st.comms_snapshot().notices.last().unwrap().recipients, ["김민지"]);
}

#[tokio::test]
async fn highlights_go_to_owners_and_can_be_cleared() {
    let (mut st, mut c) = setup(&["이서연", "최도윤"]);
    let r = first_row(&st, "이서연");
    let range = CellRange::new(r, 7, 1, 1);
    host(&mut st, move |s| s.highlight(0, range, "교육 과정명과 인원을 적어 주세요", HighlightAudience::Owners));
    let lee = c[0].take();
    let h = lee
        .iter()
        .find_map(|m| match m {
            ServerMsg::Highlight(h) => Some(h.clone()),
            _ => None,
        })
        .expect("담당자에게 간다");
    assert_eq!((h.range, h.acked), (range, false));
    assert!(!c[1].take().iter().any(|m| matches!(m, ServerMsg::Highlight(_))), "담당자가 아니면 받지 않는다");

    st.handle(Command::Client { conn: c[0].conn, msg: ClientMsg::AckHighlight { id: h.id } });
    let snap = st.comms_snapshot();
    assert_eq!(snap.highlights[0].acked[0].0, "이서연");
    assert!(st.welcome("이서연").unwrap().highlights[0].acked);

    host(&mut st, move |s| s.highlight(0, range, "모두 보세요", HighlightAudience::All));
    assert!(c[1].take().iter().any(|m| matches!(m, ServerMsg::Highlight(_))));
    host(&mut st, move |s| s.clear_highlight(h.id));
    assert!(c[0].take().iter().any(|m| matches!(m, ServerMsg::HighlightCleared { id } if *id == h.id)));
    assert_eq!(st.welcome("이서연").unwrap().highlights.len(), 1, "해제한 강조는 빠진다");
}

#[tokio::test]
async fn owners_highlight_on_unowned_cells_goes_to_everyone() {
    let mut init = fixture();
    init.sheets[0].1.assignment.key_col = None;
    init.sheets[0].1.assignment.extra_people = vec!["가".into(), "나".into()];
    let r = init.sheets[0].0.header_rows;
    init.sheets[0].1.assignment.ranges.push(collect_core::config::RangeAssignment {
        range: CellRange::new(r, F, 1, 1),
        person: "가".into(),
        note: String::new(),
    });
    let (mut st, mut c) = setup_with(init, &["가", "나"]);
    let got = |c: &mut Client| c.take().iter().any(|m| matches!(m, ServerMsg::Highlight(_)));

    host(&mut st, move |s| s.highlight(0, CellRange::new(r, F, 1, 1), "", HighlightAudience::Owners));
    assert!(got(&mut c[0]));
    assert!(!got(&mut c[1]), "범위 배정된 칸은 그 사람에게만");

    host(&mut st, move |s| s.highlight(0, CellRange::new(r, F, 2, 1), "", HighlightAudience::Owners));
    assert!(got(&mut c[0]) && got(&mut c[1]), "담당자 없는 칸이 섞여 있으면 모두에게");
    assert_eq!(st.comms_snapshot().highlights.last().unwrap().recipients, ["가", "나"]);
}

#[tokio::test]
async fn hidden_policy_limits_highlight_audience() {
    let mut init = fixture();
    init.sheets[0].1.permissions.others_rows = OthersRows::Hidden;
    let (mut st, mut c) = setup_with(init, &["이서연", "최도윤"]);
    let r = first_row(&st, "이서연");
    host(&mut st, move |s| s.highlight(0, CellRange::new(r, F, 1, 1), "", HighlightAudience::All));
    assert!(c[0].take().iter().any(|m| matches!(m, ServerMsg::Highlight(_))));
    assert!(
        !c[1].take().iter().any(|m| matches!(m, ServerMsg::Highlight(_))),
        "남의 행은 보이지 않으니 강조도 받지 않는다"
    );
}

#[tokio::test]
async fn chat_routes_to_everyone_host_or_one_person() {
    let (mut st, mut c) = setup(&["이서연", "최도윤"]);
    let say = |st: &mut State, c: &Client, to: ChatTo, text: &str| {
        st.handle(Command::Client { conn: c.conn, msg: ClientMsg::Chat { to, text: text.into() } });
    };
    say(&mut st, &c[0], ChatTo::All, "  출장비 행 제가 지웠어요  ");
    for x in &mut c {
        let got = chats(&x.take());
        assert_eq!(got.len(), 1);
        assert_eq!(
            (got[0].from.as_str(), got[0].text.as_str(), got[0].host),
            ("이서연", "출장비 행 제가 지웠어요", false)
        );
    }
    say(&mut st, &c[1], ChatTo::Host, "호스트님 질문");
    assert_eq!(chats(&c[1].take()).len(), 1, "보낸 사람에게는 돌아온다");
    assert!(chats(&c[0].take()).is_empty(), "다른 참여자는 못 본다");
    say(&mut st, &c[1], ChatTo::Person { name: "이서연".into() }, "몰래");
    assert!(c[1].take().iter().any(|m| matches!(m, ServerMsg::Error { .. })));

    host(&mut st, |s| s.chat(ChatTo::Person { name: "최도윤".into() }, "복원했습니다 #F7:H7"));
    assert_eq!(chats(&c[1].take())[0].text, "복원했습니다 #F7:H7");
    assert!(chats(&c[0].take()).is_empty());
    let snap = st.comms_snapshot();
    assert_eq!(snap.chat.len(), 3, "호스트는 모든 채팅을 본다");
    assert!(snap.chat[2].host);
    let w = st.welcome("이서연").unwrap();
    assert_eq!(w.chat.len(), 1, "이서연에게 보이는 것은 전체 채팅 하나");

    // 너무 빨리 보내면 막는다.
    for i in 0..12 {
        say(&mut st, &c[0], ChatTo::All, &format!("{i}"));
    }
    assert!(c[0].take().iter().any(|m| matches!(m, ServerMsg::Error { message } if message.contains("너무 빨리"))));
    // 길면 자른다.
    host(&mut st, |s| s.chat(ChatTo::All, &"가".repeat(5000)));
    assert_eq!(st.comms_snapshot().chat.last().unwrap().text.chars().count(), comms::MAX_CHAT_CHARS);
}

fn reactions(msgs: &[ServerMsg]) -> Vec<(u32, Vec<ChatReactionMark>)> {
    msgs.iter()
        .filter_map(|m| match m {
            ServerMsg::ChatReactions { id, reactions } => Some((*id, reactions.clone())),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn chat_reactions_toggle_for_others_messages_only() {
    let (mut st, mut c) = setup(&["이서연", "최도윤", "한지우"]);
    let say = |st: &mut State, c: &Client, to: ChatTo, text: &str| {
        st.handle(Command::Client { conn: c.conn, msg: ClientMsg::Chat { to, text: text.into() } });
    };
    let react = |st: &mut State, c: &Client, id: u32, kind: ChatReaction| {
        st.handle(Command::Client { conn: c.conn, msg: ClientMsg::ChatReact { id, kind } });
    };
    say(&mut st, &c[0], ChatTo::All, "다 적었어요");
    let all = chats(&c[0].take())[0].id;
    for x in &mut c[1..] {
        x.take();
    }

    // 남의 글: 모두에게 리액션 목록이 간다. 한 사람이 여러 종류를 달 수 있다.
    react(&mut st, &c[1], all, ChatReaction::Good);
    react(&mut st, &c[1], all, ChatReaction::Thanks);
    for x in &mut c {
        let got = reactions(&x.take());
        assert_eq!(got.len(), 2);
        assert_eq!(got[1].0, all);
        assert_eq!(
            got[1].1.iter().map(|r| (r.kind, r.by.as_str(), r.host)).collect::<Vec<_>>(),
            vec![(ChatReaction::Good, "최도윤", false), (ChatReaction::Thanks, "최도윤", false)]
        );
    }
    // 같은 종류를 다시 누르면 뗀다.
    react(&mut st, &c[1], all, ChatReaction::Good);
    assert_eq!(reactions(&c[0].take())[0].1.iter().map(|r| r.kind).collect::<Vec<_>>(), vec![ChatReaction::Thanks]);
    for x in &mut c[1..] {
        x.take();
    }
    // 내 글에는 달 수 없다.
    react(&mut st, &c[0], all, ChatReaction::Ok);
    assert!(reactions(&c[0].take()).is_empty());

    // 호스트에게만 보낸 글: 다른 참여자는 달 수 없고, 호스트는 단다(보낸 사람에게만 알린다).
    say(&mut st, &c[1], ChatTo::Host, "질문 있어요");
    let private = chats(&c[1].take())[0].id;
    react(&mut st, &c[2], private, ChatReaction::Yes);
    assert!(reactions(&c[1].take()).is_empty());
    host(&mut st, move |s| s.chat_react(private, ChatReaction::Yes));
    let got = reactions(&c[1].take());
    assert_eq!(got[0].1.len(), 1);
    assert!(got[0].1[0].host);
    assert!(reactions(&c[0].take()).is_empty() && reactions(&c[2].take()).is_empty());
    // 호스트는 이름이 바뀌어도 같은 사람: 다시 누르면 뗀다. 호스트 글에는 호스트가 달 수 없다.
    host(&mut st, move |s| s.chat_react(private, ChatReaction::Yes));
    assert!(reactions(&c[1].take())[0].1.is_empty());
    host(&mut st, |s| s.chat(ChatTo::All, "확인했습니다"));
    let from_host = st.comms_snapshot().chat.last().unwrap().id;
    host(&mut st, move |s| s.chat_react(from_host, ChatReaction::Ok));
    react(&mut st, &c[2], from_host, ChatReaction::Ok);

    // 호스트 화면·입장(welcome)에도 리액션이 실린다.
    let snap = st.comms_snapshot();
    let kinds = |id: u32| snap.chat.iter().find(|m| m.id == id).unwrap().reactions.clone();
    assert_eq!(kinds(all).len(), 1);
    assert_eq!(kinds(from_host).iter().map(|r| (r.by.as_str(), r.host)).collect::<Vec<_>>(), vec![("한지우", false)]);
    let w = st.welcome("이서연").unwrap();
    assert_eq!(w.chat.iter().find(|m| m.id == all).unwrap().reactions[0].kind, ChatReaction::Thanks);
    // 없는 채팅은 그냥 둔다.
    react(&mut st, &c[2], 9999, ChatReaction::Ok);
    assert!(reactions(&c[2].take()).iter().all(|(id, _)| *id != 9999));

    // 너무 빨리 누르면 막는다.
    for _ in 0..40 {
        react(&mut st, &c[2], all, ChatReaction::Sad);
    }
    assert!(c[2].take().iter().any(|m| matches!(m, ServerMsg::Error { message } if message.contains("너무 빨리"))));
}

#[tokio::test]
async fn origins_and_kinds_are_recorded() {
    let (mut st, c) = setup(&["이서연"]);
    let r = first_row(&st, "이서연");
    set(
        &mut st,
        &c[0],
        "p",
        vec![edit(r, CellValue::Number(1.0)), edit(r + 1, CellValue::Number(2.0))],
        BatchOrigin::Paste,
    );
    set(&mut st, &c[0], "x", vec![edit(r, CellValue::Empty)], BatchOrigin::Clear);
    set(&mut st, &c[0], "u", vec![edit(r, CellValue::Number(1.0))], BatchOrigin::Undo);
    // 참여자는 '복원'으로 표시할 수 없다.
    set(&mut st, &c[0], "r", vec![edit(r + 1, CellValue::Number(3.0))], BatchOrigin::Restore);
    let got: Vec<(BatchOrigin, ChangeKind)> = st.log.iter().map(|e| (e.origin, e.kind)).collect();
    assert_eq!(
        got,
        [
            (BatchOrigin::Paste, ChangeKind::Input),
            (BatchOrigin::Paste, ChangeKind::Input),
            (BatchOrigin::Clear, ChangeKind::Delete),
            (BatchOrigin::Undo, ChangeKind::Revert),
            (BatchOrigin::Edit, ChangeKind::Edit),
        ]
    );
}

#[tokio::test]
async fn snapshots_restore_and_can_be_undone() {
    let (mut st, mut c) = setup(&["이서연"]);
    let r = first_row(&st, "이서연");
    let snaps = st.snapshots.list.iter().map(|(m, _)| m.kind).collect::<Vec<_>>();
    assert_eq!(snaps, [SnapshotKind::Original]);
    set(&mut st, &c[0], "a", vec![edit(r, CellValue::Number(9800.0))], BatchOrigin::Edit);
    host(&mut st, |s| s.take_snapshot("점심 전"));
    set(
        &mut st,
        &c[0],
        "b",
        vec![edit(r, CellValue::Number(1.0)), edit(r + 1, CellValue::Number(2.0))],
        BatchOrigin::Edit,
    );
    c[0].take();
    let manual = st.snapshots.list[1].0.clone();
    assert_eq!((manual.kind, manual.label.as_str(), manual.seq), (SnapshotKind::Manual, "점심 전", 1));

    let id = manual.id;
    assert_eq!(host_get(&mut st, move |s| s.restore(id, "호스트")), Ok(2));
    assert_eq!(st.sheets[0].model.cell(CellRef::new(r, F)).unwrap().v, CellValue::Number(9800.0));
    assert_eq!(st.sheets[0].model.cell(CellRef::new(r + 1, F)).unwrap().v, CellValue::Empty);
    assert_eq!(st.snapshots.list.last().unwrap().0.kind, SnapshotKind::BeforeRestore);
    assert!(st.log.iter().rev().take(2).all(|e| e.origin == BatchOrigin::Restore && e.kind == ChangeKind::Revert));
    assert!(c[0].take().iter().any(|m| matches!(m, ServerMsg::Cells { by, .. } if by == "호스트")));

    // 복원 직전 스냅샷으로 복원하면 복원을 되돌린다.
    let before = st.snapshots.list.last().unwrap().0.id;
    host(&mut st, move |s| {
        s.restore(before, "호스트").unwrap();
    });
    assert_eq!(st.sheets[0].model.cell(CellRef::new(r, F)).unwrap().v, CellValue::Number(1.0));
    assert!(host_get(&mut st, |s| s.restore(999, "호스트")).is_err());
}

#[tokio::test(start_paused = true)]
async fn auto_snapshot_only_when_something_changed() {
    let (mut st, c) = setup(&["이서연"]);
    let r = first_row(&st, "이서연");
    tokio::time::advance(AUTO_SNAPSHOT + Duration::from_secs(1)).await;
    st.handle(Command::Tick);
    assert_eq!(st.snapshots.list.len(), 1, "바뀐 것이 없으면 남기지 않는다");
    set(&mut st, &c[0], "a", vec![edit(r, CellValue::Number(1.0))], BatchOrigin::Edit);
    st.handle(Command::Tick);
    assert_eq!(st.snapshots.list.len(), 1, "15분이 다시 지나야 한다");
    tokio::time::advance(AUTO_SNAPSHOT).await;
    st.handle(Command::Tick);
    assert_eq!(st.snapshots.list.last().unwrap().0.kind, SnapshotKind::Auto);
}

#[tokio::test]
async fn store_keeps_changes_and_snapshots() {
    let path = std::env::temp_dir().join(format!("collect-server-store-{}.sqlite", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let mut init = fixture();
    init.store = Some(Store::open(&path).unwrap());
    let (mut st, c) = setup_with(init, &["이서연"]);
    let r = first_row(&st, "이서연");
    set(&mut st, &c[0], "a", vec![edit(r, CellValue::Number(9800.0))], BatchOrigin::Edit);
    host(&mut st, |s| s.take_snapshot(""));
    st.store.as_ref().unwrap().flush().unwrap();
    let saved = collect_store::load(&path).unwrap();
    assert_eq!(saved.changes.len(), 1);
    assert_eq!(saved.changes[0].new, CellValue::Number(9800.0));
    assert_eq!(
        saved.snapshots.iter().map(|(m, _)| m.kind).collect::<Vec<_>>(),
        [SnapshotKind::Original, SnapshotKind::Manual]
    );

    // 다시 불러와 확정하면 새 파일: 지금까지의 기록을 옮기고 원본 스냅샷부터 다시.
    let path2 = path.with_extension("2.sqlite");
    let _ = std::fs::remove_file(&path2);
    let store2 = Store::open(&path2).unwrap();
    host(&mut st, move |s| s.set_store(store2));
    st.store.as_ref().unwrap().flush().unwrap();
    let saved = collect_store::load(&path2).unwrap();
    assert_eq!((saved.changes.len(), saved.snapshots.len()), (1, 1));
    drop(st);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(&path2);
}

#[tokio::test]
async fn host_feed_carries_cell_changes_and_resets() {
    let (mut st, c) = setup(&["이서연"]);
    let mut feed = st.host_feed.subscribe();
    let r = first_row(&st, "이서연");
    set(&mut st, &c[0], "a", vec![edit(r, CellValue::Text("9,800".into()))], BatchOrigin::Edit);
    match feed.try_recv().unwrap() {
        HostEvent::Cells { by, cells, .. } => {
            assert_eq!(by, "이서연");
            assert_eq!(cells[0].value, CellValue::Number(9800.0));
        }
        other => panic!("{other:?}"),
    }
    host(&mut st, |s| s.changed());
    assert_eq!(feed.try_recv().unwrap(), HostEvent::Reset);
    st.handle(Command::Formulas { values: vec![(0, CellRef::new(r, 6), CellValue::Number(0.5))], epoch: None });
    assert!(matches!(feed.try_recv().unwrap(), HostEvent::Cells { by, .. } if by.is_empty()));
}
