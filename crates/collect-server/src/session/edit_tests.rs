//! 편집 단위 테스트: 셀 수정·검사·잠금·충돌·마감·숨김 정책·이력·서식·수식 결과.

use collect_core::config::{FormatPolicy, RangeAssignment};
use collect_core::format::{FillPatch, StylePatch};

use super::tests::{Client, connect, fixture, host, join};
use super::*;
use crate::protocol::{CellEdit, CellUpdate, Rejection, StyleChange};

const F: u32 = 5; // 신청 금액(숫자, 필수)
const G: u32 = 6; // 증감률(수식)
const H: u32 = 7; // 산출 근거(글자)

fn num(n: f64) -> CellValue {
    CellValue::Number(n)
}

fn text(s: &str) -> CellValue {
    CellValue::Text(s.into())
}

fn cell(row: u32, col: u32, value: CellValue, base: u32) -> CellEdit {
    CellEdit { row, col, value, base }
}

fn set(st: &mut State, c: &Client, batch: &str, cells: Vec<CellEdit>) {
    st.handle(Command::Client {
        conn: c.conn,
        msg: ClientMsg::SetCells { sheet: 0, batch: batch.into(), cells, origin: None },
    });
}

fn lock(st: &mut State, c: &Client, row: u32, col: u32) {
    st.handle(Command::Client { conn: c.conn, msg: ClientMsg::Lock { sheet: 0, at: CellRef::new(row, col) } });
}

/// 그 사람이 맡은 첫 데이터 행.
fn first_row(st: &State, name: &str) -> u32 {
    st.rt[0].access.rows_of(st.rt[0].access.person(name).unwrap())[0]
}

fn value(st: &State, row: u32, col: u32) -> CellValue {
    st.sheets[0].model.cell(CellRef::new(row, col)).unwrap().v.clone()
}

type AppliedMsg = (Vec<CellUpdate>, Vec<CellRef>, Vec<Rejection>);

fn applied(msgs: &[ServerMsg]) -> AppliedMsg {
    msgs.iter()
        .find_map(|m| match m {
            ServerMsg::Applied { cells, conflicts, rejected, .. } => {
                Some((cells.clone(), conflicts.clone(), rejected.clone()))
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("applied가 와야 함: {msgs:?}"))
}

fn cells_from(msgs: &[ServerMsg]) -> Vec<(String, Vec<CellUpdate>)> {
    msgs.iter()
        .filter_map(|m| match m {
            ServerMsg::Cells { by, cells, .. } => Some((by.clone(), cells.clone())),
            _ => None,
        })
        .collect()
}

fn lock_changes(msgs: &[ServerMsg]) -> Vec<(CellRef, Option<String>)> {
    msgs.iter()
        .filter_map(|m| match m {
            ServerMsg::LockChanged { at, by, .. } => Some((*at, by.clone())),
            _ => None,
        })
        .collect()
}

fn lock_result(msgs: &[ServerMsg]) -> (bool, Option<String>, Option<String>) {
    msgs.iter()
        .find_map(|m| match m {
            ServerMsg::LockResult { ok, by, reason, .. } => Some((*ok, by.clone(), reason.clone())),
            _ => None,
        })
        .unwrap_or_else(|| panic!("lockResult가 와야 함: {msgs:?}"))
}

fn errors(msgs: &[ServerMsg]) -> Vec<String> {
    msgs.iter()
        .filter_map(|m| match m {
            ServerMsg::Error { message } => Some(message.clone()),
            _ => None,
        })
        .collect()
}

fn setup(names: &[&str]) -> (State, Vec<Client>) {
    setup_with(fixture(), names)
}

fn setup_with(init: SessionInit, names: &[&str]) -> (State, Vec<Client>) {
    let mut st = State::new(init);
    let mut clients: Vec<Client> = Vec::new();
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

/// 김민지 행의 F열을 이서연에게도 범위 배정한다(두 사람이 같은 칸을 편집할 수 있게).
fn shared_cell_init() -> (SessionInit, u32) {
    let mut init = fixture();
    let access = Access::compile(&init.sheets[0].0, &init.sheets[0].1);
    let kim_row = access.rows_of(access.person("김민지").unwrap())[0];
    init.sheets[0].1.assignment.ranges.push(RangeAssignment {
        range: CellRange::new(kim_row, F, 1, 1),
        person: "이서연".into(),
        note: String::new(),
    });
    (init, kim_row)
}

#[tokio::test]
async fn set_cells_normalizes_applies_and_broadcasts() {
    let (mut st, mut c) = setup(&["이서연", "최도윤"]);
    let r = first_row(&st, "이서연");
    assert!(value(&st, r, F).is_empty());
    let before = st.snapshot();

    set(&mut st, &c[0], "b1", vec![cell(r, F, text("9,800"), 0)]);
    let mine = c[0].take();
    let (cells, conflicts, rejected) = applied(&mine);
    assert_eq!(cells, [CellUpdate { row: r, col: F, value: num(9800.0), ver: 1 }]);
    assert!(conflicts.is_empty() && rejected.is_empty());
    // 진행률 증분: 내 진행률이 오고 호스트 표도 바뀐다.
    let progress = mine.iter().find_map(|m| match m {
        ServerMsg::Progress { progress, .. } => Some(progress.clone()),
        _ => None,
    });
    let lee_before = before.participants.iter().find(|p| p.name == "이서연").unwrap().filled;
    assert_eq!(progress.map(|p| p.filled), Some(lee_before + 1));
    let after = st.snapshot();
    assert_eq!(after.filled, before.filled + 1);
    assert_eq!(st.rt[0].progress, st.rt[0].access.progress(&st.sheets[0].model), "증분 계산이 다시 센 값과 같아야 함");

    // 다른 사람에게는 누가 바꿨는지와 함께 간다.
    assert_eq!(cells_from(&c[1].take()), [("이서연".into(), cells)]);
    assert_eq!(st.log.len(), 1);
    assert_eq!((st.log[0].old.clone(), st.log[0].new.clone()), (CellValue::Empty, num(9800.0)));

    // 지우기도 같은 경로: 진행률이 다시 줄어든다.
    set(&mut st, &c[0], "b2", vec![cell(r, F, CellValue::Empty, 1)]);
    assert_eq!(applied(&c[0].take()).0[0].ver, 2);
    assert_eq!(st.snapshot().filled, before.filled);
}

#[tokio::test]
async fn rejects_other_rows_formulas_locked_columns_and_bad_values() {
    let (mut st, mut c) = setup(&["이서연", "최도윤"]);
    let r = first_row(&st, "이서연");
    let other = first_row(&st, "최도윤");
    set(
        &mut st,
        &c[0],
        "b1",
        vec![
            cell(other, F, num(1.0), 0),
            cell(r, G, num(1.0), 0),
            cell(r, 4, num(1.0), 0), // E 전년 실적(입력 금지)
            cell(r, F, text("구천팔백"), 0),
            cell(r, H, text("간담회 4회"), 0),
        ],
    );
    let (cells, _, rejected) = applied(&c[0].take());
    assert_eq!(cells.len(), 1);
    assert_eq!(rejected.len(), 4);
    assert!(rejected[3].reason.contains("숫자"), "{:?}", rejected[3]);
    assert_eq!(rejected[1].ver, 0);
    assert_eq!(value(&st, other, F), CellValue::Empty);
    // 거절된 칸은 다른 사람에게 가지 않는다.
    let seen = cells_from(&c[1].take());
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].1[0].col, H);
}

#[tokio::test]
async fn locks_block_others_until_commit() {
    let (init, k) = shared_cell_init();
    let (mut st, mut c) = setup_with(init, &["김민지", "이서연"]);
    let at = CellRef::new(k, F);

    lock(&mut st, &c[0], k, F);
    assert_eq!(lock_result(&c[0].take()), (true, None, None));
    assert_eq!(lock_changes(&c[1].take()), [(at, Some("김민지".into()))]);
    let kim = st.snapshot().participants.into_iter().find(|p| p.name == "김민지").unwrap();
    assert_eq!(kim.editing, Some(SheetCell::new(0, at)));

    lock(&mut st, &c[1], k, F);
    assert_eq!(lock_result(&c[1].take()), (false, Some("김민지".into()), None));
    set(&mut st, &c[1], "x", vec![cell(k, F, num(1.0), 0)]);
    let (_, _, rejected) = applied(&c[1].take());
    assert!(rejected[0].reason.contains("김민지 님이 입력 중"));

    // 확정하면 잠금이 풀린다.
    set(&mut st, &c[0], "y", vec![cell(k, F, num(3600.0), 0)]);
    let seen = c[1].take();
    assert_eq!(lock_changes(&seen), [(at, None)]);
    assert_eq!(cells_from(&seen)[0].1[0].value, num(3600.0));
    assert!(st.rt[0].locks.is_empty());

    // 남의 칸·입력 금지 칸은 잠글 수 없다.
    let own = first_row(&st, "이서연");
    lock(&mut st, &c[0], own, F);
    assert_eq!(lock_result(&c[0].take()).2.as_deref(), Some("입력할 수 없는 칸입니다"));
}

#[tokio::test]
async fn one_lock_per_person_and_unlock() {
    let (init, k) = shared_cell_init();
    let (mut st, mut c) = setup_with(init, &["김민지", "이서연"]);
    lock(&mut st, &c[0], k, F);
    lock(&mut st, &c[0], k, H); // 다른 칸으로 옮기면 앞 잠금은 풀린다
    assert_eq!(st.rt[0].locks.len(), 1);
    let (f, h) = (CellRef::new(k, F), CellRef::new(k, H));
    let kim = Some("김민지".to_string());
    assert_eq!(lock_changes(&c[1].take()), [(f, kim.clone()), (f, None), (h, kim)]);
    st.handle(Command::Client { conn: c[0].conn, msg: ClientMsg::Unlock { sheet: 0, at: CellRef::new(k, H) } });
    assert!(st.rt[0].locks.is_empty());
}

#[tokio::test]
async fn last_write_wins_and_reports_conflict() {
    let (init, k) = shared_cell_init();
    let (mut st, mut c) = setup_with(init, &["김민지", "이서연"]);
    set(&mut st, &c[0], "a", vec![cell(k, F, num(3500.0), 0)]);
    c[0].take();
    c[1].take();
    // 이서연은 김민지의 수정을 보기 전(base 0) 값으로 보냈다 → 덮어쓰고 충돌을 알린다.
    set(&mut st, &c[1], "b", vec![cell(k, F, num(4000.0), 0)]);
    let (cells, conflicts, _) = applied(&c[1].take());
    assert_eq!(conflicts, [CellRef::new(k, F)]);
    assert_eq!(cells[0], CellUpdate { row: k, col: F, value: num(4000.0), ver: 2 });
    assert_eq!(cells_from(&c[0].take())[0].1[0].value, num(4000.0));
    // 내가 연달아 바꾼 것(응답 전에 또 보냄)은 충돌이 아니다.
    set(&mut st, &c[1], "c", vec![cell(k, F, num(4100.0), 1)]);
    assert!(applied(&c[1].take()).1.is_empty());
}

#[tokio::test]
async fn resent_batch_is_applied_once() {
    let (mut st, mut c) = setup(&["이서연"]);
    let r = first_row(&st, "이서연");
    set(&mut st, &c[0], "offline-1", vec![cell(r, F, num(1.0), 0)]);
    c[0].take();
    set(&mut st, &c[0], "offline-1", vec![cell(r, F, num(2.0), 0)]);
    let (cells, conflicts, rejected) = applied(&c[0].take());
    assert_eq!(cells[0].value, num(1.0));
    assert!(conflicts.is_empty() && rejected.is_empty());
    assert_eq!(st.log.len(), 1);
}

#[tokio::test(start_paused = true)]
async fn orphaned_lock_survives_short_disconnect_then_expires() {
    let (init, k) = shared_cell_init();
    let (mut st, mut c) = setup_with(init, &["이서연"]);
    let mut kim = connect(&mut st, 10);
    join(&mut st, &kim, "김민지", None);
    let token = kim.welcome().token;
    lock(&mut st, &kim, k, F);
    c[0].take();

    st.handle(Command::Disconnect { conn: kim.conn });
    tokio::time::advance(Duration::from_secs(10)).await;
    st.handle(Command::Tick);
    assert_eq!(st.rt[0].locks.len(), 1, "유예 시간 안에는 남는다");
    // 다시 붙으면 그대로 이어서 편집한다.
    let mut again = connect(&mut st, 11);
    join(&mut st, &again, "김민지", Some(&token));
    again.welcome();
    tokio::time::advance(LOCK_GRACE + Duration::from_secs(1)).await;
    st.handle(Command::Tick);
    assert_eq!(st.rt[0].locks.len(), 1);

    st.handle(Command::Disconnect { conn: again.conn });
    tokio::time::advance(LOCK_GRACE + Duration::from_secs(1)).await;
    st.handle(Command::Tick);
    assert!(st.rt[0].locks.is_empty());
    assert_eq!(lock_changes(&c[0].take()), [(CellRef::new(k, F), None)]);
}

#[tokio::test(start_paused = true)]
async fn abandoned_editor_lock_is_released() {
    let (init, k) = shared_cell_init();
    let (mut st, mut c) = setup_with(init, &["김민지"]);
    lock(&mut st, &c[0], k, F);
    tokio::time::advance(LOCK_MAX + Duration::from_secs(1)).await;
    st.handle(Command::Tick);
    assert!(st.rt[0].locks.is_empty());
    assert_eq!(lock_changes(&c[0].take()), [(CellRef::new(k, F), None)]);
}

#[tokio::test]
async fn host_unlock_notifies_owner() {
    let (init, k) = shared_cell_init();
    let (mut st, mut c) = setup_with(init, &["김민지", "이서연"]);
    lock(&mut st, &c[0], k, F);
    c[0].take();
    c[1].take();
    let (tx, rx) = oneshot::channel();
    st.handle(Command::Unlock { name: "김민지".into(), reply: tx });
    assert_eq!(rx.await.unwrap(), Ok(()));
    assert_eq!(lock_changes(&c[0].take()), [(CellRef::new(k, F), None)]); // 편집기를 닫으라는 뜻
    assert_eq!(lock_changes(&c[1].take()), [(CellRef::new(k, F), None)]);
    assert!(st.host_unlock("모르는 사람").is_err());
}

#[tokio::test]
async fn deadline_makes_session_read_only() {
    let (mut st, mut c) = setup(&["이서연"]);
    let r = first_row(&st, "이서연");
    host(&mut st, |s| {
        s.sheets[0].config.deadline = Some("2000-01-01T09:00".into());
        s.changed();
    });
    assert!(st.read_only && st.snapshot().read_only);
    assert!(c[0].welcome().session.read_only);
    set(&mut st, &c[0], "late", vec![cell(r, F, num(1.0), 0)]);
    assert!(applied(&c[0].take()).2[0].reason.contains("마감"));
    lock(&mut st, &c[0], r, F);
    assert!(!lock_result(&c[0].take()).0);

    // '마감 후 자동 읽기 전용'을 끄면 다시 입력할 수 있다.
    host(&mut st, |s| {
        s.sheets[0].config.permissions.lock_after_deadline = false;
        s.changed();
    });
    assert!(!st.read_only);

    // 시간이 지나 마감되면 주기 점검이 알린다.
    host(&mut st, |s| {
        s.sheets[0].config.permissions.lock_after_deadline = true;
        s.sheets[0].config.deadline = Some("2999-01-01".into());
        s.changed();
    });
    c[0].take();
    assert!(!st.read_only);
    st.deadline = chrono::NaiveDate::from_ymd_opt(2000, 1, 1).and_then(|d| d.and_hms_opt(0, 0, 0));
    st.handle(Command::Tick);
    assert!(c[0].take().iter().any(|m| matches!(m, ServerMsg::ReadOnly { read_only: true, .. })));
}

#[test]
fn deadline_formats() {
    assert!(parse_deadline(Some("2026-10-10T18:00")).is_some());
    assert!(parse_deadline(Some("2026-10-10T18:00:30")).is_some());
    assert_eq!(parse_deadline(Some("2026-10-10")).unwrap().to_string(), "2026-10-10 23:59:59");
    assert!(parse_deadline(Some("내일")).is_none() && parse_deadline(None).is_none());
}

#[tokio::test]
async fn hidden_policy_keeps_other_rows_private() {
    let mut init = fixture();
    init.sheets[0].1.permissions.others_rows = OthersRows::Hidden;
    let (mut st, mut c) = setup_with(init, &["이서연", "최도윤"]);
    let r = first_row(&st, "이서연");
    lock(&mut st, &c[0], r, F);
    set(&mut st, &c[0], "b1", vec![cell(r, F, num(9800.0), 0)]);
    applied(&c[0].take());
    let seen = c[1].take();
    assert!(cells_from(&seen).is_empty() && lock_changes(&seen).is_empty(), "{seen:?}");
    st.handle(Command::Client { conn: c[1].conn, msg: ClientMsg::RowHistory { sheet: 0, row: r } });
    assert!(matches!(&c[1].take()[..], [ServerMsg::History { entries, .. }] if entries.is_empty()));
    // 다시 들어와도 남의 행 버전은 받지 않는다.
    let mut again = connect(&mut st, 9);
    join(&mut st, &again, "최도윤", None);
    assert!(again.take().iter().any(|m| matches!(m, ServerMsg::DupName { .. })));
    st.handle(Command::Client { conn: 9, msg: ClientMsg::SwitchDevice { name: "최도윤".into() } });
    assert!(again.welcome().sheets[0].versions.is_empty());
}

#[tokio::test]
async fn row_history_lists_changes_in_order() {
    let (mut st, mut c) = setup(&["이서연"]);
    let r = first_row(&st, "이서연");
    set(&mut st, &c[0], "b1", vec![cell(r, F, num(9800.0), 0)]);
    set(&mut st, &c[0], "b2", vec![cell(r, H, text("간담회 4회"), 0), cell(r, F, num(9900.0), 1)]);
    c[0].take();
    st.handle(Command::Client { conn: c[0].conn, msg: ClientMsg::RowHistory { sheet: 0, row: r } });
    let msgs = c[0].take();
    let [ServerMsg::History { row, entries, .. }] = &msgs[..] else { panic!("{msgs:?}") };
    assert_eq!(*row, r);
    let got: Vec<_> = entries.iter().map(|e| (e.batch.as_str(), e.col, e.new.clone())).collect();
    assert_eq!(got, [("b1", F, num(9800.0)), ("b2", H, text("간담회 4회")), ("b2", F, num(9900.0))]);
    assert_eq!(entries[2].old, num(9800.0));
    assert!(entries.iter().all(|e| e.user == "이서연" && e.at_ms > 0));
}

#[tokio::test]
async fn format_follows_policy() {
    let (mut st, mut c) = setup(&["이서연", "최도윤"]);
    let r = first_row(&st, "이서연");
    let mine = CellRange::new(r, F, 1, 1);
    let fill = StylePatch { fill: Some(FillPatch::Color { rgb: 0xFFF3B0 }), ..Default::default() };
    let bold = StylePatch { bold: Some(true), ..Default::default() };
    let format = |st: &mut State, c: &Client, range: CellRange, patch: &StylePatch| {
        st.handle(Command::Client { conn: c.conn, msg: ClientMsg::Format { sheet: 0, range, patch: patch.clone() } });
    };

    format(&mut st, &c[0], mine, &fill);
    assert!(errors(&c[0].take())[0].contains("서식을 바꿀 수 없습니다"));

    host(&mut st, |s| {
        s.sheets[0].config.permissions.format = FormatPolicy::FillOnly;
        s.changed();
    });
    c[0].take();
    c[1].take();
    format(&mut st, &c[0], mine, &bold);
    assert!(errors(&c[0].take())[0].contains("배경색만"));
    let other = CellRange::new(first_row(&st, "최도윤"), F, 1, 1);
    format(&mut st, &c[0], other, &fill);
    assert!(errors(&c[0].take())[0].contains("입력할 수 있는 칸만"));
    format(&mut st, &c[0], mine, &fill);
    let s = st.sheets[0].model.cell(CellRef::new(r, F)).unwrap().s;
    assert_eq!(st.sheets[0].model.styles[s as usize].fill, Some(0xFFF3B0));
    for client in &mut c {
        let msgs = client.take();
        let changes = msgs.iter().find_map(|m| match m {
            ServerMsg::Styles { changes, .. } => Some(changes.clone()),
            _ => None,
        });
        assert_eq!(changes, Some(vec![StyleChange { row: r, col: F, s }]));
    }

    host(&mut st, |s| {
        s.sheets[0].config.permissions.format = FormatPolicy::All;
        s.changed();
    });
    c[0].take();
    format(&mut st, &c[0], mine, &bold);
    assert!(errors(&c[0].take()).is_empty());
}

#[tokio::test]
async fn recalc_requests_and_formula_results() {
    let (mut st, mut c) = setup(&["이서연", "최도윤"]);
    let (tx, mut rx) = mpsc::unbounded_channel();
    st.handle(Command::SetRecalc(Some(tx)));
    let r = first_row(&st, "이서연");
    set(&mut st, &c[0], "b1", vec![cell(r, F, num(9800.0), 0)]);
    let req = rx.try_recv().expect("재계산 요청");
    assert_eq!(req.writes, [(0, CellRef::new(r, F), num(9800.0))]);
    c[0].take();
    c[1].take();

    let g = CellRef::new(r, G);
    st.handle(Command::Formulas(vec![(0, g, num(0.065)), (0, CellRef::new(r, H), text("수식 아님"))]));
    for client in &mut c {
        let seen = cells_from(&client.take());
        assert_eq!(seen, [(String::new(), vec![CellUpdate { row: r, col: G, value: num(0.065), ver: 1 }])]);
    }
    assert!(value(&st, r, H).is_empty(), "수식이 아닌 칸은 바꾸지 않는다");
    st.handle(Command::Formulas(vec![(0, g, num(0.065))]));
    assert!(cells_from(&c[0].take()).is_empty(), "같은 값이면 보내지 않는다");
    assert_eq!(st.log.len(), 1, "수식 결과는 이력에 남기지 않는다");
}

#[tokio::test]
async fn welcome_carries_versions_and_locks_and_replace_resets_them() {
    let (init, k) = shared_cell_init();
    let (mut st, c) = setup_with(init, &["김민지", "이서연"]);
    set(&mut st, &c[1], "b1", vec![cell(k, F, num(1.0), 0)]);
    lock(&mut st, &c[0], k, F);
    let mut d = connect(&mut st, 7);
    join(&mut st, &d, "최도윤", None);
    let w = d.welcome();
    let i = k * st.sheets[0].model.cols + F;
    assert_eq!(w.sheets[0].versions, [(i, 1)]);
    assert_eq!(w.sheets[0].locks, [LockEntry { at: CellRef::new(k, F), by: "김민지".into() }]);

    host(&mut st, |s| {
        let (model, config) = (s.sheets[0].model.clone(), s.sheets[0].config.clone());
        s.replace(vec![(model, config)]);
    });
    assert!(st.rt[0].locks.is_empty() && st.log.is_empty());
    let w = d.welcome();
    assert!(w.sheets[0].versions.is_empty() && w.sheets[0].locks.is_empty());
}

fn add_rows(st: &mut State, c: &Client, count: u32) {
    st.handle(Command::Client { conn: c.conn, msg: ClientMsg::AddRows { sheet: 0, count } });
}

#[tokio::test]
async fn participants_add_rows_when_allowed() {
    let mut st = State::new(fixture());
    let mut a = connect(&mut st, 1);
    let mut b = connect(&mut st, 2);
    join(&mut st, &a, "정하늘", None);
    join(&mut st, &b, "최도윤", None);
    a.take();
    b.take();
    let rows = st.sheets[0].model.rows;
    add_rows(&mut st, &a, 1);
    assert!(
        a.take()
            .iter()
            .any(|m| matches!(m, ServerMsg::Error { message } if message.contains("행을 추가할 수 없습니다")))
    );
    assert_eq!(st.sheets[0].model.rows, rows);

    host(&mut st, |s| {
        s.sheets[0].config.permissions.add_rows = true;
        s.changed();
    });
    a.take();
    b.take();
    let mut feed = st.host_feed.subscribe();
    add_rows(&mut st, &a, 2);
    assert_eq!(st.sheets[0].model.rows, rows + 2);
    assert_eq!(st.rt[0].versions.len(), st.sheets[0].model.cells.len());
    let (r0, key) = (rows, st.sheets[0].config.assignment.key_col.unwrap());
    // 행 배정 기준 열에 추가한 사람 이름 → 그 사람의 행. 수식(G)은 위 행에서 내려 쓴다.
    assert_eq!(value(&st, r0, key), text("정하늘"));
    assert_eq!(st.rt[0].access.row_owner(r0), st.rt[0].access.person("정하늘"));
    assert!(st.sheets[0].model.cell(CellRef::new(r0, G)).unwrap().f.is_some());
    assert_eq!(st.log.last().map(|e| e.origin), Some(BatchOrigin::AddRow));
    let msgs = a.take();
    let w = msgs.iter().find_map(|m| match m {
        ServerMsg::Welcome(w) => Some(w),
        _ => None,
    });
    assert_eq!(w.map(|w| w.sheets[0].model.rows), Some(rows + 2), "모두에게 새 시트");
    assert!(msgs.iter().any(
        |m| matches!(m, ServerMsg::RowsAdded { by, range, .. } if by == "정하늘" && range.row == r0 && range.rows == 2)
    ));
    assert!(b.take().iter().any(|m| matches!(m, ServerMsg::RowsAdded { .. })));
    assert!(matches!(feed.try_recv(), Ok(HostEvent::Reset)));

    // 새 행의 입력 칸: 추가한 사람만(행 배정 기준이 있으므로). 잠금 열(D 비목)도 새 행에서는 입력할 수 있고,
    // 수식(G)·행 배정 기준(C) 칸은 그대로 잠겨 있다.
    set(&mut st, &a, "n1", vec![cell(r0, F, text("1,200"), 0), cell(r0, 3, text("새 비목"), 0)]);
    let (ok, _, rejected) = applied(&a.take());
    assert!(rejected.is_empty(), "{rejected:?}");
    assert_eq!(ok[0].value, num(1200.0));
    assert_eq!(value(&st, r0, 3), text("새 비목"));
    set(&mut st, &a, "n1b", vec![cell(r0, key, text("최도윤"), 0), cell(r0, G, num(1.0), 0)]);
    assert_eq!(applied(&a.take()).2.len(), 2);
    assert!(
        !st.rt[0].access.can_edit(
            collect_core::access::Viewer::Person(st.rt[0].access.person("정하늘").unwrap()),
            CellRef::new(5, 3)
        )
    );
    set(&mut st, &b, "n2", vec![cell(r0, H, text("남의 새 행"), 0)]);
    assert_eq!(applied(&b.take()).2.len(), 1);
    // 예외는 한 벌(입력 가능 + 기준 칸 입력 금지)이고, 이어서 추가하면 범위가 늘어난다.
    let added: Vec<_> =
        st.sheets[0].config.exceptions.iter().filter(|e| e.note == super::edit::ADDED_ROWS_NOTE).collect();
    assert_eq!(added.len(), 2);
    add_rows(&mut st, &a, 1);
    let added: Vec<_> =
        st.sheets[0].config.exceptions.iter().filter(|e| e.note == super::edit::ADDED_ROWS_NOTE).collect();
    assert_eq!(added.len(), 2);
    assert_eq!(added[0].range, CellRange::new(r0, 0, 3, st.sheets[0].model.cols));
    a.take();
    b.take();

    // 마감되면 추가할 수 없다.
    host(&mut st, |s| s.set_closed(true));
    a.take();
    add_rows(&mut st, &a, 1);
    assert!(a.take().iter().any(|m| matches!(m, ServerMsg::Error { .. })));
    assert_eq!(st.sheets[0].model.rows, rows + 3);
}

#[tokio::test]
async fn added_rows_without_key_column_are_open_to_everyone_and_capped() {
    let mut init = fixture();
    init.sheets[0].1.assignment.key_col = None;
    init.sheets[0].1.assignment.extra_people = vec!["가".into(), "나".into()];
    init.sheets[0].1.permissions.add_rows = true;
    let mut st = State::new(init);
    let mut a = connect(&mut st, 1);
    let mut b = connect(&mut st, 2);
    join(&mut st, &a, "가", None);
    join(&mut st, &b, "나", None);
    a.take();
    add_rows(&mut st, &a, 500); // 한 번에 MAX_ADD_ROWS까지만
    let r0 = st.sheets[0].model.rows - super::edit::MAX_ADD_ROWS;
    assert_eq!(st.sheets[0].baseline.rows + super::edit::MAX_ADD_ROWS, st.sheets[0].model.rows);
    b.take();
    set(&mut st, &b, "x", vec![cell(r0, F, text("5"), 0)]);
    assert!(applied(&b.take()).2.is_empty(), "담당자 없는 새 행은 누구나");
    for _ in 0..(super::edit::MAX_ADDED_ROWS / super::edit::MAX_ADD_ROWS) {
        add_rows(&mut st, &a, super::edit::MAX_ADD_ROWS);
    }
    assert_eq!(st.sheets[0].model.rows - st.sheets[0].baseline.rows, super::edit::MAX_ADDED_ROWS);
    assert!(a.take().iter().any(|m| matches!(m, ServerMsg::Error { message } if message.contains("까지 추가"))));
}
