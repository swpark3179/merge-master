//! 참여자 브라우저가 접속하는 HTTP/WebSocket 서버. 포트 하나에 호스트가 공개한 여러 취합 업무(세션)를 둔다([`Hub`]).
//!
//! - `GET /`                   참여자 웹(SPA) — 업무 목록. `apps/participant/dist`를 바이너리에 넣어 서빙
//! - `GET /{slug}`             참여자 웹 — 그 업무(공개 중이 아니면 404 상태로 같은 화면: '공개되어 있지 않습니다')
//! - `GET /assets/*`           SPA 정적 파일
//! - `GET /api/sessions`       공개 중인 업무 목록(업무명·요청자·마감·접속자 수·내 담당)
//! - `GET /api/session?s=slug` 접속 전 공개 정보(제목·요청자·안내·마감·접속자 수)
//! - `GET /ws?s=slug`          WebSocket — [`protocol`]. 시트는 입장한 뒤에만, 그 사람 기준으로 보낸다.
//!
//! `s`를 빼면 공개 중인 업무가 하나뿐일 때 그 업무다(둘 이상이면 400). 상태는 업무마다 [`session`] 액터 하나가 소유한다.
//! 세션은 시트를 확정할 때 만들어지고([`SessionHandle::spawn`]), 호스트가 공개하면 [`Hub`]에 들어갔다가 공개를 멈추면
//! 빠진다(세션과 편집 내용은 그대로 남는다). 서버는 공개 중인 업무가 있는 동안 열어 둔다.

mod guard;
pub mod hub;
pub mod ipnames;
mod net;
pub mod protocol;
pub mod session;
mod static_files;
mod ws;

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use axum::extract::{ConnectInfo, FromRequestParts, Query, State};
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use collect_core::CellRange;
use tokio::sync::watch;
use tokio::task::JoinHandle;

pub use hub::{Hub, SessionListItem, validate_slug};
pub use ipnames::{IpEntry, IpNames, IpNamesView};
pub use net::{LanAddress, lan_addresses};
pub use protocol::{ClientMsg, ServerMsg};
pub use session::{
    CommsSnapshot, HighlightAudience, HighlightSummary, HostEvent, HostSnapshot, NoticeSummary, NoticeTarget,
    RecalcRequest, SessionHandle, SessionInit, Shared,
};
pub use session::{META_DISCARDED, META_ENDED, META_JOB, META_SERVER, Recovered, ServerMeta, recover};

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub bind: IpAddr,
    pub port: u16,
    /// 개발용 관리 엔드포인트(`/__dev/*`: 승인·거절·연결 끊기·잠금 해제·설정)를 켠다. 개발 서버·E2E 전용.
    pub dev_admin: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    #[error("포트 {port}을(를) 열 수 없습니다: {source}")]
    Bind { port: u16, source: std::io::Error },
}

pub(crate) struct AppState {
    pub config: ServerConfig,
    pub hub: Hub,
    pub next_conn: AtomicU64,
    pub shutdown: watch::Receiver<bool>,
    /// 개발용 관리 API로 공개를 멈춘 업무(`/__dev/publish`로 다시 공개).
    parked: std::sync::Mutex<Vec<(String, SessionHandle)>>,
}

/// 실행 중인 서버. [`ServerHandle::stop`]으로 닫는다.
pub struct ServerHandle {
    pub local_addr: SocketAddr,
    state: Arc<AppState>,
    shutdown: watch::Sender<bool>,
    join: JoinHandle<()>,
}

impl ServerHandle {
    /// 이 서버가 서비스하는 업무 목록.
    pub fn hub(&self) -> &Hub {
        &self.state.hub
    }

    pub fn port(&self) -> u16 {
        self.local_addr.port()
    }

    /// 새 연결을 받지 않고, 열린 WebSocket에 종료를 알린 뒤 끝날 때까지 기다린다.
    pub async fn stop(self) {
        let _ = self.shutdown.send(true);
        let _ = self.join.await;
    }
}

pub async fn start(config: ServerConfig, hub: Hub) -> Result<ServerHandle, ServerError> {
    let addr = SocketAddr::new(config.bind, config.port);
    let listener =
        tokio::net::TcpListener::bind(addr).await.map_err(|source| ServerError::Bind { port: config.port, source })?;
    let local_addr = listener.local_addr().map_err(|source| ServerError::Bind { port: config.port, source })?;

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let state = Arc::new(AppState {
        config,
        hub,
        next_conn: AtomicU64::new(1),
        shutdown: shutdown_rx.clone(),
        parked: Default::default(),
    });

    let app = router(state.clone());
    let mut stop = shutdown_rx;
    let join = tokio::spawn(async move {
        let serve = axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
            .with_graceful_shutdown(async move {
                let _ = stop.wait_for(|v| *v).await;
            });
        if let Err(e) = serve.await {
            tracing::error!("서버 종료 오류: {e}");
        }
    });
    tracing::info!("참여자 서버 시작: http://{local_addr}/");
    Ok(ServerHandle { local_addr, state, shutdown: shutdown_tx, join })
}

fn router(state: Arc<AppState>) -> Router {
    let mut r = Router::new()
        .route("/", get(|| async { static_files::index(StatusCode::OK) }))
        .route("/api/health", get(|| async { "ok" }))
        .route("/api/sessions", get(sessions))
        .route("/api/session", get(session_info))
        .route("/ws", get(ws::upgrade))
        .route("/assets/{*path}", get(static_files::asset))
        .route("/{slug}", get(spa))
        .route("/{slug}/", get(spa));
    if state.config.dev_admin {
        r = r
            .route("/__dev/approve", post(dev_approve))
            .route("/__dev/reject", post(dev_reject))
            .route("/__dev/kick", post(dev_kick))
            .route("/__dev/unlock", post(dev_unlock))
            .route("/__dev/config", post(dev_config))
            .route("/__dev/ip-names", post(dev_ip_names))
            .route("/__dev/formulas", post(dev_formulas))
            .route("/__dev/close", post(dev_close))
            .route("/__dev/notice", post(dev_notice))
            .route("/__dev/highlight", post(dev_highlight))
            .route("/__dev/highlight/clear", post(dev_highlight_clear))
            .route("/__dev/chat", post(dev_chat))
            .route("/__dev/comms", get(dev_comms))
            .route("/__dev/history", get(dev_history))
            .route("/__dev/snapshot", get(dev_snapshot))
            .route("/__dev/unpublish", post(dev_unpublish))
            .route("/__dev/publish", post(dev_publish));
    }
    r.fallback(static_files::fallback).layer(axum::middleware::map_response(guard::security_headers)).with_state(state)
}

/// 주소 한 조각: 같은 이름의 내장 파일(`favicon.ico` 등)이 있으면 그 파일, 아니면 참여자 웹. 공개 중인 업무가 아니어도
/// 화면은 보내되(그 업무가 없다고 보이고 업무 목록으로 안내) 상태는 404.
async fn spa(State(state): State<Arc<AppState>>, axum::extract::Path(slug): axum::extract::Path<String>) -> Response {
    if let Some(file) = static_files::file(&slug) {
        return file;
    }
    let status = if state.hub.get(&slug).is_some() { StatusCode::OK } else { StatusCode::NOT_FOUND };
    static_files::index(status)
}

async fn sessions(State(state): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>) -> Response {
    Json(state.hub.list(peer.ip())).into_response()
}

/// 요청의 `?s=` 주소 경로.
#[derive(serde::Deserialize)]
pub(crate) struct Pick {
    #[serde(default)]
    pub s: Option<String>,
}

/// 요청이 가리키는 공개 중인 업무(`?s=`, 하나뿐이면 생략 가능).
pub(crate) struct Picked(SessionHandle);

impl FromRequestParts<Arc<AppState>> for Picked {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &Arc<AppState>) -> Result<Self, Self::Rejection> {
        let Query(pick) = Query::<Pick>::from_request_parts(parts, state).await.map_err(IntoResponse::into_response)?;
        let target = state.hub.resolve(pick.s.as_deref()).map_err(IntoResponse::into_response)?;
        Ok(Picked(target.session))
    }
}

async fn session_info(Picked(session): Picked) -> impl IntoResponse {
    Json(session.info())
}

#[derive(serde::Deserialize)]
struct DevName {
    name: String,
    #[serde(default)]
    range: Option<String>,
}

fn dev_result<T: serde::Serialize>(r: Result<T, String>) -> Response {
    match r {
        Ok(v) => Json(v).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e).into_response(),
    }
}

/// 개발 서버의 시트 주소(`H10`, `인력계획!E5:E9`) → (시트 순번, 범위). 개발 서버의 시트는 A1부터라 모델 좌표와 같다.
async fn dev_range(session: &SessionHandle, text: &str) -> Result<(u32, CellRange), String> {
    let names = session.with(|s| s.sheets.iter().map(|x| x.model.sheet.clone()).collect::<Vec<_>>()).await?;
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let (sheet, range) = collect_core::a1::parse_sheet_range(text, &names, 0).map_err(|e| e.to_string())?;
    Ok((sheet as u32, range))
}

async fn dev_approve(Picked(session): Picked, Json(req): Json<DevName>) -> Response {
    let range = match req.range.as_deref() {
        Some(text) => match dev_range(&session, text).await {
            Ok(r) => Some(r),
            Err(e) => return (StatusCode::BAD_REQUEST, e).into_response(),
        },
        None => None,
    };
    dev_result(session.approve(&req.name, range, "").await.map(|_| "ok"))
}

async fn dev_reject(Picked(session): Picked, Json(req): Json<DevName>) -> Response {
    dev_result(session.reject(&req.name).await.map(|_| "ok"))
}

async fn dev_kick(Picked(session): Picked, Json(req): Json<DevName>) -> Response {
    dev_result(session.kick(&req.name).await.map(|_| "ok"))
}

async fn dev_unlock(Picked(session): Picked, Json(req): Json<DevName>) -> Response {
    dev_result(session.unlock(&req.name).await.map(|_| "ok"))
}

/// 설정 일부 바꾸기(E2E: 마감 지나기, 서식 정책).
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct DevConfig {
    /// 빈 문자열이면 마감을 없앤다.
    #[serde(default)]
    deadline: Option<String>,
    #[serde(default)]
    format: Option<collect_core::config::FormatPolicy>,
    /// 참여자 행 추가 허용.
    #[serde(default)]
    add_rows: Option<bool>,
    /// 서식 정책·행 추가를 바꿀 시트(기본 첫 시트). 마감은 모든 시트가 같다.
    #[serde(default)]
    sheet: usize,
}

async fn dev_config(Picked(session): Picked, Json(req): Json<DevConfig>) -> Response {
    let r = session
        .with(move |s| {
            if req.sheet >= s.sheets.len() {
                return Err("없는 시트입니다".to_string());
            }
            if let Some(d) = req.deadline {
                s.sheets[0].config.deadline = (!d.is_empty()).then_some(d);
            }
            let config = &mut s.sheets[req.sheet].config;
            if let Some(f) = req.format {
                config.permissions.format = f;
            }
            if let Some(on) = req.add_rows {
                config.permissions.add_rows = on;
            }
            s.changed();
            Ok("ok")
        })
        .await;
    dev_result(r.and_then(|r| r))
}

/// IP 명단 바꾸기(E2E: 이 PC를 명단에 올리면 이름 없이 바로 들어온다). 본문은 명단 JSON 파일 내용. 호스트 앱처럼 모든 업무에 넣는다.
async fn dev_ip_names(State(state): State<Arc<AppState>>, body: String) -> Response {
    match IpNames::parse_json(&body) {
        Ok(names) => {
            let parked: Vec<SessionHandle> = state.parked.lock().unwrap().iter().map(|(_, s)| s.clone()).collect();
            for session in state.hub.sessions().into_iter().map(|(_, s)| s).chain(parked) {
                session.set_ip_names(names.clone());
            }
            state.hub.set_ip_names(names);
            dev_result(Ok("ok"))
        }
        Err(e) => (StatusCode::BAD_REQUEST, e).into_response(),
    }
}

/// 수식 결과 밀어 넣기(E2E: Excel 없이 재계산 결과 알림 경로를 확인). `[["G3", 0.05], ["인력계획!E3", 1], ...]`
async fn dev_formulas(Picked(session): Picked, Json(req): Json<Vec<(String, collect_core::CellValue)>>) -> Response {
    let mut values = Vec::new();
    for (a1, v) in req {
        match dev_range(&session, &a1).await {
            Ok((sheet, r)) => values.push((sheet, r.top_left(), v)),
            Err(e) => return (StatusCode::BAD_REQUEST, e).into_response(),
        }
    }
    session.formula_results(values);
    dev_result(Ok("ok"))
}

#[derive(serde::Deserialize)]
struct DevClose {
    closed: bool,
}

/// 취합 마감·다시 열기(E2E: 호스트가 저장 뒤 '읽기 전용으로 전환').
async fn dev_close(Picked(session): Picked, Json(req): Json<DevClose>) -> Response {
    dev_result(session.with(move |s| s.set_closed(req.closed)).await.map(|_| "ok"))
}

#[derive(serde::Deserialize)]
struct DevNotice {
    text: String,
    #[serde(default)]
    important: bool,
    target: NoticeTarget,
}

async fn dev_notice(Picked(session): Picked, Json(req): Json<DevNotice>) -> Response {
    dev_result(session.with(move |s| s.send_notice(&req.text, req.important, req.target)).await.map(|_| "ok"))
}

#[derive(serde::Deserialize)]
struct DevHighlight {
    /// 시트 주소(예: H10, 인력계획!E5). 개발 서버의 시트는 A1부터라 모델 좌표와 같다.
    range: String,
    #[serde(default)]
    note: String,
    audience: HighlightAudience,
}

async fn dev_highlight(Picked(session): Picked, Json(req): Json<DevHighlight>) -> Response {
    let (sheet, range) = match dev_range(&session, &req.range).await {
        Ok(r) => r,
        Err(e) => return (StatusCode::BAD_REQUEST, e).into_response(),
    };
    dev_result(session.with(move |s| s.highlight(sheet as usize, range, &req.note, req.audience)).await.map(|_| "ok"))
}

#[derive(serde::Deserialize)]
struct DevId {
    id: u32,
}

async fn dev_highlight_clear(Picked(session): Picked, Json(req): Json<DevId>) -> Response {
    dev_result(session.with(move |s| s.clear_highlight(req.id)).await.map(|_| "ok"))
}

#[derive(serde::Deserialize)]
struct DevChat {
    to: protocol::ChatTo,
    text: String,
}

async fn dev_chat(Picked(session): Picked, Json(req): Json<DevChat>) -> Response {
    dev_result(session.with(move |s| s.chat(req.to, &req.text)).await.map(|_| "ok"))
}

async fn dev_comms(Picked(session): Picked) -> impl IntoResponse {
    Json(session.comms())
}

/// 셀 변경 기록 전체(E2E 확인용).
async fn dev_history(Picked(session): Picked) -> Response {
    dev_result(session.with(|s| s.history().to_vec()).await)
}

async fn dev_snapshot(Picked(session): Picked) -> impl IntoResponse {
    Json(session.snapshot())
}

#[derive(serde::Deserialize)]
struct DevUnpublish {
    s: String,
    /// 업무를 끝낸 것으로(참여자에게 '업무가 끝났습니다').
    #[serde(default)]
    ended: bool,
}

/// 업무 공개 멈추기(E2E: 그 업무의 연결만 닫히고 목록에서 빠진다). 세션은 남겨 두었다가 `/__dev/publish`로 다시 공개한다.
async fn dev_unpublish(State(state): State<Arc<AppState>>, Query(req): Query<DevUnpublish>) -> Response {
    match state.hub.unpublish(&req.s, req.ended) {
        Some(session) => {
            state.parked.lock().unwrap().push((req.s, session));
            dev_result(Ok("ok"))
        }
        None => (StatusCode::NOT_FOUND, "공개 중인 업무가 아닙니다").into_response(),
    }
}

async fn dev_publish(State(state): State<Arc<AppState>>, Query(req): Query<Pick>) -> Response {
    let slug = req.s.unwrap_or_default();
    let mut parked = state.parked.lock().unwrap();
    let Some(i) = parked.iter().position(|(s, _)| *s == slug) else {
        return (StatusCode::NOT_FOUND, "멈춘 업무가 아닙니다").into_response();
    };
    let (slug, session) = parked.remove(i);
    dev_result(state.hub.publish(&slug, session).map(|_| "ok"))
}

#[cfg(test)]
mod tests;
