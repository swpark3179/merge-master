//! 참여자 브라우저가 접속하는 HTTP/WebSocket 서버.
//!
//! - `GET /{slug}`        참여자 웹(SPA) — `apps/participant/dist`를 바이너리에 넣어 서빙
//! - `GET /assets/*`      SPA 정적 파일
//! - `GET /api/session`   접속 전 공개 정보(제목·요청자·안내·마감·접속자 수)
//! - `GET /ws`            WebSocket — [`protocol`] v2. 시트는 입장한 뒤에만, 그 사람 기준으로 보낸다.
//!
//! 상태는 [`session`] 액터 하나가 소유한다. 세션은 시트를 확정할 때 만들어지고([`SessionHandle::spawn`]),
//! 서버는 서비스를 시작할 때 그 세션에 붙었다가 중지하면 떨어진다(세션과 편집 내용은 그대로 남는다).

mod guard;
mod net;
pub mod protocol;
pub mod session;
mod static_files;
mod ws;

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use collect_core::CellRange;
use tokio::sync::watch;
use tokio::task::JoinHandle;

pub use net::{LanAddress, lan_addresses};
pub use protocol::{ClientMsg, ServerMsg};
pub use session::{
    CommsSnapshot, HighlightAudience, HighlightSummary, HostEvent, HostSnapshot, NoticeSummary, NoticeTarget,
    RecalcRequest, SessionHandle, SessionInit, Shared,
};
pub use session::{META_SERVER, Recovered, ServerMeta, recover};

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub bind: IpAddr,
    pub port: u16,
    /// 접속 주소 경로(`http://IP:PORT/{slug}`).
    pub slug: String,
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
    pub session: SessionHandle,
    pub next_conn: AtomicU64,
    pub shutdown: watch::Receiver<bool>,
}

/// 실행 중인 서버. [`ServerHandle::stop`]으로 닫는다.
pub struct ServerHandle {
    pub local_addr: SocketAddr,
    state: Arc<AppState>,
    shutdown: watch::Sender<bool>,
    join: JoinHandle<()>,
}

impl ServerHandle {
    pub fn session(&self) -> &SessionHandle {
        &self.state.session
    }

    /// 접속 주소 경로.
    pub fn slug(&self) -> &str {
        &self.state.config.slug
    }

    pub fn online(&self) -> usize {
        self.state.session.snapshot().online as usize
    }

    /// 새 연결을 받지 않고, 열린 WebSocket에 종료를 알린 뒤 끝날 때까지 기다린다.
    pub async fn stop(self) {
        let _ = self.shutdown.send(true);
        let _ = self.join.await;
    }
}

pub async fn start(config: ServerConfig, session: SessionHandle) -> Result<ServerHandle, ServerError> {
    let addr = SocketAddr::new(config.bind, config.port);
    let listener =
        tokio::net::TcpListener::bind(addr).await.map_err(|source| ServerError::Bind { port: config.port, source })?;
    let local_addr = listener.local_addr().map_err(|source| ServerError::Bind { port: config.port, source })?;

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let state = Arc::new(AppState { config, session, next_conn: AtomicU64::new(1), shutdown: shutdown_rx.clone() });

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
    tracing::info!("참여자 서버 시작: http://{local_addr}/{}", state.config.slug);
    Ok(ServerHandle { local_addr, state, shutdown: shutdown_tx, join })
}

fn router(state: Arc<AppState>) -> Router {
    let mut r = Router::new()
        .route("/", get(root))
        .route("/api/health", get(|| async { "ok" }))
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
            .route("/__dev/formulas", post(dev_formulas))
            .route("/__dev/close", post(dev_close))
            .route("/__dev/notice", post(dev_notice))
            .route("/__dev/highlight", post(dev_highlight))
            .route("/__dev/highlight/clear", post(dev_highlight_clear))
            .route("/__dev/chat", post(dev_chat))
            .route("/__dev/comms", get(dev_comms))
            .route("/__dev/history", get(dev_history))
            .route("/__dev/snapshot", get(dev_snapshot));
    }
    r.fallback(static_files::fallback).layer(axum::middleware::map_response(guard::security_headers)).with_state(state)
}

async fn root(State(state): State<Arc<AppState>>) -> Redirect {
    Redirect::temporary(&format!("/{}", state.config.slug))
}

async fn spa(State(state): State<Arc<AppState>>, axum::extract::Path(slug): axum::extract::Path<String>) -> Response {
    if slug != state.config.slug {
        return (StatusCode::NOT_FOUND, "취합 주소가 맞는지 확인해 주세요").into_response();
    }
    static_files::index()
}

async fn session_info(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    Json(state.session.info())
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

async fn dev_approve(State(state): State<Arc<AppState>>, Json(req): Json<DevName>) -> Response {
    let range = match req.range.as_deref().map(CellRange::parse).transpose() {
        Ok(r) => r,
        Err(e) => return (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    };
    dev_result(state.session.approve(&req.name, range, "").await.map(|_| "ok"))
}

async fn dev_reject(State(state): State<Arc<AppState>>, Json(req): Json<DevName>) -> Response {
    dev_result(state.session.reject(&req.name).await.map(|_| "ok"))
}

async fn dev_kick(State(state): State<Arc<AppState>>, Json(req): Json<DevName>) -> Response {
    dev_result(state.session.kick(&req.name).await.map(|_| "ok"))
}

async fn dev_unlock(State(state): State<Arc<AppState>>, Json(req): Json<DevName>) -> Response {
    dev_result(state.session.unlock(&req.name).await.map(|_| "ok"))
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
}

async fn dev_config(State(state): State<Arc<AppState>>, Json(req): Json<DevConfig>) -> Response {
    let r = state
        .session
        .with(move |s| {
            if let Some(d) = req.deadline {
                s.config.deadline = (!d.is_empty()).then_some(d);
            }
            if let Some(f) = req.format {
                s.config.permissions.format = f;
            }
            s.changed();
        })
        .await;
    dev_result(r.map(|_| "ok"))
}

/// 수식 결과 밀어 넣기(E2E: Excel 없이 재계산 결과 알림 경로를 확인). `[["G3", 0.05], ...]`
async fn dev_formulas(
    State(state): State<Arc<AppState>>,
    Json(req): Json<Vec<(String, collect_core::CellValue)>>,
) -> Response {
    let mut values = Vec::new();
    for (a1, v) in req {
        match collect_core::CellRef::parse(&a1) {
            Ok(at) => values.push((at, v)),
            Err(e) => return (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
        }
    }
    state.session.formula_results(values);
    dev_result(Ok("ok"))
}

#[derive(serde::Deserialize)]
struct DevClose {
    closed: bool,
}

/// 취합 마감·다시 열기(E2E: 호스트가 저장 뒤 '읽기 전용으로 전환').
async fn dev_close(State(state): State<Arc<AppState>>, Json(req): Json<DevClose>) -> Response {
    dev_result(state.session.with(move |s| s.set_closed(req.closed)).await.map(|_| "ok"))
}

#[derive(serde::Deserialize)]
struct DevNotice {
    text: String,
    #[serde(default)]
    important: bool,
    target: NoticeTarget,
}

async fn dev_notice(State(state): State<Arc<AppState>>, Json(req): Json<DevNotice>) -> Response {
    dev_result(state.session.with(move |s| s.send_notice(&req.text, req.important, req.target)).await.map(|_| "ok"))
}

#[derive(serde::Deserialize)]
struct DevHighlight {
    /// 시트 주소(예: H10). 개발 서버의 시트는 A1부터라 모델 좌표와 같다.
    range: String,
    #[serde(default)]
    note: String,
    audience: HighlightAudience,
}

async fn dev_highlight(State(state): State<Arc<AppState>>, Json(req): Json<DevHighlight>) -> Response {
    let range = match CellRange::parse(&req.range) {
        Ok(r) => r,
        Err(e) => return (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    };
    dev_result(state.session.with(move |s| s.highlight(range, &req.note, req.audience)).await.map(|_| "ok"))
}

#[derive(serde::Deserialize)]
struct DevId {
    id: u32,
}

async fn dev_highlight_clear(State(state): State<Arc<AppState>>, Json(req): Json<DevId>) -> Response {
    dev_result(state.session.with(move |s| s.clear_highlight(req.id)).await.map(|_| "ok"))
}

#[derive(serde::Deserialize)]
struct DevChat {
    to: protocol::ChatTo,
    text: String,
}

async fn dev_chat(State(state): State<Arc<AppState>>, Json(req): Json<DevChat>) -> Response {
    dev_result(state.session.with(move |s| s.chat(req.to, &req.text)).await.map(|_| "ok"))
}

async fn dev_comms(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    Json(state.session.comms())
}

/// 셀 변경 기록 전체(E2E 확인용).
async fn dev_history(State(state): State<Arc<AppState>>) -> Response {
    dev_result(state.session.with(|s| s.history().to_vec()).await)
}

async fn dev_snapshot(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    Json(state.session.snapshot())
}

#[cfg(test)]
mod tests;
