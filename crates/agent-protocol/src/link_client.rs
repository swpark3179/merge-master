//! 호스트 앱이 에이전트에 여는 관리자 연결([`crate::link`]). 끊기면 1·2·4·8·15초 뒤 다시 붙고, 에이전트가 거절하면
//! (관리 키·없는 서비스·서비스 중 아님·다른 버전) 멈춘다. 15초마다 ping을 보내 45초 동안 아무것도 오지 않으면 끊는다.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::sync::{mpsc, oneshot, watch};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};

use crate::client::{AgentClient, ClientError};
use crate::link::{HostPush, LinkCall, LinkFrame, LinkMsg, MAX_LINK_MESSAGE};
use crate::{ApiError, HEADER_AGENT, HEADER_TOKEN, PROTOCOL};

/// 연결을 기다리는 시간.
const CONNECT: Duration = Duration::from_secs(5);
/// 요청 하나의 답을 기다리는 시간.
const CALL: Duration = Duration::from_secs(30);
const PING: Duration = Duration::from_secs(15);
/// 이만큼 아무것도 오지 않으면 끊긴 것으로 본다.
const SILENT: Duration = Duration::from_secs(45);
/// 다시 붙기 전에 기다리는 초(마지막 것을 되풀이).
const BACKOFF: [u64; 5] = [1, 2, 4, 8, 15];

pub const NOT_CONNECTED: &str = "에이전트에 연결되어 있지 않습니다(다시 연결하는 중)";

#[derive(Debug, Clone, PartialEq)]
pub enum LinkStatus {
    Connecting,
    Connected,
    /// 다시 붙지 않는다(까닭).
    Stopped(String),
}

/// 연결이 알리는 것(연결 작업 안에서 부른다 — 오래 걸리는 일은 따로 돌린다).
#[derive(Debug, Clone)]
pub enum LinkEvent {
    Connected,
    Disconnected(String),
    Push(HostPush),
    Stopped(String),
}

type OnEvent = Arc<dyn Fn(LinkEvent) + Send + Sync>;
type Waiting = Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>;

struct Inner {
    /// 지금 연결의 보낼 줄(끊겨 있으면 없음).
    tx: Mutex<Option<mpsc::UnboundedSender<String>>>,
    waiting: Waiting,
    next: AtomicU64,
    status: watch::Sender<LinkStatus>,
    stop: watch::Sender<bool>,
}

impl Inner {
    fn answer(&self, id: u64, result: Result<Value, String>) {
        if let Some(tx) = self.waiting.lock().unwrap().remove(&id) {
            let _ = tx.send(result);
        }
    }

    fn fail_all(&self, why: &str) {
        for (_, tx) in self.waiting.lock().unwrap().drain() {
            let _ = tx.send(Err(why.to_string()));
        }
    }
}

/// 서비스 하나의 관리자 연결. 버리면 닫는다.
pub struct HostLink {
    inner: Arc<Inner>,
}

impl HostLink {
    /// 연결을 시작한다(tokio 런타임 안에서). `on_event`는 붙음·끊김·밀어 준 것마다 불린다.
    pub fn start(
        client: &AgentClient,
        id: &str,
        token: &str,
        on_event: impl Fn(LinkEvent) + Send + Sync + 'static,
    ) -> HostLink {
        let inner = Arc::new(Inner {
            tx: Mutex::new(None),
            waiting: Mutex::new(HashMap::new()),
            next: AtomicU64::new(1),
            status: watch::channel(LinkStatus::Connecting).0,
            stop: watch::channel(false).0,
        });
        tokio::spawn(run(inner.clone(), client.link_url(id), token.to_string(), Arc::new(on_event)));
        HostLink { inner }
    }

    pub fn status(&self) -> LinkStatus {
        self.inner.status.borrow().clone()
    }

    pub fn subscribe(&self) -> watch::Receiver<LinkStatus> {
        self.inner.status.subscribe()
    }

    pub fn connected(&self) -> bool {
        self.status() == LinkStatus::Connected
    }

    /// 요청 하나를 보내고 답을 기다린다(30초).
    pub async fn call(&self, call: LinkCall) -> Result<Value, String> {
        let id = self.inner.next.fetch_add(1, Ordering::Relaxed);
        let text = serde_json::to_string(&LinkFrame { id, call }).map_err(|e| e.to_string())?;
        let (tx, rx) = oneshot::channel();
        self.inner.waiting.lock().unwrap().insert(id, tx);
        let sent = self.inner.tx.lock().unwrap().as_ref().is_some_and(|t| t.send(text).is_ok());
        if !sent {
            self.inner.waiting.lock().unwrap().remove(&id);
            return Err(match self.status() {
                LinkStatus::Stopped(why) => why,
                _ => NOT_CONNECTED.to_string(),
            });
        }
        match tokio::time::timeout(CALL, rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err("에이전트와의 연결이 끊겼습니다".into()),
            Err(_) => {
                self.inner.waiting.lock().unwrap().remove(&id);
                Err("에이전트가 응답하지 않습니다(시간 초과)".into())
            }
        }
    }

    /// 닫는다(다시 붙지 않는다).
    pub fn close(&self) {
        self.inner.stop.send_replace(true);
    }
}

impl Drop for HostLink {
    fn drop(&mut self) {
        self.close();
    }
}

enum Fail {
    /// 다시 붙어 본다(닿지 않음 등).
    Retry(String),
    /// 에이전트가 거절했다(다시 붙어도 같다).
    Fatal(String),
}

type Socket = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn connect(url: &str, token: &str) -> Result<Socket, Fail> {
    let mut req = url.into_client_request().map_err(|e| Fail::Fatal(e.to_string()))?;
    let headers = req.headers_mut();
    headers.insert(HEADER_AGENT, HeaderValue::from(PROTOCOL));
    headers.insert(
        HEADER_TOKEN,
        HeaderValue::from_str(token).map_err(|_| Fail::Fatal("관리 키가 잘못되었습니다".into()))?,
    );
    let config =
        WebSocketConfig::default().max_message_size(Some(MAX_LINK_MESSAGE)).max_frame_size(Some(MAX_LINK_MESSAGE));
    match tokio::time::timeout(CONNECT, tokio_tungstenite::connect_async_with_config(req, Some(config), true)).await {
        Err(_) => Err(Fail::Retry("응답이 없습니다(시간 초과)".into())),
        Ok(Ok((socket, _))) => Ok(socket),
        Ok(Err(WsError::Http(resp))) => {
            let status = resp.status().as_u16();
            let message = resp
                .body()
                .as_deref()
                .and_then(|b| serde_json::from_slice::<ApiError>(b).ok())
                .map(|e| e.message)
                .unwrap_or_else(|| format!("에이전트 오류({status})"));
            let text = ClientError::Api { status, message }.to_string();
            if matches!(status, 400 | 403 | 404 | 409 | 426) { Err(Fail::Fatal(text)) } else { Err(Fail::Retry(text)) }
        }
        Ok(Err(e)) => Err(Fail::Retry(e.to_string())),
    }
}

async fn run(inner: Arc<Inner>, url: String, token: String, on_event: OnEvent) {
    let mut stop = inner.stop.subscribe();
    let mut tries = 0usize;
    loop {
        if *stop.borrow() {
            break;
        }
        inner.status.send_replace(LinkStatus::Connecting);
        let attempt = tokio::select! {
            r = connect(&url, &token) => r,
            () = async { let _ = stop.wait_for(|s| *s).await; } => break,
        };
        match attempt {
            Ok(socket) => {
                tries = 0;
                let (tx, rx) = mpsc::unbounded_channel();
                *inner.tx.lock().unwrap() = Some(tx);
                inner.status.send_replace(LinkStatus::Connected);
                on_event(LinkEvent::Connected);
                let why = serve(&inner, socket, rx, &on_event, &mut stop).await;
                inner.tx.lock().unwrap().take();
                inner.fail_all("에이전트와의 연결이 끊겼습니다");
                if *stop.borrow() {
                    break;
                }
                inner.status.send_replace(LinkStatus::Connecting);
                on_event(LinkEvent::Disconnected(why));
            }
            Err(Fail::Fatal(why)) => {
                inner.status.send_replace(LinkStatus::Stopped(why.clone()));
                on_event(LinkEvent::Stopped(why));
                return;
            }
            Err(Fail::Retry(why)) => on_event(LinkEvent::Disconnected(why)),
        }
        let wait = Duration::from_secs(BACKOFF[tries.min(BACKOFF.len() - 1)]);
        tries += 1;
        tokio::select! {
            () = tokio::time::sleep(wait) => {}
            () = async { let _ = stop.wait_for(|s| *s).await; } => break,
        }
    }
    inner.fail_all("연결을 닫았습니다");
    inner.status.send_replace(LinkStatus::Stopped("연결을 닫았습니다".into()));
}

/// 붙어 있는 동안: 받은 답은 기다리는 요청에, 밀어 준 것은 `on_event`로. 끊긴 까닭을 돌려준다.
async fn serve(
    inner: &Inner,
    socket: Socket,
    mut rx: mpsc::UnboundedReceiver<String>,
    on_event: &OnEvent,
    stop: &mut watch::Receiver<bool>,
) -> String {
    let (mut sink, mut stream) = socket.split();
    let mut ping = tokio::time::interval(PING);
    ping.tick().await;
    let mut heard = Instant::now();
    loop {
        tokio::select! {
            incoming = stream.next() => {
                heard = Instant::now();
                match incoming {
                    Some(Ok(Message::Text(text))) => match serde_json::from_str::<LinkMsg>(&text) {
                        Ok(LinkMsg::Ok { id, value }) => inner.answer(id, Ok(value)),
                        Ok(LinkMsg::Err { id, message }) => inner.answer(id, Err(message)),
                        Ok(LinkMsg::Push { push }) => on_event(LinkEvent::Push(push)),
                        Err(_) => {} // 모르는 메시지(새 에이전트)는 건너뛴다
                    },
                    Some(Ok(Message::Close(_))) | None => return "에이전트가 연결을 닫았습니다".into(),
                    Some(Err(e)) => return e.to_string(),
                    Some(Ok(_)) => {} // pong 등
                }
            }
            out = rx.recv() => match out {
                Some(text) => {
                    if let Err(e) = sink.send(Message::Text(text.into())).await {
                        return e.to_string();
                    }
                }
                None => return "연결을 닫았습니다".into(),
            },
            _ = ping.tick() => {
                if heard.elapsed() > SILENT {
                    return "에이전트가 응답하지 않습니다".into();
                }
                if let Err(e) = sink.send(Message::Ping(Default::default())).await {
                    return e.to_string();
                }
            }
            () = async { let _ = stop.wait_for(|s| *s).await; } => {
                let _ = sink.send(Message::Close(None)).await;
                return "연결을 닫았습니다".into();
            }
        }
    }
}
