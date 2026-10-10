//! 세션 액터 단위 테스트: 네트워크 없이 연결 대신 채널을 꽂아 상태 전이를 검사한다.

use std::net::Ipv4Addr;

use base64::Engine;
use collect_core::access::flag;
use collect_core::config::{ColumnRole, suggested_key_col};

use super::*;

pub(super) fn fixture() -> SessionInit {
    let model: SheetModel = serde_json::from_slice(include_bytes!("../../../../fixtures/budget.sheet.json")).unwrap();
    let mut config = CollectConfig::suggest(&model);
    config.assignment.key_col = suggested_key_col(&model); // C 담당자
    config.requester = "박지훈".into();
    config.columns[5].required = true; // F 신청 금액
    SessionInit::new(model, config)
}

pub(super) struct Client {
    pub(super) conn: ConnId,
    rx: mpsc::UnboundedReceiver<Outgoing>,
    pub(super) closed: bool,
}

impl Client {
    pub(super) fn take(&mut self) -> Vec<ServerMsg> {
        let mut out = Vec::new();
        while let Ok(o) = self.rx.try_recv() {
            match o {
                Outgoing::Msg(m) => out.push(m),
                Outgoing::Close => self.closed = true,
            }
        }
        out
    }

    pub(super) fn welcome(&mut self) -> Welcome {
        self.take()
            .into_iter()
            .find_map(|m| match m {
                ServerMsg::Welcome(w) => Some(*w),
                _ => None,
            })
            .expect("welcome가 와야 함")
    }
}

pub(super) fn connect(st: &mut State, conn: ConnId) -> Client {
    let (outbox, rx) = mpsc::unbounded_channel();
    st.handle(Command::Connect { conn, ip: ip_of(conn), outbox });
    Client { conn, rx, closed: false }
}

pub(super) fn ip_of(conn: ConnId) -> IpAddr {
    IpAddr::V4(Ipv4Addr::new(192, 168, 0, conn as u8))
}

/// IP 명단에 이 연결의 IP를 그 이름으로 올린다(호스트가 JSON으로 관리하는 명단).
pub(super) fn list_ip(st: &mut State, conn: ConnId, name: &str) {
    let mut entries = st.ip_names.entries();
    entries.retain(|e| e.ip != ip_of(conn).to_string());
    entries.push(crate::ipnames::IpEntry { ip: ip_of(conn).to_string(), name: name.into() });
    st.ip_names = IpNames::from_entries(&entries).unwrap();
}

/// IP 명단에 있는 사람으로 들어온다(이 연결의 IP를 그 이름으로 올린 뒤 `join`).
pub(super) fn join(st: &mut State, c: &Client, name: &str, token: Option<&str>) {
    if !name.trim().is_empty() {
        list_ip(st, c.conn, name.trim());
    }
    join_unlisted(st, c, name, token);
}

/// IP 명단 밖에서 이름으로 참여한다(호스트 승인이 필요하다).
pub(super) fn join_unlisted(st: &mut State, c: &Client, name: &str, token: Option<&str>) {
    let msg = ClientMsg::Join { name: name.into(), token: token.map(String::from), protocol: PROTOCOL_VERSION };
    st.handle(Command::Client { conn: c.conn, msg });
}

/// 호스트가 세션 안에서 시트·설정을 고친다([`SessionHandle::with`]와 같은 경로).
pub(super) fn host(st: &mut State, f: impl FnOnce(&mut Shared<'_>) + Send + 'static) {
    let done = st.handle(Command::With(Box::new(move |s: &mut Shared<'_>| {
        f(s);
        Box::new(|| {}) as Completion
    })));
    if let Some(done) = done {
        done();
    }
}

/// 호스트 작업을 하고 결과를 돌려받는다.
pub(super) fn host_get<R: Send + 'static>(st: &mut State, f: impl FnOnce(&mut Shared<'_>) -> R + Send + 'static) -> R {
    let slot = std::sync::Arc::new(std::sync::Mutex::new(None));
    let out = slot.clone();
    host(st, move |s| *out.lock().unwrap() = Some(f(s)));
    slot.lock().unwrap().take().expect("작업 결과")
}

fn row<'a>(snap: &'a HostSnapshot, name: &str) -> &'a ParticipantRow {
    snap.participants.iter().find(|p| p.name == name).unwrap()
}

#[tokio::test]
async fn roster_name_gets_projected_welcome() {
    let mut st = State::new(fixture());
    let mut a = connect(&mut st, 1);
    join(&mut st, &a, " 김민지 ", None);
    let w = a.welcome();
    assert_eq!((w.name.as_str(), w.color), ("김민지", 2));
    assert!(!w.token.is_empty());
    assert_eq!(w.session.requester, "박지훈");
    assert_eq!(w.roster.len(), 6);
    assert_eq!(w.sheets[0].model.rows, 1002);
    assert!(w.sheets[0].progress.required > 0 && w.sheets[0].progress.rows > 0);
    let presence: Vec<_> = w.presence.iter().map(|p| (p.name.as_str(), p.host)).collect();
    assert_eq!(presence, [("박지훈", true), ("김민지", false)]);
    // 셀 상태 비트: 김민지 행의 F열은 MINE, 이서연 행(3행)은 아님
    let flags = base64::engine::general_purpose::STANDARD.decode(&w.sheets[0].flags).unwrap();
    let mine = |r: u32, c: u32| flags[(r * w.sheets[0].model.cols + c) as usize] & flag::MINE != 0;
    let kim_row = w.sheets[0].row_owner.iter().position(|&o| o == 2).unwrap() as u32;
    assert!(mine(kim_row, 5) && !mine(2, 5));
    let snap = st.snapshot();
    assert_eq!((snap.online, snap.roster), (1, 6));
    assert_eq!(row(&snap, "김민지").status, ParticipantStatus::Online);
    assert!(row(&snap, "김민지").assignment.starts_with("행 "));
    assert_eq!(row(&snap, "이서연").status, ParticipantStatus::Offline);
}

#[tokio::test]
async fn unknown_name_waits_for_host_then_gets_range() {
    let mut st = State::new(fixture());
    let mut a = connect(&mut st, 1);
    join_unlisted(&mut st, &a, "김민수", None);
    assert!(matches!(a.take()[..], [ServerMsg::Pending { .. }]));
    assert_eq!(st.snapshot().pending, [PendingRow { name: "김민수".into(), ip: "192.168.0.1".into() }]);

    let range = CellRange::parse("H500:H620").unwrap();
    st.approve("김민수", Some((0, range)), "추가 배정".into()).unwrap();
    assert_eq!(st.sheets[0].config.assignment.extra_people, ["김민수"]);
    assert_eq!(st.sheets[0].config.assignment.ranges[0].range, range);
    let w = a.welcome();
    assert_eq!(w.name, "김민수");
    assert!(st.snapshot().pending.is_empty());
    assert_eq!(row(&st.snapshot(), "김민수").assignment, "H500:H620");
}

#[tokio::test]
async fn host_can_reject_or_kick() {
    let mut st = State::new(fixture());
    let mut a = connect(&mut st, 1);
    join_unlisted(&mut st, &a, "모르는사람", None);
    a.take();
    st.reject("모르는사람").unwrap();
    assert!(matches!(a.take()[..], [ServerMsg::Rejected { .. }]) && a.closed);
    assert!(st.reject("모르는사람").is_err());

    let mut b = connect(&mut st, 2);
    join(&mut st, &b, "김민지", None);
    b.take();
    st.kick("김민지").unwrap();
    assert!(matches!(b.take()[..], [ServerMsg::Kicked { reason: KickReason::Host }, ..]) && b.closed);
    st.handle(Command::Disconnect { conn: 2 });
    assert_eq!(row(&st.snapshot(), "김민지").status, ParticipantStatus::Offline);
}

#[tokio::test]
async fn duplicate_name_then_switch_device() {
    let mut st = State::new(fixture());
    let mut a = connect(&mut st, 1);
    join(&mut st, &a, "이서연", None);
    a.welcome();
    let mut b = connect(&mut st, 2);
    join(&mut st, &b, "이서연", None);
    assert!(matches!(b.take()[..], [ServerMsg::DupName { .. }]));
    st.handle(Command::Client { conn: 2, msg: ClientMsg::SwitchDevice { name: "이서연".into() } });
    assert!(a.take().iter().any(|m| matches!(m, ServerMsg::Kicked { reason: KickReason::SwitchedDevice })));
    assert!(a.closed);
    b.welcome();
    // 끊긴 옛 연결의 종료 알림이 늦게 와도 새 연결은 그대로 접속 중이다.
    st.handle(Command::Disconnect { conn: 1 });
    let snap = st.snapshot();
    assert_eq!(row(&snap, "이서연").status, ParticipantStatus::Online);
    assert_eq!(row(&snap, "이서연").ip.as_deref(), Some("192.168.0.2"));
}

#[tokio::test]
async fn token_reconnect_skips_duplicate_prompt() {
    let mut st = State::new(fixture());
    let mut a = connect(&mut st, 1);
    join(&mut st, &a, "최도윤", None);
    let token = a.welcome().token;
    // 같은 브라우저가 새로 고침: 옛 연결이 아직 열려 있어도 토큰이 맞으면 바로 넘어온다.
    let mut b = connect(&mut st, 2);
    join(&mut st, &b, "최도윤", Some(&token));
    assert_eq!(b.welcome().token, token);
    a.take();
    assert!(a.closed);
    // 틀린 토큰이면 중복 확인을 거친다.
    let mut c = connect(&mut st, 3);
    join(&mut st, &c, "최도윤", Some("wrong"));
    assert!(matches!(c.take()[..], [ServerMsg::DupName { .. }]));
}

#[tokio::test]
async fn cursor_is_shared_and_shown_to_host() {
    let mut st = State::new(fixture());
    let mut a = connect(&mut st, 1);
    let mut b = connect(&mut st, 2);
    join(&mut st, &a, "이서연", None);
    join(&mut st, &b, "최도윤", None);
    a.take();
    b.take();
    let at = CellRef::new(4, 5);
    st.handle(Command::Client { conn: 1, msg: ClientMsg::Cursor { sheet: 0, at: Some(at) } });
    assert!(a.take().is_empty()); // 자기 커서는 돌려받지 않는다
    assert!(matches!(&b.take()[..], [ServerMsg::Cursor { name, at: Some(x), .. }] if name == "이서연" && *x == at));
    assert_eq!(row(&st.snapshot(), "이서연").cursor, Some(SheetCell::new(0, at)));
    // 시트 밖 좌표는 버린다.
    st.handle(Command::Client { conn: 1, msg: ClientMsg::Cursor { sheet: 0, at: Some(CellRef::new(5000, 0)) } });
    assert_eq!(row(&st.snapshot(), "이서연").cursor, None);
}

#[tokio::test(start_paused = true)]
async fn idle_participant_becomes_away_then_back() {
    let mut st = State::new(fixture());
    let mut a = connect(&mut st, 1);
    let mut b = connect(&mut st, 2);
    join(&mut st, &a, "이서연", None);
    join(&mut st, &b, "최도윤", None);
    a.take();
    b.take();
    tokio::time::advance(AWAY_AFTER + Duration::from_secs(1)).await;
    st.handle(Command::Client { conn: 2, msg: ClientMsg::Ping });
    st.handle(Command::Tick);
    assert_eq!(row(&st.snapshot(), "이서연").status, ParticipantStatus::Away);
    assert_eq!(row(&st.snapshot(), "이서연").idle_mins, 3);
    let seen = b.take();
    let away = seen.iter().any(|m| {
        matches!(m, ServerMsg::Presence { people }
            if people.iter().any(|p| p.name == "이서연" && p.status == PresenceStatus::Away))
    });
    assert!(away, "{seen:?}");
    st.handle(Command::Client { conn: 1, msg: ClientMsg::Cursor { sheet: 0, at: None } });
    assert_eq!(row(&st.snapshot(), "이서연").status, ParticipantStatus::Online);
}

#[tokio::test]
async fn host_update_resends_sheet_or_removes() {
    let mut st = State::new(fixture());
    let mut a = connect(&mut st, 1);
    join(&mut st, &a, "김민지", None);
    let before = a.welcome().sheets[0].progress.required;

    host(&mut st, |s| {
        s.sheets[0].config.columns[7].required = true; // H 산출 근거도 필수
        s.sheets[0].config.columns[7].role = ColumnRole::Input;
        s.changed();
    });
    assert!(a.welcome().sheets[0].progress.required > before);

    host(&mut st, |s| {
        s.sheets[0].config.assignment.key_col = None; // 행 배정을 없애면 명단에서 빠진다
        s.changed();
    });
    assert!(a.take().iter().any(|m| matches!(m, ServerMsg::Kicked { reason: KickReason::Removed })));
    assert!(a.closed);
}

#[tokio::test]
async fn bad_names_and_old_clients_are_rejected() {
    let mut st = State::new(fixture());
    let mut a = connect(&mut st, 1);
    join(&mut st, &a, "   ", None);
    assert!(matches!(a.take()[..], [ServerMsg::Rejected { .. }]) && !a.closed);
    st.handle(Command::Client { conn: 1, msg: ClientMsg::Join { name: "김민지".into(), token: None, protocol: 0 } });
    assert!(matches!(a.take()[..], [ServerMsg::Rejected { .. }]) && a.closed);
}

#[tokio::test]
async fn hidden_policy_strips_other_rows_in_welcome() {
    let mut init = fixture();
    init.sheets[0].1.permissions.others_rows = OthersRows::Hidden;
    let mut st = State::new(init);
    let mut a = connect(&mut st, 1);
    join(&mut st, &a, "김민지", None);
    let w = a.welcome();
    assert!(!w.sheets[0].model.hidden_rows.is_empty());
    let other = w.sheets[0].model.hidden_rows[0];
    assert!(w.sheets[0].model.cells[(other * w.sheets[0].model.cols + 3) as usize].v.is_empty()); // 남의 행 비목
    assert!(w.sheets[0].row_owner.iter().all(|&o| o == -1 || o == 2));
}

#[test]
fn summary_compresses_runs() {
    let init = fixture();
    let access = Access::compile(&init.sheets[0].0, &init.sheets[0].1);
    let lee = access.person("이서연").unwrap();
    let s = assignment_summary(&init.sheets[0].0, &init.sheets[0].1, &access, lee);
    assert!(s.starts_with("행 3–7 외 "), "{s}");
}

#[tokio::test]
async fn unlisted_ip_needs_approval_even_for_roster_names_then_token_reenters() {
    let mut st = State::new(fixture());
    let mut a = connect(&mut st, 1);
    join_unlisted(&mut st, &a, "김민지", None);
    assert!(matches!(a.take()[..], [ServerMsg::Pending { .. }]));
    // 명단에 그 이름이 있어도(설정이 바뀌어도) 승인 없이는 들어오지 않는다.
    host(&mut st, |s| s.changed());
    assert!(!a.take().iter().any(|m| matches!(m, ServerMsg::Welcome(_))));
    st.approve("김민지", None, String::new()).unwrap();
    let token = a.welcome().token;
    assert_eq!(st.sheets[0].config.assignment.extra_people, Vec::<String>::new(), "이미 명단에 있는 이름");

    // 같은 브라우저(토큰)는 다시 묻지 않는다. 다른 기기는 다시 승인을 기다리고, 승인하면 앞 기기는 끊긴다.
    st.handle(Command::Disconnect { conn: 1 });
    let mut b = connect(&mut st, 2);
    join_unlisted(&mut st, &b, "김민지", Some(&token));
    assert_eq!(b.welcome().name, "김민지");
    let mut c = connect(&mut st, 3);
    join_unlisted(&mut st, &c, "김민지", None);
    assert!(matches!(c.take()[..], [ServerMsg::Pending { .. }]), "중복 확인 없이 승인 대기");
    st.handle(Command::Client { conn: 3, msg: ClientMsg::SwitchDevice { name: "김민지".into() } });
    assert!(c.take().is_empty(), "IP 명단 밖에서는 기기 전환을 할 수 없다");
    st.approve("김민지", None, String::new()).unwrap();
    c.welcome();
    assert!(b.take().iter().any(|m| matches!(m, ServerMsg::Kicked { reason: KickReason::SwitchedDevice })));
}

#[tokio::test]
async fn listed_ip_enters_with_its_name_and_joins_the_roster() {
    let mut st = State::new(fixture());
    let mut lee = connect(&mut st, 9);
    join(&mut st, &lee, "이서연", None);
    lee.welcome();
    // 연결하자마자 IP 명단의 이름을 알려 주고, 보낸 이름과 상관없이 그 이름으로 들어온다.
    list_ip(&mut st, 1, "한도윤");
    let mut a = connect(&mut st, 1);
    assert!(matches!(&a.take()[..], [ServerMsg::Known { name }] if name == "한도윤"));
    join_unlisted(&mut st, &a, "아무개", None);
    let w = a.welcome();
    assert_eq!(w.name, "한도윤");
    assert_eq!(st.sheets[0].config.assignment.extra_people, ["한도윤"], "명단에 없던 이름은 명단에 더한다");
    assert!(lee.take().iter().any(|m| matches!(m, ServerMsg::Welcome(w) if w.roster.contains(&"한도윤".into()))));
    assert!(st.snapshot().pending.is_empty());
}

#[tokio::test]
async fn new_ip_list_admits_waiting_connections() {
    let mut st = State::new(fixture());
    let mut a = connect(&mut st, 1);
    join_unlisted(&mut st, &a, "최도윤", None);
    a.take();
    let mut b = connect(&mut st, 2); // 이름 입력 화면
    let names = IpNames::parse_json(r#"{"192.168.0.1": "최도윤", "192.168.0.2": "정하늘"}"#).unwrap();
    st.handle(Command::SetIpNames(names));
    assert_eq!(a.welcome().name, "최도윤");
    assert!(st.snapshot().pending.is_empty());
    assert!(matches!(&b.take()[..], [ServerMsg::Known { name }] if name == "정하늘"));
}

#[tokio::test]
async fn host_is_shown_by_the_name_of_its_ip() {
    let mut st = State::new(fixture());
    st.host_ips = vec![IpAddr::V4(Ipv4Addr::new(10, 0, 0, 5)), IpAddr::V4(Ipv4Addr::LOCALHOST)];
    assert_eq!(st.snapshot().host_name, "박지훈", "명단에 없으면 요청자");
    st.handle(Command::SetIpNames(IpNames::parse_json(r#"{"10.0.0.5": "정호스트"}"#).unwrap()));
    assert_eq!(st.snapshot().host_name, "정호스트");
    let mut a = connect(&mut st, 1);
    join(&mut st, &a, "김민지", None);
    let w = a.welcome();
    assert_eq!(w.presence[0].name, "정호스트");
    assert!(w.presence[0].host);
    assert_eq!(host_get(&mut st, |s| s.host_name().to_string()), "정호스트");
    host(&mut st, |s| s.chat(ChatTo::All, "안녕하세요"));
    assert!(a.take().iter().any(|m| matches!(m, ServerMsg::Chat(c) if c.from == "정호스트" && c.host)));
}
