//! 연결 하나를 맡는 작업: 받은 메시지는 세션 액터로 넘기고, 액터가 보낸 메시지를 소켓에 쓴다.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;

use crate::AppState;
use crate::guard::{Bucket, MAX_DROPPED, origin_allowed};
use crate::protocol::{ClientMsg, ServerMsg};
use crate::session::{Command, Outgoing};

/// 붙여넣기 묶음(최대 2만 칸)이 들어가는 크기.
const MAX_MESSAGE_BYTES: usize = 4 * 1024 * 1024;

pub async fn upgrade(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    // 다른 웹사이트에서 열린 페이지가 참여자 브라우저를 통해 붙는 것을 막는다.
    if !origin_allowed(&headers) {
        tracing::warn!(%peer, origin = ?headers.get("origin"), "다른 출처의 WebSocket 거절");
        return (StatusCode::FORBIDDEN, "다른 사이트에서 열린 페이지는 연결할 수 없습니다").into_response();
    }
    ws.max_message_size(MAX_MESSAGE_BYTES).on_upgrade(move |socket| handle(socket, state, peer))
}

fn encode(msg: &ServerMsg) -> Message {
    Message::Text(serde_json::to_string(msg).expect("ServerMsg 직렬화").into())
}

async fn handle(socket: WebSocket, state: Arc<AppState>, peer: SocketAddr) {
    let conn = state.next_conn.fetch_add(1, Ordering::Relaxed);
    let (outbox, mut inbox) = mpsc::unbounded_channel();
    state.session.send(Command::Connect { conn, ip: peer.ip(), outbox });
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
                    Ok(msg) => state.session.send(Command::Client { conn, msg }),
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
                let _ = sink.send(encode(&ServerMsg::Closing)).await;
                let _ = sink.send(Message::Close(None)).await;
                break;
            }
        }
    }
    state.session.send(Command::Disconnect { conn });
    tracing::info!(conn, "연결 종료");
}
