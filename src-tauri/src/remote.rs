//! 에이전트로 넘긴 업무의 관리자 연결([`HostLink`]): 서비스 중인 에이전트 업무마다 하나를 열어 두고, 그 업무의 command를
//! 이 연결로 보낸다([`AppState::target`] — 업무 id 자리에 서비스 id). 에이전트가 밀어 준 것은 이 PC 업무와 같은 이름의
//! 이벤트로 웹뷰에 보낸다(`participants:{id}`, `comms:{id}`, `sheet-changes:{id}`, `recalc-status:{id}`). 그래서 설정·
//! 모니터링·소통·히스토리·참여자 화면은 이 PC 업무와 똑같이 쓴다(호스트는 늘 관리자).
//!
//! - 연결은 에이전트 상태를 묻는 3초 고리([`crate::agent::refresh`])가 맞춘다: 서비스 중이면 열고, 끝났거나 찾지 못하면 닫는다.
//! - 다시 붙으면 화면이 다시 읽게 하고(`reset`) 이 PC의 IP 명단을 보낸다(그 서비스의 넘겨받은 명단 — 에이전트 PC 명단이 이긴다).
//! - 이 PC에서만 되는 일(Excel 불러오기·저장, 공유 시작·중지, 넘기기, 업무 끝내기)은 [`NOT_HERE`]로 거절한다.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use agent_protocol::client::AgentClient;
use agent_protocol::link::{HostPush, LinkCall, LinkState};
use agent_protocol::link_client::{HostLink, LinkEvent, LinkStatus};
use collect_server::host::HostRequest;
use collect_server::{HostEvent, IpNames, RecalcStatus, SessionHandle};
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager};

use crate::agent::AgentRef;
use crate::jobs::event;
use crate::{AppState, CmdResult};

pub(crate) const NOT_HERE: &str = "에이전트에서 진행 중인 업무라 이 PC에서는 할 수 없어요";
/// 에이전트가 거절해 멈춘 연결을 다시 열어 보는 간격(에이전트를 다시 켜는 동안 잠깐 '없는 서비스'일 수 있다).
const RETRY_STOPPED: Duration = Duration::from_secs(30);

#[derive(Default)]
struct Cache {
    recalc: RecalcStatus,
    /// 참여자가 보낸 마지막 채팅 번호(처음 받기 전이면 없음 — 그때는 깜빡이지 않는다).
    last_chat: Option<u32>,
}

/// 에이전트 업무 하나의 연결.
pub(crate) struct Remote {
    link: HostLink,
    cache: Arc<Mutex<Cache>>,
    started: Instant,
}

impl Remote {
    fn start(app: &AppHandle, client: &AgentClient, r: &AgentRef) -> Remote {
        let cache = Arc::new(Mutex::new(Cache::default()));
        let (app, id, seen) = (app.clone(), r.id.clone(), cache.clone());
        let link = HostLink::start(client, &r.id, &r.token, move |e| on_event(&app, &id, &seen, e));
        Remote { link, cache, started: Instant::now() }
    }

    /// 업무 command 하나.
    pub(crate) async fn host(&self, req: HostRequest) -> CmdResult<Value> {
        self.link.call(LinkCall::Host { req }).await
    }

    /// 업무를 처음 그릴 것.
    pub(crate) async fn state(&self) -> CmdResult<LinkState> {
        serde_json::from_value(self.link.call(LinkCall::State).await?).map_err(|e| e.to_string())
    }

    pub(crate) fn recalc(&self) -> RecalcStatus {
        self.cache.lock().unwrap().recalc.clone()
    }

    pub(crate) fn linked(&self) -> bool {
        self.link.connected()
    }

    /// 다시 붙지 않는 까닭(에이전트가 거절함).
    pub(crate) fn stopped(&self) -> Option<String> {
        match self.link.status() {
            LinkStatus::Stopped(why) => Some(why),
            _ => None,
        }
    }

    pub(crate) fn last_chat(&self) -> u32 {
        self.cache.lock().unwrap().last_chat.unwrap_or(0)
    }

    /// 이 PC의 IP 명단을 보낸다.
    async fn send_ip_names(&self, names: &IpNames) {
        let names: BTreeMap<String, String> = names.entries().into_iter().map(|e| (e.ip, e.name)).collect();
        if let Err(e) = self.link.call(LinkCall::IpNames { names }).await {
            tracing::warn!("에이전트에 IP 명단을 보내지 못했습니다: {e}");
        }
    }
}

#[derive(Default)]
pub(crate) struct Remotes {
    map: Mutex<HashMap<String, Arc<Remote>>>,
}

impl Remotes {
    pub(crate) fn get(&self, id: &str) -> Option<Arc<Remote>> {
        self.map.lock().unwrap().get(id).cloned()
    }

    /// 그 연결을 닫는다(목록에서 지움).
    pub(crate) fn remove(&self, id: &str) {
        self.map.lock().unwrap().remove(id);
    }

    fn all(&self) -> Vec<Arc<Remote>> {
        self.map.lock().unwrap().values().cloned().collect()
    }
}

/// 업무 command를 보낼 곳.
pub(crate) enum Target {
    Local(SessionHandle),
    Remote(Arc<Remote>),
}

impl AppState {
    /// 그 업무(이 PC) 또는 에이전트 업무(서비스 id).
    pub(crate) fn target(&self, id: &str) -> CmdResult<Target> {
        match self.job(id) {
            Ok(job) => Ok(Target::Local(job.session()?)),
            Err(e) => self.remotes.get(id).map(Target::Remote).ok_or(e),
        }
    }
}

/// 에이전트 상태를 받은 뒤: 서비스 중인 에이전트 업무는 연결을 열어 두고, 그 밖의 연결은 닫는다(버리면 닫힌다).
/// 연결을 여는 데 tokio 런타임이 있어야 한다(비동기 command·상태 고리에서 부른다).
pub(crate) fn sync(app: &AppHandle) {
    let state = app.state::<AppState>();
    let wanted = state.agents.linkable();
    let mut map = state.remotes.map.lock().unwrap();
    map.retain(|id, remote| {
        wanted.iter().any(|r| r.id == *id) && (remote.stopped().is_none() || remote.started.elapsed() < RETRY_STOPPED)
    });
    for r in wanted {
        if map.contains_key(&r.id) {
            continue;
        }
        match AgentClient::new(&r.addr) {
            Ok(client) => {
                map.insert(r.id.clone(), Arc::new(Remote::start(app, &client, &r)));
            }
            Err(e) => tracing::warn!("에이전트 주소가 잘못되었습니다({}): {e}", r.addr),
        }
    }
}

/// 이 PC의 IP 명단이 바뀌었다: 열린 연결마다 보낸다.
pub(crate) fn send_ip_names(state: &AppState, names: &IpNames) {
    for remote in state.remotes.all() {
        let names = names.clone();
        tauri::async_runtime::spawn(async move { remote.send_ip_names(&names).await });
    }
}

fn on_event(app: &AppHandle, id: &str, cache: &Mutex<Cache>, e: LinkEvent) {
    match e {
        LinkEvent::Connected => {
            let (app, id) = (app.clone(), id.to_string());
            tauri::async_runtime::spawn(async move {
                let state = app.state::<AppState>();
                if let Some(remote) = state.remotes.get(&id) {
                    let names = state.ip_names.lock().unwrap().clone();
                    remote.send_ip_names(&names).await;
                }
                // 끊긴 사이 바뀐 것을 화면이 다시 읽는다.
                let _ = app.emit(&event("sheet-changes", &id), HostEvent::Reset);
                crate::agent::emit(&app);
            });
        }
        LinkEvent::Disconnected(why) => {
            tracing::info!(id, "에이전트 관리자 연결이 끊겼습니다: {why}");
            crate::agent::emit(app);
        }
        LinkEvent::Stopped(why) => {
            tracing::warn!(id, "에이전트 관리자 연결을 멈춥니다: {why}");
            crate::agent::emit(app);
        }
        LinkEvent::Push(HostPush::Snapshot(v)) => {
            let _ = app.emit(&event("participants", id), v);
        }
        LinkEvent::Push(HostPush::Comms(v)) => {
            let newest = v["chat"]
                .as_array()
                .map(|chat| {
                    chat.iter().filter(|m| m["host"] != true).filter_map(|m| m["id"].as_u64()).max().unwrap_or(0)
                })
                .unwrap_or(0) as u32;
            let before = cache.lock().unwrap().last_chat.replace(newest);
            if before.is_some_and(|b| newest > b) {
                crate::monitor::request_attention(app);
            }
            let _ = app.emit(&event("comms", id), v);
            if before != Some(newest) {
                crate::agent::emit(app);
            }
        }
        LinkEvent::Push(HostPush::Event(v)) => {
            let _ = app.emit(&event("sheet-changes", id), v);
        }
        LinkEvent::Push(HostPush::Recalc(r)) => {
            cache.lock().unwrap().recalc = r.clone();
            let _ = app.emit(&event("recalc-status", id), r);
        }
        LinkEvent::Push(HostPush::Status(s)) => {
            app.state::<AppState>().agents.record_status(id, s);
            crate::agent::emit(app);
        }
    }
}
