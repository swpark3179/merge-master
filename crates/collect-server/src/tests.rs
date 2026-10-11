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
    ServerConfig { bind: IpAddr::V4(Ipv4Addr::LOCALHOST), port: 0, slug: "q4".into(), dev_admin }
}

fn init() -> SessionHandle {
    let model: SheetModel = serde_json::from_slice(include_bytes!("../../../fixtures/budget.sheet.json")).unwrap();
    let mut config = CollectConfig::suggest(&model);
    config.assignment.key_col = suggested_key_col(&model); // C 담당자
    config.title = "2026년 부서별 예산 신청".into();
    config.requester = "박지훈".into();
    SessionHandle::spawn(SessionInit::new(model, config))
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
    let server = start(config(false), init()).await.unwrap();
    let addr = server.local_addr;
    assert!(http(addr, "GET", "/api/health", "").await.ends_with("ok"));
    let info = http(addr, "GET", "/api/session", "").await;
    assert!(info.contains(r#""title":"2026년 부서별 예산 신청""#) && info.contains(r#""requester":"박지훈""#));
    // 시트는 입장한 뒤 WebSocket으로만, 그 사람 기준으로 보낸다.
    assert!(http(addr, "GET", "/api/sheet", "").await.starts_with("HTTP/1.1 404"));
    assert!(http(addr, "GET", "/__dev/snapshot", "").await.starts_with("HTTP/1.1 404"));
    assert!(http(addr, "GET", "/", "").await.contains("location: /q4"));
    assert!(http(addr, "GET", "/wrong", "").await.starts_with("HTTP/1.1 404"));
    assert!(http(addr, "GET", "/assets/../../Cargo.toml", "").await.starts_with("HTTP/1.1 404"));
    server.stop().await;
}

#[tokio::test]
async fn join_pending_approve_and_switch_over_websocket() {
    let server = start(config(true), init()).await.unwrap();
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
    server.session().set_ip_names(names);
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
    assert_eq!(server.online(), 2);
    tokio::time::timeout(Duration::from_secs(5), server.stop()).await.expect("stop()이 끝나지 않음");
    assert!(matches!(until(&mut b, |m| matches!(m, ServerMsg::Closing)).await, ServerMsg::Closing));
}

#[tokio::test]
async fn host_events_follow_participants() {
    let server = start(config(false), init()).await.unwrap();
    let mut events = server.session().subscribe();
    let mut a = open(server.local_addr).await;
    enter(server.session(), &mut a, "이서연").await;
    tokio::time::timeout(Duration::from_secs(5), events.wait_for(|s| s.online == 1)).await.unwrap().unwrap();
    drop(a);
    tokio::time::timeout(Duration::from_secs(5), events.wait_for(|s| s.online == 0)).await.unwrap().unwrap();
    server.stop().await;
}

#[tokio::test]
async fn bind_conflict_is_reported() {
    let first = start(config(false), init()).await.unwrap();
    let mut cfg = config(false);
    cfg.port = first.local_addr.port();
    assert!(matches!(start(cfg, init()).await, Err(ServerError::Bind { .. })));
    first.stop().await;
}

/// 세션은 서버보다 오래 산다: 서비스를 멈췄다 다시 열어도 입력한 값이 그대로다.
#[tokio::test]
async fn edits_flow_over_websocket_and_survive_restart() {
    use crate::protocol::CellEdit;
    use collect_core::CellValue;

    let session = init();
    let server = start(config(true), session.clone()).await.unwrap();
    let addr = server.local_addr;
    let mut a = open(addr).await;
    let mut b = open(addr).await;
    let w = enter(&session, &mut a, "이서연").await;
    let tb = enter(&session, &mut b, "최도윤").await.token;
    let row = w.sheets[0].row_owner.iter().position(|&o| o == 0).unwrap() as u32;

    let cells = vec![CellEdit { row, col: 5, value: CellValue::Text("9,800".into()), base: 0 }];
    send(&mut a, &ClientMsg::SetCells { sheet: 0, batch: "b1".into(), cells, origin: None }).await;
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
    let again = start(config(false), session.clone()).await.unwrap();
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

    let server = start(config(false), init()).await.unwrap();
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
    let server = start(config(false), init()).await.unwrap();
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
    let server = start(config(false), init()).await.unwrap();
    let addr = server.local_addr;
    assert!(http(addr, "GET", "/api/health", "").await.ends_with("ok"));
    server.stop().await;
    assert!(tokio::net::TcpStream::connect(addr).await.is_err(), "서비스를 멈추면 포트도 닫힌다");
}
