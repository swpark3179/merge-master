//! 에이전트 관리 API를 실제 소켓으로 시험한다(Excel 대신 FakeEngine, 시각은 바꿔 끼운다).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_protocol::{
    AgentInfo, Created, FolderList, HandoffManifest, OutputTarget, PROTOCOL, Package, ServiceState, ServiceStatus,
    StatusItem,
};
use chrono::NaiveDateTime;
use collect_agent::{FakeEngine, Running, StartOptions};
use collect_core::SheetModel;
use collect_core::config::{CollectConfig, suggested_key_col};
use collect_core::export::ExportOptions;
use collect_server::protocol::{ClientMsg, PROTOCOL_VERSION, ServerMsg};
use collect_server::{SessionHandle, SessionInit};
use collect_store::Store;
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::Message;

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("agent-api-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn at(text: &str) -> NaiveDateTime {
    NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M").unwrap()
}

/// 바꿔 끼우는 시계.
#[derive(Clone)]
struct TestClock(Arc<Mutex<NaiveDateTime>>);

impl TestClock {
    fn new(t: &str) -> Self {
        Self(Arc::new(Mutex::new(at(t))))
    }
    fn set(&self, t: &str) {
        *self.0.lock().unwrap() = at(t);
    }
}

struct Env {
    data: PathBuf,
    engine: Arc<FakeEngine>,
    clock: TestClock,
}

impl Env {
    fn new(name: &str) -> Env {
        let data = temp(name);
        std::fs::write(
            data.join("agent.json"),
            serde_json::json!({ "baseDir": data.join("결과"), "port": 0, "maxDays": 60 }).to_string(),
        )
        .unwrap();
        Env { data, engine: Arc::new(FakeEngine::default()), clock: TestClock::new("2026-10-20T10:00") }
    }

    async fn start(&self) -> Running {
        let mut opts = StartOptions::new(self.data.clone(), self.engine.clone());
        opts.bind = "127.0.0.1".parse().unwrap();
        opts.port = Some(0);
        let clock = self.clock.0.clone();
        opts.clock = Arc::new(move || *clock.lock().unwrap());
        opts.retry_delay = Duration::ZERO;
        opts.ticker = false;
        collect_agent::start(opts).await.unwrap()
    }

    fn base(&self) -> PathBuf {
        self.data.join("결과")
    }
}

/// 넘길 묶음: 예산신청서 세션(기록 파일)을 만들고 칸 하나를 고친 뒤 사본을 뜬다.
async fn package(dir: &Path, id: &str, slug: &str, file: &str) -> Package {
    let model: SheetModel = serde_json::from_slice(include_bytes!("../../../fixtures/budget.sheet.json")).unwrap();
    let mut config = CollectConfig::suggest(&model);
    config.assignment.key_col = suggested_key_col(&model);
    config.title = "2026 예산".into();
    config.requester = "박지훈".into();
    let sheet = model.sheet.clone();
    let src = dir.join(format!("{id}-src.sqlite"));
    let copy = dir.join(format!("{id}-copy.sqlite"));
    let store = Store::open(&src).unwrap();
    store.meta("workbook", r"C:\work\budget.xlsx");
    let session =
        SessionHandle::spawn(SessionInit { sheets: vec![(model, config)], store: Some(store), restore: None });
    session
        .with(|s| {
            let at = collect_core::CellRef::new(2, 5);
            s.host_set(
                "박지훈",
                collect_core::history::BatchOrigin::Edit,
                0,
                vec![(at, collect_core::CellValue::Number(7.0))],
            );
        })
        .await
        .unwrap();
    session.backup_store(copy.clone()).await.unwrap();
    let manifest = HandoffManifest {
        id: id.into(),
        token: format!("{id}-token"),
        job_id: "job-1".into(),
        host_pc: "HOST-PC".into(),
        host_version: "1.7.4".into(),
        host_name: Some("박지훈".into()),
        title: "2026 예산".into(),
        slug: slug.into(),
        deadline: "2026-10-20T18:00".into(),
        output: OutputTarget { dir: vec!["2026".into(), "예산".into()], file_name: file.into() },
        export: ExportOptions { mark: None, styles: true, history: true, cancelled: true },
        workbook_name: "budget.xlsx".into(),
        sheets: vec![sheet],
        ip_names: Default::default(),
    };
    Package { manifest, store: std::fs::read(&copy).unwrap(), workbook: b"PK fake xlsx".to_vec() }
}

struct Reply {
    status: u16,
    body: Vec<u8>,
}

impl Reply {
    fn json<T: serde::de::DeserializeOwned>(&self) -> T {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|e| panic!("{e}: {} {}", self.status, String::from_utf8_lossy(&self.body)))
    }
    fn message(&self) -> String {
        self.json::<serde_json::Value>()["message"].as_str().unwrap_or_default().to_string()
    }
}

/// 아주 작은 HTTP/1.1 클라이언트(연결마다 요청 하나).
async fn request(port: u16, method: &str, path: &str, headers: &[(&str, String)], body: &[u8]) -> Reply {
    let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let mut head = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Length: {}\r\n",
        body.len()
    );
    for (k, v) in headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    s.write_all(head.as_bytes()).await.unwrap();
    s.write_all(body).await.unwrap();
    let mut out = Vec::new();
    s.read_to_end(&mut out).await.unwrap();
    let split = out.windows(4).position(|w| w == b"\r\n\r\n").expect("응답 머리") + 4;
    let head = String::from_utf8_lossy(&out[..split]).to_string();
    let status = head.split_whitespace().nth(1).unwrap().parse().unwrap();
    let mut body = out[split..].to_vec();
    if head.to_ascii_lowercase().contains("transfer-encoding: chunked") {
        body = dechunk(&body);
    }
    Reply { status, body }
}

fn dechunk(mut b: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let line = b.windows(2).position(|w| w == b"\r\n").unwrap();
        let n = usize::from_str_radix(std::str::from_utf8(&b[..line]).unwrap().trim(), 16).unwrap();
        if n == 0 {
            return out;
        }
        out.extend_from_slice(&b[line + 2..line + 2 + n]);
        b = &b[line + 2 + n + 2..];
    }
}

fn agent_header() -> (&'static str, String) {
    ("x-mm-agent", PROTOCOL.to_string())
}

async fn api(port: u16, method: &str, path: &str, body: &[u8], token: Option<&str>) -> Reply {
    let mut h = vec![agent_header(), ("Content-Type", "application/json".to_string())];
    if let Some(t) = token {
        h.push(("x-mm-token", t.to_string()));
    }
    request(port, method, path, &h, body).await
}

async fn create(port: u16, p: &Package) -> Reply {
    request(port, "POST", "/agent/services", &[agent_header()], &p.encode()).await
}

async fn status(port: u16, id: &str, token: &str) -> StatusItem {
    let body = serde_json::json!([{ "id": id, "token": token }]).to_string();
    let mut items: Vec<StatusItem> = api(port, "POST", "/agent/status", body.as_bytes(), None).await.json();
    items.remove(0)
}

async fn finish(port: u16, id: &str, token: &str, save: bool) -> Reply {
    let body = serde_json::json!({ "save": save }).to_string();
    api(port, "POST", &format!("/agent/services/{id}/finish"), body.as_bytes(), Some(token)).await
}

/// 상태가 `want`가 될 때까지(저장은 뒤에서 한다).
async fn until(port: u16, id: &str, token: &str, want: ServiceState) -> ServiceStatus {
    for _ in 0..200 {
        let s = status(port, id, token).await.status.unwrap();
        if s.state == want {
            return s;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("{id}이(가) {want:?}이(가) 되지 않았습니다");
}

#[tokio::test]
async fn hand_off_serve_approve_and_save() {
    let env = Env::new("flow");
    let running = env.start().await;
    let port = running.port();

    let info: AgentInfo = api(port, "GET", "/agent/info", b"", None).await.json();
    assert_eq!((info.protocol, info.services), (PROTOCOL, 0));
    assert!(info.excel && info.base_ok);

    // 받기 → 주소 경로 q4, 같은 경로를 바라는 두 번째는 q4-2.
    let p1 = package(&env.data, "svc-1", "q4", "budget_취합.xlsx").await;
    let c1 = create(port, &p1).await;
    assert_eq!(c1.status, 200, "{}", c1.message());
    let c1: Created = c1.json();
    assert_eq!(c1.slug, "q4");
    assert!(c1.output_path.ends_with("budget_취합.xlsx"));
    let p2 = package(&env.data, "svc-2", "q4", "budget_취합.xlsx").await;
    let c2: Created = create(port, &p2).await.json();
    assert_eq!(c2.slug, "q4-2");
    // 같은 id·관리 키로 다시 보내면 같은 서비스, 관리 키가 다르면 거절.
    let again: Created = create(port, &p1).await.json();
    assert_eq!(again.slug, "q4");
    let mut other = p1.clone();
    other.manifest.token = "다른 키".into();
    assert_eq!(create(port, &other).await.status, 409);

    // 참여자 서버: 받은 업무가 그 경로로 공개되어 있다.
    let session = request(port, "GET", "/api/session?s=q4", &[], b"").await;
    assert_eq!(session.status, 200);
    assert!(String::from_utf8_lossy(&session.body).contains("2026 예산"));

    // 명단 밖 참여자 → 승인 대기 → 관리 API로 승인.
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/ws?s=q4")).await.unwrap();
    let join = ClientMsg::Join { name: "정하늘".into(), token: None, protocol: PROTOCOL_VERSION };
    ws.send(Message::Text(serde_json::to_string(&join).unwrap().into())).await.unwrap();
    let mut pending = Vec::new();
    for _ in 0..100 {
        pending = status(port, "svc-1", "svc-1-token").await.status.unwrap().pending;
        if !pending.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(pending[0].name, "정하늘");
    let body = serde_json::json!({ "name": "정하늘" }).to_string();
    assert_eq!(api(port, "POST", "/agent/services/svc-1/approve", body.as_bytes(), Some("틀린 키")).await.status, 403);
    let ok = api(port, "POST", "/agent/services/svc-1/approve", body.as_bytes(), Some("svc-1-token")).await;
    assert_eq!(ok.status, 200, "{}", ok.message());
    let welcome = loop {
        let Some(Ok(Message::Text(t))) = ws.next().await else { panic!("연결이 끊겼습니다") };
        if let Ok(ServerMsg::Welcome(w)) = serde_json::from_str::<ServerMsg>(&t) {
            break w;
        }
    };
    assert_eq!(welcome.name, "정하늘");
    let s = status(port, "svc-1", "svc-1-token").await.status.unwrap();
    assert!(s.pending.is_empty() && s.online == 1);
    assert_eq!(s.state, ServiceState::Running);
    assert!(s.urls[0].ends_with(&format!(":{port}/q4")), "{:?}", s.urls);

    // 관리 키가 틀리면 상태도 볼 수 없다.
    assert!(status(port, "svc-1", "틀린 키").await.forbidden);
    assert!(status(port, "없는-id", "x").await.missing);

    // 지금 저장하고 끝내기 → 저장한 뒤 참여자에게 '끝났다'.
    let r = finish(port, "svc-1", "svc-1-token", true).await;
    assert_eq!(r.status, 200, "{}", r.message());
    let done = until(port, "svc-1", "svc-1-token", ServiceState::Done).await;
    let saved = PathBuf::from(done.saved_path.clone().unwrap());
    assert_eq!(saved, env.base().join("2026").join("예산").join("budget_취합.xlsx"));
    let summary: Vec<(String, usize)> = serde_json::from_slice(&std::fs::read(&saved).unwrap()).unwrap();
    assert_eq!(summary[0].1, 1, "고친 칸 하나를 쓴다");
    let closing = loop {
        match ws.next().await {
            Some(Ok(Message::Text(t))) => {
                if let Ok(m @ ServerMsg::Closing { .. }) = serde_json::from_str::<ServerMsg>(&t) {
                    break m;
                }
            }
            other => panic!("closing을 받지 못했습니다: {other:?}"),
        }
    };
    assert!(matches!(closing, ServerMsg::Closing { ended: true, .. }));
    assert!(done.urls.is_empty());
    assert_eq!(request(port, "GET", "/api/session?s=q4", &[], b"").await.status, 404);

    // 같은 이름이 이미 있으면 덮지 않고 _2.
    finish(port, "svc-2", "svc-2-token", true).await;
    let done2 = until(port, "svc-2", "svc-2-token", ServiceState::Done).await;
    assert!(done2.saved_path.unwrap().ends_with("budget_취합_2.xlsx"));

    // 끝난 뒤 같은 경로를 바라면 다시 q4.
    let p3 = package(&env.data, "svc-3", "q4", "c.xlsx").await;
    let c3: Created = create(port, &p3).await.json();
    assert_eq!(c3.slug, "q4");
    // 저장 없이 끝내기.
    let cancelled: ServiceStatus = finish(port, "svc-3", "svc-3-token", false).await.json();
    assert_eq!(cancelled.state, ServiceState::Cancelled);
    assert!(!env.base().join("2026/예산/c.xlsx").exists());
    running.stop().await;
}

#[tokio::test]
async fn deadline_saves_and_failures_are_retried() {
    let env = Env::new("deadline");
    let running = env.start().await;
    let port = running.port();
    let p = package(&env.data, "svc-d", "hr", "인력.xlsx").await;
    assert_eq!(create(port, &p).await.status, 200);

    // 마감 전에는 아무것도 하지 않는다.
    assert!(running.agent.tick().is_empty());
    // 마감이 지나면 저장한다. 두 번 실패하고 세 번째에 된다.
    env.engine.fail_next(2);
    env.clock.set("2026-10-20T18:00");
    for h in running.agent.tick() {
        h.await.unwrap();
    }
    let s = status(port, "svc-d", "svc-d-token").await.status.unwrap();
    assert_eq!((s.state, s.attempts), (ServiceState::Saving, 1));
    assert!(s.last_error.is_some());
    for _ in 0..2 {
        for h in running.agent.tick() {
            h.await.unwrap();
        }
    }
    let s = status(port, "svc-d", "svc-d-token").await.status.unwrap();
    assert_eq!((s.state, s.attempts), (ServiceState::Done, 3));
    assert!(env.base().join("2026/예산/인력.xlsx").exists());

    // 세 번 모두 실패하면 '저장 실패' → 다시 저장.
    env.clock.set("2026-10-20T10:00");
    let p = package(&env.data, "svc-f", "hr", "실패.xlsx").await;
    assert_eq!(create(port, &p).await.status, 200);
    env.clock.set("2026-10-20T18:01");
    env.engine.fail_next(3);
    for _ in 0..3 {
        for h in running.agent.tick() {
            h.await.unwrap();
        }
    }
    let s = status(port, "svc-f", "svc-f-token").await.status.unwrap();
    assert_eq!(s.state, ServiceState::Failed);
    assert_eq!(request(port, "GET", "/api/session?s=hr", &[], b"").await.status, 404, "공유는 닫는다");
    let r = api(port, "POST", "/agent/services/svc-f/retry", b"", Some("svc-f-token")).await;
    assert_eq!(r.status, 200, "{}", r.message());
    until(port, "svc-f", "svc-f-token", ServiceState::Done).await;
    running.stop().await;
}

#[tokio::test]
async fn services_come_back_after_restart() {
    let env = Env::new("restart");
    let running = env.start().await;
    let port = running.port();
    let mut later = package(&env.data, "svc-later", "a", "a.xlsx").await;
    later.manifest.deadline = "2026-10-21T18:00".into();
    let soon = package(&env.data, "svc-soon", "b", "b.xlsx").await;
    assert_eq!(create(port, &later).await.status, 200);
    assert_eq!(create(port, &soon).await.status, 200);
    running.stop().await;

    // 꺼진 사이 svc-soon의 마감이 지났다.
    env.clock.set("2026-10-20T19:00");
    let running = env.start().await;
    let port = running.port();
    let s = status(port, "svc-later", "svc-later-token").await.status.unwrap();
    assert_eq!(s.state, ServiceState::Running);
    assert_eq!(request(port, "GET", "/api/session?s=a", &[], b"").await.status, 200, "다시 공개");
    assert_eq!(status(port, "svc-soon", "svc-soon-token").await.status.unwrap().state, ServiceState::Saving);
    assert_eq!(request(port, "GET", "/api/session?s=b", &[], b"").await.status, 404);
    for h in running.agent.tick() {
        h.await.unwrap();
    }
    assert_eq!(status(port, "svc-soon", "svc-soon-token").await.status.unwrap().state, ServiceState::Done);
    assert!(env.base().join("2026/예산/b.xlsx").exists());
    running.stop().await;
}

#[tokio::test]
async fn bad_requests_are_refused() {
    let env = Env::new("bad");
    let running = env.start().await;
    let port = running.port();

    // 헤더 없음 · 다른 판 · 브라우저.
    assert_eq!(request(port, "GET", "/agent/info", &[], b"").await.status, 400);
    let old = request(port, "GET", "/agent/info", &[("x-mm-agent", "99".into())], b"").await;
    assert_eq!(old.status, 426);
    assert!(old.message().contains("버전"));
    let browser = request(port, "GET", "/agent/info", &[agent_header(), ("Origin", "http://evil".into())], b"").await;
    assert_eq!(browser.status, 403);

    // 폴더: 기본 폴더 밖으로 못 나간다, 없는 폴더는 '저장할 때 만듦'.
    std::fs::create_dir_all(env.base().join("2026")).unwrap();
    let root: FolderList = api(port, "GET", "/agent/folders", b"", None).await.json();
    assert_eq!(root.folders, ["2026"]);
    let missing: FolderList = api(port, "GET", "/agent/folders?path=2026%2F%EC%83%88", b"", None).await.json();
    assert!(!missing.exists);
    assert_eq!(api(port, "GET", "/agent/folders?path=..", b"", None).await.status, 400);
    assert_eq!(api(port, "GET", "/agent/folders?path=C%3A%5CWindows", b"", None).await.status, 400);

    // 묶음 검사.
    assert_eq!(request(port, "POST", "/agent/services", &[agent_header()], b"garbage").await.status, 400);
    let good = package(&env.data, "svc-x", "q4", "x.xlsx").await;
    let mut p = good.clone();
    p.manifest.deadline = "2026-10-20T10:00".into();
    assert!(create(port, &p).await.message().contains("마감"), "지금은 마감이 될 수 없다");
    let mut p = good.clone();
    p.manifest.deadline = "2027-01-30T10:00".into();
    assert!(create(port, &p).await.message().contains("60일"));
    let mut p = good.clone();
    p.manifest.output.file_name = "x.xlsm".into();
    assert!(create(port, &p).await.message().contains("같은 형식"));
    let mut p = good.clone();
    p.manifest.output.dir = vec!["..".into()];
    assert_eq!(create(port, &p).await.status, 400);
    let mut p = good.clone();
    p.manifest.output.file_name = format!("{}.xlsx", "가".repeat(115));
    p.manifest.output.dir = vec!["가".repeat(100)];
    assert!(create(port, &p).await.message().contains("너무 깁니다"));
    let mut p = good.clone();
    p.store = b"not sqlite".to_vec();
    assert!(create(port, &p).await.message().contains("기록 파일"));
    assert_eq!(
        api(port, "GET", "/agent/info", b"", None).await.json::<AgentInfo>().services,
        0,
        "실패한 받기는 남지 않는다"
    );
    assert!(!env.data.join("services").join("svc-x").exists());
    running.stop().await;
}

#[tokio::test]
async fn host_client_talks_to_the_agent() {
    use agent_protocol::client::{AgentClient, ClientError};
    let env = Env::new("client");
    let running = env.start().await;
    let client = AgentClient::new(&format!("127.0.0.1:{}", running.port())).unwrap();
    let info = client.info().await.unwrap();
    assert_eq!(info.port, running.port());
    std::fs::create_dir_all(env.base().join("2026").join("새 폴더")).unwrap();
    let list = client.folders(&["2026".into()]).await.unwrap();
    assert_eq!(list.folders, ["새 폴더"]);
    assert!(matches!(client.folders(&["..".into()]).await, Err(ClientError::Api { status: 400, .. })));

    let p = package(&env.data, "svc-c", "q4", "c.xlsx").await;
    let created = client.create(&p).await.unwrap();
    assert_eq!(created.slug, "q4");
    let token = p.manifest.token.clone();
    let items =
        client.status(&[agent_protocol::StatusQuery { id: "svc-c".into(), token: token.clone() }]).await.unwrap();
    assert_eq!(items[0].status.as_ref().unwrap().state, ServiceState::Running);
    let err = client.reject("svc-c", &token, "없는 사람").await.unwrap_err();
    assert!(matches!(err, ClientError::Api { status: 400, .. }), "{err:?}");
    assert!(matches!(client.finish("svc-c", "틀린 키", true).await, Err(ClientError::Api { status: 403, .. })));
    client.finish("svc-c", &token, true).await.unwrap();
    until(running.port(), "svc-c", &token, ServiceState::Done).await;
    assert!(matches!(client.retry("svc-c", &token).await, Err(ClientError::Api { status: 409, .. })));
    running.stop().await;

    // 꺼진 에이전트
    assert!(matches!(client.info().await, Err(ClientError::Unreachable(_))));
}
