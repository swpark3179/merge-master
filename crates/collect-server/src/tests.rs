//! 실제 소켓으로 확인하는 통합 테스트(라우트, 입장·승인·기기 전환, 정상 종료).

use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

use collect_core::SheetModel;
use collect_core::config::{CollectConfig, suggested_key_col};
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::Message;

use super::*;
use crate::protocol::{KickReason, PROTOCOL_VERSION};

fn config(dev_admin: bool) -> ServerConfig {
    ServerConfig { bind: IpAddr::V4(Ipv4Addr::LOCALHOST), port: 0, dev_admin }
}

fn init() -> SessionHandle {
    init_titled("2026년 부서별 예산 신청")
}

fn init_titled(title: &str) -> SessionHandle {
    let model: SheetModel = serde_json::from_slice(include_bytes!("../../../fixtures/budget.sheet.json")).unwrap();
    let mut config = CollectConfig::suggest(&model);
    config.assignment.key_col = suggested_key_col(&model); // C 담당자
    config.title = title.into();
    config.requester = "박지훈".into();
    SessionHandle::spawn(SessionInit::new(model, config))
}

/// 세션 하나를 `q4`로 공개한 서버.
async fn serve(session: SessionHandle, dev_admin: bool) -> ServerHandle {
    let hub = Hub::new();
    hub.publish("q4", session).unwrap();
    start(config(dev_admin), hub).await.unwrap()
}

async fn http(addr: SocketAddr, method: &str, path: &str, body: &str) -> String {
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: x\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    s.write_all(req.as_bytes()).await.unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).await.unwrap();
    out
}

type Client = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn open(addr: SocketAddr) -> Client {
    tokio_tungstenite::connect_async(format!("ws://{addr}/ws")).await.unwrap().0
}

/// 그 업무(주소 경로)의 WebSocket.
async fn open_at(addr: SocketAddr, slug: &str) -> Client {
    tokio_tungstenite::connect_async(format!("ws://{addr}/ws?s={slug}")).await.unwrap().0
}

async fn next(ws: &mut Client) -> ServerMsg {
    loop {
        let msg =
            tokio::time::timeout(Duration::from_secs(5), ws.next()).await.expect("응답 시간 초과").unwrap().unwrap();
        if let Message::Text(t) = msg {
            return serde_json::from_str(&t).unwrap();
        }
    }
}

/// `pred`에 맞는 메시지가 올 때까지 읽는다(프레즌스 같은 다른 메시지는 건너뜀).
async fn until(ws: &mut Client, pred: impl Fn(&ServerMsg) -> bool) -> ServerMsg {
    loop {
        let m = next(ws).await;
        if pred(&m) {
            return m;
        }
    }
}

async fn send(ws: &mut Client, msg: &ClientMsg) {
    ws.send(Message::Text(serde_json::to_string(msg).unwrap().into())).await.unwrap();
}

fn join(name: &str) -> ClientMsg {
    ClientMsg::Join { name: name.into(), token: None, protocol: PROTOCOL_VERSION }
}

/// IP 명단 밖(시험은 모두 127.0.0.1)에서 이름으로 참여 → 승인 대기 → 호스트 승인 → 입장.
async fn enter(session: &SessionHandle, ws: &mut Client, name: &str) -> Box<protocol::Welcome> {
    send(ws, &join(name)).await;
    assert!(matches!(until(ws, |m| matches!(m, ServerMsg::Pending { .. })).await, ServerMsg::Pending { .. }));
    session.approve(name, None, "").await.unwrap();
    let ServerMsg::Welcome(w) = until(ws, |m| matches!(m, ServerMsg::Welcome(_))).await else { unreachable!() };
    w
}

#[tokio::test]
async fn http_routes() {
    let server = serve(init(), false).await;
    let addr = server.local_addr;
    assert!(http(addr, "GET", "/api/health", "").await.ends_with("ok"));
    let info = http(addr, "GET", "/api/session", "").await;
    assert!(info.contains(r#""title":"2026년 부서별 예산 신청""#) && info.contains(r#""requester":"박지훈""#));
    // 시트는 입장한 뒤 WebSocket으로만, 그 사람 기준으로 보낸다.
    assert!(http(addr, "GET", "/api/sheet", "").await.starts_with("HTTP/1.1 404"));
    assert!(http(addr, "GET", "/__dev/snapshot", "").await.starts_with("HTTP/1.1 404"));
    // 맨 위 주소는 업무 목록 화면(예전처럼 한 업무로 보내지 않는다), 공개 중이 아닌 주소는 404.
    let root = http(addr, "GET", "/", "").await;
    assert!(!root.contains("location:"), "{root}");
    assert!(http(addr, "GET", "/wrong", "").await.starts_with("HTTP/1.1 404"));
    assert!(http(addr, "GET", "/api/session?s=wrong", "").await.starts_with("HTTP/1.1 404"));
    let list = http(addr, "GET", "/api/sessions", "").await;
    assert!(list.contains(r#""slug":"q4""#) && list.contains(r#""title":"2026년 부서별 예산 신청""#), "{list}");
    assert!(http(addr, "GET", "/assets/../../Cargo.toml", "").await.starts_with("HTTP/1.1 404"));
    server.stop().await;
}

#[tokio::test]
async fn join_pending_approve_and_switch_over_websocket() {
    let session = init();
    let server = serve(session.clone(), true).await;
    let addr = server.local_addr;

    // IP 명단 밖에서는 명단에 있는 이름이어도 승인을 기다린다.
    let mut a = open(addr).await;
    send(&mut a, &join("김민지")).await;
    assert!(matches!(next(&mut a).await, ServerMsg::Pending { .. }));
    let ok = http(addr, "POST", "/__dev/approve", r#"{"name":"김민지"}"#).await;
    assert!(ok.starts_with("HTTP/1.1 200"), "{ok}");
    let ServerMsg::Welcome(w) = until(&mut a, |m| matches!(m, ServerMsg::Welcome(_))).await else { unreachable!() };
    assert_eq!(w.session.title, "2026년 부서별 예산 신청");

    // 명단 밖 이름 → 승인 대기 → (개발용 관리 API로) 범위를 주고 승인
    let mut b = open(addr).await;
    send(&mut b, &join("김민수")).await;
    assert!(matches!(next(&mut b).await, ServerMsg::Pending { .. }));
    let snap = http(addr, "GET", "/__dev/snapshot", "").await;
    assert!(snap.contains(r#""pending":[{"name":"김민수""#));
    let ok = http(addr, "POST", "/__dev/approve", r#"{"name":"김민수","range":"H500:H620"}"#).await;
    assert!(ok.starts_with("HTTP/1.1 200"), "{ok}");
    assert!(matches!(until(&mut b, |m| matches!(m, ServerMsg::Welcome(_))).await, ServerMsg::Welcome(_)));

    // 이 PC(127.0.0.1)를 IP 명단에 김민지로 올리면: 새 연결은 이름을 묻지 않고(`known`) 들어오려 하고,
    // 김민지가 이미 접속 중이라 중복 확인 → 전환 → 앞 기기는 끊긴다.
    let names = IpNames::parse_json(r#"{"127.0.0.1": "김민지"}"#).unwrap();
    session.set_ip_names(names);
    let mut c = open(addr).await;
    assert!(matches!(next(&mut c).await, ServerMsg::Known { name } if name == "김민지"));
    send(&mut c, &join("")).await;
    assert!(matches!(next(&mut c).await, ServerMsg::DupName { .. }));
    send(&mut c, &ClientMsg::SwitchDevice { name: "김민지".into() }).await;
    assert!(matches!(until(&mut c, |m| matches!(m, ServerMsg::Welcome(_))).await, ServerMsg::Welcome(_)));
    let kicked = until(&mut a, |m| matches!(m, ServerMsg::Kicked { .. })).await;
    assert!(matches!(kicked, ServerMsg::Kicked { reason: KickReason::SwitchedDevice }));

    // 커서는 다른 참여자에게 전달된다.
    send(&mut c, &ClientMsg::Cursor { sheet: 0, at: Some(collect_core::CellRef::new(10, 5)) }).await;
    let cur = until(&mut b, |m| matches!(m, ServerMsg::Cursor { .. })).await;
    assert!(matches!(cur, ServerMsg::Cursor { name, .. } if name == "김민지"));

    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(session.snapshot().online, 2);
    tokio::time::timeout(Duration::from_secs(5), server.stop()).await.expect("stop()이 끝나지 않음");
    let closing = until(&mut b, |m| matches!(m, ServerMsg::Closing { .. })).await;
    assert!(matches!(closing, ServerMsg::Closing { ended: false, .. }));
}

#[tokio::test]
async fn host_events_follow_participants() {
    let session = init();
    let server = serve(session.clone(), false).await;
    let mut events = session.subscribe();
    let mut a = open(server.local_addr).await;
    enter(&session, &mut a, "이서연").await;
    tokio::time::timeout(Duration::from_secs(5), events.wait_for(|s| s.online == 1)).await.unwrap().unwrap();
    drop(a);
    tokio::time::timeout(Duration::from_secs(5), events.wait_for(|s| s.online == 0)).await.unwrap().unwrap();
    server.stop().await;
}

#[tokio::test]
async fn bind_conflict_is_reported() {
    let first = serve(init(), false).await;
    let mut cfg = config(false);
    cfg.port = first.local_addr.port();
    assert!(matches!(start(cfg, Hub::new()).await, Err(ServerError::Bind { .. })));
    first.stop().await;
}

/// 세션은 서버보다 오래 산다: 서비스를 멈췄다 다시 열어도 입력한 값이 그대로다.
#[tokio::test]
async fn edits_flow_over_websocket_and_survive_restart() {
    use crate::protocol::CellEdit;
    use collect_core::CellValue;

    let session = init();
    let server = serve(session.clone(), true).await;
    let addr = server.local_addr;
    let mut a = open(addr).await;
    let mut b = open(addr).await;
    let w = enter(&session, &mut a, "이서연").await;
    let tb = enter(&session, &mut b, "최도윤").await.token;
    let row = w.sheets[0].row_owner.iter().position(|&o| o == 0).unwrap() as u32;

    let cells = vec![CellEdit { row, col: 5, value: CellValue::Text("9,800".into()), base: 0 }];
    send(&mut a, &ClientMsg::SetCells { sheet: 0, batch: "b1".into(), cells, origin: None, layout: None }).await;
    let ok = until(&mut a, |m| matches!(m, ServerMsg::Applied { .. })).await;
    assert!(matches!(ok, ServerMsg::Applied { cells, .. } if cells[0].value == CellValue::Number(9800.0)));
    let seen = until(&mut b, |m| matches!(m, ServerMsg::Cells { .. })).await;
    assert!(matches!(seen, ServerMsg::Cells { by, .. } if by == "이서연"));

    // 개발용 관리 API: 수식 결과 밀어 넣기, 마감 지나기.
    let g = format!("G{}", row + 1);
    let r = http(addr, "POST", "/__dev/formulas", &format!(r#"[["{g}", 0.065]]"#)).await;
    assert!(r.starts_with("HTTP/1.1 200"), "{r}");
    let f = until(&mut b, |m| matches!(m, ServerMsg::Cells { by, .. } if by.is_empty())).await;
    assert!(matches!(f, ServerMsg::Cells { cells, .. } if cells[0].col == 6));
    let r = http(addr, "POST", "/__dev/config", r#"{"deadline":"2000-01-01T09:00"}"#).await;
    assert!(r.starts_with("HTTP/1.1 200"), "{r}");
    let ServerMsg::Welcome(w) = until(&mut a, |m| matches!(m, ServerMsg::Welcome(_))).await else { unreachable!() };
    assert!(w.session.read_only);

    server.stop().await;
    let again = serve(session.clone(), false).await;
    // 승인받은 브라우저는 토큰으로 다시 묻지 않고 들어온다.
    let mut c = open(again.local_addr).await;
    send(&mut c, &ClientMsg::Join { name: "최도윤".into(), token: Some(tb), protocol: PROTOCOL_VERSION }).await;
    let ServerMsg::Welcome(w) = until(&mut c, |m| matches!(m, ServerMsg::Welcome(_))).await else { unreachable!() };
    assert_eq!(w.sheets[0].model.cells[(row * w.sheets[0].model.cols + 5) as usize].v, CellValue::Number(9800.0));
    let history = session.with(|s| s.history().len()).await.unwrap();
    assert_eq!(history, 1);
    again.stop().await;
}

/// 호스트 작업의 결과를 받은 직후에도 접속자 표가 바뀐 내용이어야 한다(예전에는 답이 먼저 와서 화면이 옛 명단을 보였다).
#[tokio::test]
async fn snapshot_is_published_before_host_job_returns() {
    let session = init();
    assert_eq!(session.snapshot().roster, 6);
    for round in 0..20 {
        session
            .with(move |s| {
                s.sheets[0].config.assignment.key_col = if round % 2 == 0 { None } else { Some(2) };
                s.sheets[0].config.assignment.extra_people = vec!["한 명".into()];
                s.changed();
            })
            .await
            .unwrap();
        let want = if round % 2 == 0 { 1 } else { 7 };
        assert_eq!(session.snapshot().roster, want, "{round}번째");
    }
}

#[tokio::test]
async fn security_headers_and_foreign_origins() {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;

    let server = serve(init(), false).await;
    let addr = server.local_addr;
    let page = http(addr, "GET", "/q4", "").await.to_lowercase();
    assert!(page.contains("x-content-type-options: nosniff"), "{page}");
    assert!(page.contains("x-frame-options: deny"));
    assert!(page.contains("content-security-policy: default-src 'self'"));
    assert!(http(addr, "GET", "/api/session", "").await.to_lowercase().contains("referrer-policy: no-referrer"));

    // 다른 사이트에서 열린 페이지의 WebSocket은 거절한다.
    let mut req = format!("ws://{addr}/ws").into_client_request().unwrap();
    req.headers_mut().insert("origin", "http://evil.example".parse().unwrap());
    assert!(tokio_tungstenite::connect_async(req).await.is_err());
    // 같은 주소에서 열린 페이지는 된다.
    let mut req = format!("ws://{addr}/ws").into_client_request().unwrap();
    req.headers_mut().insert("origin", format!("http://{addr}").parse().unwrap());
    assert!(tokio_tungstenite::connect_async(req).await.is_ok());
    server.stop().await;
}

#[tokio::test]
async fn flooding_connection_is_warned_then_closed() {
    let server = serve(init(), false).await;
    let mut ws = open(server.local_addr).await;
    for _ in 0..400 {
        send(&mut ws, &ClientMsg::Ping).await;
    }
    let warned = until(&mut ws, |m| matches!(m, ServerMsg::Error { .. })).await;
    assert!(matches!(warned, ServerMsg::Error { message } if message.contains("너무 빨리")));
    // 계속 쏟아부으면 끊는다(보내다 실패할 수도 있다).
    for _ in 0..800 {
        if ws.send(Message::Text(serde_json::to_string(&ClientMsg::Ping).unwrap().into())).await.is_err() {
            break;
        }
    }
    let closed = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match ws.next().await {
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => return true,
                Some(Ok(_)) => {}
            }
        }
    })
    .await;
    assert_eq!(closed, Ok(true));
    server.stop().await;
}

#[tokio::test]
async fn stopping_the_service_closes_the_port() {
    let server = serve(init(), false).await;
    let addr = server.local_addr;
    assert!(http(addr, "GET", "/api/health", "").await.ends_with("ok"));
    server.stop().await;
    assert!(tokio::net::TcpStream::connect(addr).await.is_err(), "서비스를 멈추면 포트도 닫힌다");
}

/// 포트 하나에 업무 둘: 주소 경로로 가려 붙고, 한 업무만 공개를 멈추면 그 업무의 연결만 닫힌다.
#[tokio::test]
async fn two_sessions_on_one_port() {
    let (a, b) = (init(), init_titled("인력 계획 취합"));
    let hub = Hub::new();
    hub.publish("q4", a.clone()).unwrap();
    hub.publish("hr", b.clone()).unwrap();
    let server = start(config(true), hub.clone()).await.unwrap();
    let addr = server.local_addr;

    let list = http(addr, "GET", "/api/sessions", "").await;
    let (i, j) = (list.find(r#""slug":"q4""#).unwrap(), list.find(r#""slug":"hr""#).unwrap());
    assert!(i < j, "공개한 순서: {list}");
    assert!(list.contains(r#""title":"인력 계획 취합""#));
    assert!(http(addr, "GET", "/api/session?s=hr", "").await.contains(r#""title":"인력 계획 취합""#));
    assert!(http(addr, "GET", "/api/session", "").await.starts_with("HTTP/1.1 400"), "둘 이상이면 주소가 필요하다");
    assert!(!http(addr, "GET", "/hr", "").await.starts_with("HTTP/1.1 404"));
    assert!(http(addr, "GET", "/__dev/snapshot", "").await.starts_with("HTTP/1.1 400"));
    // 업그레이드 전에 거절한다(없는 업무·주소 없음).
    assert!(tokio_tungstenite::connect_async(format!("ws://{addr}/ws?s=nope")).await.is_err());
    assert!(tokio_tungstenite::connect_async(format!("ws://{addr}/ws")).await.is_err());

    // 업무마다 따로 들어간다: 같은 이름이어도 다른 세션의 사람이다.
    let mut wa = open_at(addr, "q4").await;
    let mut wb = open_at(addr, "HR").await; // 주소 경로는 대소문자를 가리지 않는다
    let welcome_a = enter(&a, &mut wa, "이서연").await;
    send(&mut wb, &join("이서연")).await;
    assert!(matches!(until(&mut wb, |m| matches!(m, ServerMsg::Pending { .. })).await, ServerMsg::Pending { .. }));
    assert!(a.snapshot().pending.is_empty() && b.snapshot().pending.len() == 1, "승인 대기는 그 업무에만");
    let ok = http(addr, "POST", "/__dev/approve?s=hr", r#"{"name":"이서연"}"#).await;
    assert!(ok.starts_with("HTTP/1.1 200"), "{ok}");
    let ServerMsg::Welcome(welcome_b) = until(&mut wb, |m| matches!(m, ServerMsg::Welcome(_))).await else {
        unreachable!()
    };
    assert_eq!(welcome_a.session.title, "2026년 부서별 예산 신청");
    assert_eq!(welcome_b.session.title, "인력 계획 취합");
    assert_ne!(welcome_a.token, welcome_b.token, "토큰도 업무마다");

    // IP 명단은 업무마다 넣는다(호스트 앱은 모든 업무에): 개발 API는 모든 업무와 목록에 넣는다.
    let r = http(addr, "POST", "/__dev/ip-names", r#"{"127.0.0.1": "김민지"}"#).await;
    assert!(r.starts_with("HTTP/1.1 200"), "{r}");
    tokio::time::sleep(Duration::from_millis(50)).await;
    let list = http(addr, "GET", "/api/sessions", "").await;
    assert_eq!(list.matches(r#""mine":true"#).count(), 2, "{list}");
    for slug in ["q4", "hr"] {
        let mut c = open_at(addr, slug).await;
        assert!(matches!(next(&mut c).await, ServerMsg::Known { name } if name == "김민지"), "{slug}");
    }

    // hr만 공개를 멈추면(업무 끝냄) hr 연결만 닫히고 q4는 그대로.
    assert!(hub.unpublish("hr", true).is_some());
    let closing = until(&mut wb, |m| matches!(m, ServerMsg::Closing { .. })).await;
    assert!(matches!(closing, ServerMsg::Closing { ended: true, .. }));
    send(&mut wa, &ClientMsg::Ping).await;
    assert!(matches!(until(&mut wa, |m| matches!(m, ServerMsg::Pong { .. })).await, ServerMsg::Pong { .. }));
    assert!(!http(addr, "GET", "/api/sessions", "").await.contains(r#""slug":"hr""#));
    assert!(http(addr, "GET", "/hr", "").await.starts_with("HTTP/1.1 404"));
    assert!(tokio_tungstenite::connect_async(format!("ws://{addr}/ws?s=hr")).await.is_err());
    // 하나만 남으면 주소 없이도 그 업무.
    assert!(http(addr, "GET", "/api/session", "").await.contains(r#""title":"2026년 부서별 예산 신청""#));

    // 다시 공개하면 토큰으로 다시 들어온다.
    hub.publish("hr", b.clone()).unwrap();
    let mut again = open_at(addr, "hr").await;
    let rejoin =
        ClientMsg::Join { name: "이서연".into(), token: Some(welcome_b.token.clone()), protocol: PROTOCOL_VERSION };
    send(&mut again, &rejoin).await;
    assert!(matches!(until(&mut again, |m| matches!(m, ServerMsg::Welcome(_))).await, ServerMsg::Welcome(_)));

    // 개발용 관리 API로 멈췄다 다시 열기.
    assert!(http(addr, "POST", "/__dev/unpublish?s=hr", "").await.starts_with("HTTP/1.1 200"));
    let closing = until(&mut again, |m| matches!(m, ServerMsg::Closing { .. })).await;
    assert!(matches!(closing, ServerMsg::Closing { ended: false, .. }));
    assert!(http(addr, "POST", "/__dev/publish?s=hr", "").await.starts_with("HTTP/1.1 200"));
    assert!(http(addr, "GET", "/api/session?s=hr", "").await.contains("인력 계획 취합"));

    // 서버를 멈추면 모두 닫힌다.
    server.stop().await;
    assert!(matches!(
        until(&mut wa, |m| matches!(m, ServerMsg::Closing { .. })).await,
        ServerMsg::Closing { ended: false, .. }
    ));
}
