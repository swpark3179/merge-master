//! 관리자 연결(`GET /agent/services/{id}/host`): 넘긴 호스트 앱이 그 업무를 이 PC 업무처럼 다룬다([`agent_protocol::link`]).
//!
//! 요청은 [`Agent::host_request`]가 한다(에이전트 업무의 마감 규칙, 저장을 시작하면 쓰기 막기). 세션이 바뀌면 밀어 준다:
//! 셀 변경은 바로, 접속자 표·소통·수식 계산 상태·서비스 상태는 250ms마다 바뀐 것만. 서비스가 끝나거나 에이전트를 끝내면
//! 닫는다(호스트 앱은 다시 붙어 보다가 409를 받고 멈춘다).

use std::sync::Arc;
use std::time::Duration;

use agent_protocol::link::{HostPush, LINK_PROTOCOL, LinkCall, LinkFrame, LinkMsg};
use agent_protocol::{HEADER_AGENT, ServiceStatus};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use collect_server::HostEvent;
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use serde_json::Value;
use tokio::sync::broadcast::error::RecvError;

use crate::agent::{Agent, AgentError};
use crate::api::token;
use crate::service::Service;

/// 요청 하나의 최대 크기(붙여넣기 묶음 — 2만 칸 — 이 들어가게).
const MAX_REQUEST: usize = 16 * 1024 * 1024;
/// 접속자 표·소통·상태를 밀어 주는 최소 간격.
const PUSH_EVERY: Duration = Duration::from_millis(250);

pub async fn upgrade(
    ws: WebSocketUpgrade,
    State(agent): State<Arc<Agent>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, AgentError> {
    let version = headers.get(HEADER_AGENT).and_then(|v| v.to_str().ok()).and_then(|v| v.trim().parse::<u32>().ok());
    if version.is_none_or(|v| v < LINK_PROTOCOL) {
        return Ok((
            StatusCode::UPGRADE_REQUIRED,
            axum::Json(agent_protocol::ApiError {
                message: "이 호스트 앱은 에이전트 업무를 다룰 수 없습니다. 같은 버전의 호스트 앱으로 바꿔 주세요"
                    .into(),
            }),
        )
            .into_response());
    }
    let token = token(&headers)?;
    let service = agent.owned(&id, Some(&token))?;
    agent.live_session(&service)?;
    let protocol = version.unwrap_or(LINK_PROTOCOL);
    Ok(ws.max_message_size(MAX_REQUEST).on_upgrade(move |socket| serve(socket, agent, service, protocol)))
}

fn json<T: Serialize>(v: &T) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}

async fn send(sink: &mut futures_util::stream::SplitSink<WebSocket, Message>, msg: &LinkMsg) -> Result<(), ()> {
    let text = serde_json::to_string(msg).map_err(|_| ())?;
    sink.send(Message::Text(text.into())).await.map_err(|_| ())
}

async fn push(sink: &mut futures_util::stream::SplitSink<WebSocket, Message>, push: HostPush) -> Result<(), ()> {
    send(sink, &LinkMsg::Push { push }).await
}

/// 상태가 바뀌었는지 볼 때 시각은 뺀다.
fn same_status(a: &Option<ServiceStatus>, b: &Option<ServiceStatus>) -> bool {
    let strip = |s: &Option<ServiceStatus>| s.clone().map(|s| ServiceStatus { now_ms: 0, ..s });
    strip(a) == strip(b)
}

async fn answer(agent: &Arc<Agent>, service: &Arc<Service>, frame: LinkFrame, protocol: u32) -> LinkMsg {
    let id = frame.id;
    let result = match frame.call {
        LinkCall::Host { req } => agent.host_request(service, req).await,
        LinkCall::State => agent.link_state(service).await.map(|mut s| {
            s.status = s.status.for_protocol(protocol);
            json(&s)
        }),
        LinkCall::IpNames { names } => {
            agent.set_received(&service.id, names).map(|_| Value::Null).map_err(|e| e.message)
        }
    };
    match result {
        Ok(value) => LinkMsg::Ok { id, value },
        Err(message) => LinkMsg::Err { id, message },
    }
}

async fn serve(socket: WebSocket, agent: Arc<Agent>, service: Arc<Service>, protocol: u32) {
    let Some(session) = service.session() else { return };
    tracing::info!(id = %service.id, "관리자 연결");
    let (mut sink, mut stream) = socket.split();
    let (mut snap, mut comms, mut events) = (session.subscribe(), session.subscribe_comms(), session.host_events());
    let (mut changes, mut stopping) = (agent.subscribe(), agent.stopping());
    let status_now = || agent.status_of(&service.id, None).ok().map(|s| s.for_protocol(protocol));
    let mut status = status_now();
    let mut recalc = service.recalc_status();
    let first = [
        HostPush::Snapshot(json(&*snap.borrow_and_update())),
        HostPush::Comms(json(&*comms.borrow_and_update())),
        HostPush::Recalc(recalc.clone()),
    ];
    for p in first.into_iter().chain(status.clone().map(HostPush::Status)) {
        if push(&mut sink, p).await.is_err() {
            return;
        }
    }
    let mut every = tokio::time::interval(PUSH_EVERY);
    loop {
        tokio::select! {
            incoming = stream.next() => match incoming {
                Some(Ok(Message::Text(text))) => {
                    let reply = match serde_json::from_str::<LinkFrame>(&text) {
                        Ok(frame) => answer(&agent, &service, frame, protocol).await,
                        Err(e) => {
                            let id = serde_json::from_str::<Value>(&text).ok().and_then(|v| v["id"].as_u64()).unwrap_or(0);
                            LinkMsg::Err { id, message: format!("알 수 없는 요청입니다(호스트 앱과 에이전트의 버전을 맞춰 주세요): {e}") }
                        }
                    };
                    if send(&mut sink, &reply).await.is_err() {
                        break;
                    }
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(_)) => {} // ping/pong
            },
            ev = events.recv() => {
                let ev = match ev {
                    Ok(ev) => ev,
                    // 너무 많이 밀렸다: 화면이 시트를 다시 읽게 한다.
                    Err(RecvError::Lagged(_)) => HostEvent::Reset,
                    Err(RecvError::Closed) => break,
                };
                if push(&mut sink, HostPush::Event(json(&ev))).await.is_err() {
                    break;
                }
            }
            _ = every.tick() => {
                let mut out = Vec::new();
                if snap.has_changed().unwrap_or(false) {
                    out.push(HostPush::Snapshot(json(&*snap.borrow_and_update())));
                }
                if comms.has_changed().unwrap_or(false) {
                    out.push(HostPush::Comms(json(&*comms.borrow_and_update())));
                }
                let mut ended = false;
                if changes.has_changed().unwrap_or(true) {
                    changes.mark_unchanged();
                    let now = status_now();
                    if !same_status(&now, &status) {
                        status = now;
                        out.extend(status.clone().map(HostPush::Status));
                    }
                    let r = service.recalc_status();
                    if r != recalc {
                        recalc = r;
                        out.push(HostPush::Recalc(recalc.clone()));
                    }
                    ended = service.state().finished() || service.session().is_none();
                }
                let mut failed = false;
                for p in out {
                    failed = failed || push(&mut sink, p).await.is_err();
                }
                if failed {
                    break;
                }
                if ended {
                    let _ = sink.send(Message::Close(None)).await;
                    break;
                }
            }
            // 에이전트를 끝낸다(트레이 '끝내기'): 연결이 남아 있어도 기다리지 않는다.
            () = async { let _ = stopping.wait_for(|s| *s).await; } => {
                let _ = sink.send(Message::Close(None)).await;
                break;
            }
        }
    }
    tracing::info!(id = %service.id, "관리자 연결 종료");
}
