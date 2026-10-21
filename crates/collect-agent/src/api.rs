//! 관리 API(`/agent/*`): 참여자 서버와 같은 포트에 붙는다([`collect_server::start_with`]).

use std::sync::Arc;

use agent_protocol::{
    ApiError, Approve, Finish, HEADER_AGENT, HEADER_TOKEN, HostSaved, MAX_PACKAGE, MIN_PROTOCOL, PROTOCOL, Package,
    Reject, ServiceStatus, StatusQuery,
};
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, Query, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;

use crate::agent::{Agent, AgentError};

type Shared = State<Arc<Agent>>;

impl IntoResponse for AgentError {
    fn into_response(self) -> Response {
        let status = StatusCode::from_u16(self.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        (status, Json(ApiError { message: self.message })).into_response()
    }
}

fn error(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(ApiError { message: message.into() })).into_response()
}

pub fn router(agent: Arc<Agent>) -> Router {
    Router::new()
        .route("/agent/info", get(info))
        .route("/agent/folders", get(folders))
        .route("/agent/services", post(create).layer(DefaultBodyLimit::max(MAX_PACKAGE)))
        .route("/agent/status", post(status))
        .route("/agent/services/{id}/approve", post(approve))
        .route("/agent/services/{id}/reject", post(reject))
        .route("/agent/services/{id}/finish", post(finish))
        .route("/agent/services/{id}/retry", post(retry))
        .route("/agent/services/{id}/save-data", get(save_data))
        .route("/agent/services/{id}/source", get(source))
        .route("/agent/services/{id}/host-saved", post(host_saved))
        .route("/agent/services/{id}/host", get(crate::host_link::upgrade))
        .layer(middleware::from_fn(guard))
        .with_state(agent)
}

/// 호스트 앱의 요청만 받는다: 관리 API 판 헤더가 있어야 하고(받는 판 밖이면 426), 브라우저 요청(`Origin`)은 받지 않는다 —
/// 참여자가 연 다른 사이트의 페이지가 그 브라우저를 통해 에이전트에 서비스를 넘기거나 끝내거나 관리자로 붙지 못하게.
async fn guard(req: Request, next: Next) -> Response {
    if req.headers().contains_key(header::ORIGIN) {
        return error(StatusCode::FORBIDDEN, "브라우저에서는 에이전트를 다룰 수 없습니다");
    }
    let version =
        req.headers().get(HEADER_AGENT).and_then(|v| v.to_str().ok()).and_then(|v| v.trim().parse::<u32>().ok());
    match version {
        None => error(StatusCode::BAD_REQUEST, "에이전트 관리 요청이 아닙니다"),
        Some(v) if !(MIN_PROTOCOL..=PROTOCOL).contains(&v) => error(
            StatusCode::UPGRADE_REQUIRED,
            format!(
                "호스트 앱(관리 API {v})과 에이전트(관리 API {PROTOCOL})의 버전이 다릅니다. 같은 버전의 에이전트로 바꿔 주세요"
            ),
        ),
        Some(_) => next.run(req).await,
    }
}

/// 요청한 호스트 앱의 관리 API 판([`guard`]를 거쳤으니 있다).
pub(crate) fn protocol(headers: &HeaderMap) -> u32 {
    headers.get(HEADER_AGENT).and_then(|v| v.to_str().ok()).and_then(|v| v.trim().parse().ok()).unwrap_or(MIN_PROTOCOL)
}

/// 서비스 상태를 그 호스트 앱이 읽을 수 있게 보낸다.
fn status_reply(headers: &HeaderMap, status: ServiceStatus) -> Response {
    Json(status.for_protocol(protocol(headers))).into_response()
}

fn bytes_reply(bytes: Vec<u8>) -> Response {
    ([(header::CONTENT_TYPE, "application/octet-stream")], bytes).into_response()
}

pub(crate) fn token(headers: &HeaderMap) -> Result<String, AgentError> {
    headers
        .get(HEADER_TOKEN)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
        .filter(|t| !t.is_empty())
        .ok_or_else(AgentError::forbidden)
}

async fn info(State(agent): Shared) -> Response {
    Json(agent.info()).into_response()
}

#[derive(Deserialize)]
struct FolderQuery {
    #[serde(default)]
    path: String,
}

async fn folders(State(agent): Shared, Query(q): Query<FolderQuery>) -> Result<Response, AgentError> {
    Ok(Json(agent.folders(&q.path)?).into_response())
}

async fn create(State(agent): Shared, body: Bytes) -> Result<Response, AgentError> {
    let pkg = Package::decode(&body)?;
    Ok(Json(agent.create(pkg).await?).into_response())
}

async fn status(State(agent): Shared, headers: HeaderMap, Json(queries): Json<Vec<StatusQuery>>) -> Response {
    let v = protocol(&headers);
    let mut items = agent.status(&queries);
    for item in &mut items {
        item.status = item.status.take().map(|s| s.for_protocol(v));
    }
    Json(items).into_response()
}

async fn approve(
    State(agent): Shared,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Approve>,
) -> Result<Response, AgentError> {
    let token = token(&headers)?;
    Ok(status_reply(&headers, agent.approve(&id, Some(&token), body).await?))
}

async fn reject(
    State(agent): Shared,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Reject>,
) -> Result<Response, AgentError> {
    let token = token(&headers)?;
    Ok(status_reply(&headers, agent.reject(&id, Some(&token), &body.name).await?))
}

async fn finish(
    State(agent): Shared,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Finish>,
) -> Result<Response, AgentError> {
    let token = token(&headers)?;
    Ok(status_reply(&headers, agent.finish(&id, Some(&token), body.save).await?))
}

async fn retry(State(agent): Shared, Path(id): Path<String>, headers: HeaderMap) -> Result<Response, AgentError> {
    let token = token(&headers)?;
    Ok(status_reply(&headers, agent.retry(&id, Some(&token)).await?))
}

async fn save_data(State(agent): Shared, Path(id): Path<String>, headers: HeaderMap) -> Result<Response, AgentError> {
    let token = token(&headers)?;
    Ok(bytes_reply(agent.save_data(&id, Some(&token)).await?))
}

async fn source(State(agent): Shared, Path(id): Path<String>, headers: HeaderMap) -> Result<Response, AgentError> {
    let token = token(&headers)?;
    Ok(bytes_reply(agent.source(&id, Some(&token)).await?))
}

async fn host_saved(
    State(agent): Shared,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<HostSaved>,
) -> Result<Response, AgentError> {
    let token = token(&headers)?;
    Ok(status_reply(&headers, agent.host_saved(&id, Some(&token), body).await?))
}
