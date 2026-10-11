//! 연결 하나를 맡는 작업: 받은 메시지는 세션 액터로 넘기고, 액터가 보낸 메시지를 소켓에 쓴다.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;

use crate::guard::{Bucket, MAX_DROPPED, origin_allowed};
use crate::hub::{Gone, Target};
use crate::protocol::{ClientMsg, ServerMsg};
use crate::session::{Command, Outgoing};
use crate::{AppState, Pick};

/// 붙여넣기 묶음(최대 2만 칸)이 들어가는 크기.
const MAX_MESSAGE_BYTES: usize = 4 * 1024 * 1024;

/// `GET /ws?s=<주소 경로>`: 그 업무의 세션에 붙는다. 공개 중인 업무가 아니면 업그레이드하지 않고 404(브라우저는 조금 뒤 다시
/// 붙어 본다 — 호스트가 다시 공개하면 이어진다).
pub async fn upgrade(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Query(pick): Query<Pick>,
    headers: HeaderMap,
) -> Response {
    // 다른 웹사이트에서 열린 페이지가 참여자 브라우저를 통해 붙는 것을 막는다.
    if !origin_allowed(&headers) {
        tracing::warn!(%peer, origin = ?headers.get("origin"), "다른 출처의 WebSocket 거절");
        return (StatusCode::FORBIDDEN, "다른 사이트에서 열린 페이지는 연결할 수 없습니다").into_response();
    }
    let target = match state.hub.resolve(pick.s.as_deref()) {
        Ok(t) => t,
        Err(e) => return e.into_response(),
    };
    ws.max_message_size(MAX_MESSAGE_BYTES).on_upgrade(move |socket| handle(socket, state, target, peer))
}

fn encode(msg: &ServerMsg) -> Message {
    Message::Text(serde_json::to_string(msg).expect("ServerMsg 직렬화").into())
}

async fn handle(socket: WebSocket, state: Arc<AppState>, target: Target, peer: SocketAddr) {
    let Target { session, mut gone } = target;
    let conn = state.next_conn.fetch_add(1, Ordering::Relaxed);
    let (outbox, mut inbox) = mpsc::unbounded_channel();
    session.send(Command::Connect { conn, ip: peer.ip(), outbox });
    tracing::info!(conn, %peer, "연결");

    let (mut sink, mut stream) = socket.split();
    let mut shutdown = state.shutdown.clone();
    let mut bucket = Bucket::default();
    let mut warned = false;
    loop {
        tokio::select! {
            incoming = stream.next() => match incoming {
                // 너무 빨리 보내면 버리고 한 번 알린다. 계속 쏟아부으면 끊는다.
                Some(Ok(Message::Text(_))) if !bucket.take(std::time::Instant::now()) => {
                    if bucket.dropped >= MAX_DROPPED {
                        tracing::warn!(conn, %peer, "메시지를 너무 많이 보내 연결을 끊습니다");
                        let _ = sink.send(Message::Close(None)).await;
                        break;
                    }
                    if !warned {
                        warned = true;
                        let err = ServerMsg::Error { message: "메시지를 너무 빨리 보내고 있습니다. 일부는 버렸습니다".into() };
                        if sink.send(encode(&err)).await.is_err() {
                            break;
                        }
                    }
                }
                Some(Ok(Message::Text(text))) => match serde_json::from_str::<ClientMsg>(&text) {
                    Ok(msg) => session.send(Command::Client { conn, msg }),
                    Err(e) => {
                        let err = ServerMsg::Error { message: format!("알 수 없는 메시지: {e}") };
                        if sink.send(encode(&err)).await.is_err() {
                            break;
                        }
                    }
                },
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(_)) => {} // ping/pong/binary
            },
            out = inbox.recv() => match out {
                Some(Outgoing::Msg(msg)) => {
                    if sink.send(encode(&msg)).await.is_err() {
                        break;
                    }
                }
                Some(Outgoing::Close) | None => {
                    let _ = sink.send(Message::Close(None)).await;
                    break;
                }
            },
            // wait_for가 돌려주는 읽기 가드는 Send가 아니라서 블록 안에서 바로 버린다.
            () = async { let _ = shutdown.wait_for(|stop| *stop).await; } => {
                let _ = sink.send(encode(&ServerMsg::Closing { ended: false })).await;
                let _ = sink.send(Message::Close(None)).await;
                break;
            }
            // 이 업무만 공개를 멈췄다(다른 업무의 연결은 그대로).
            ended = async { gone.wait_for(Option::is_some).await.map(|g| *g == Some(Gone::Ended)).unwrap_or(false) } => {
                let _ = sink.send(encode(&ServerMsg::Closing { ended })).await;
                let _ = sink.send(Message::Close(None)).await;
                break;
            }
        }
    }
    session.send(Command::Disconnect { conn });
    tracing::info!(conn, "연결 종료");
}
