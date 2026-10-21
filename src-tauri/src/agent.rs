//! 에이전트 PC로 넘긴 서비스(사이드바 '에이전트에서 진행 중'): 연결 확인·폴더 조회·상태 보기·끝내기.
//!
//! 웹뷰는 다른 PC로 바로 요청할 수 없어(CSP) 여기서 보낸다([`agent_protocol::client`]). 넘긴 서비스(에이전트 주소·서비스
//! id·관리 키)는 앱 설정 폴더의 `agents.json`에 기억하고, 3초마다 상태를 물어 `agent-services` 이벤트로 보낸다.
//! 넘기다 응답을 놓쳤으면(`pending`) 에이전트에 물어 받았으면 이 PC의 업무를 끝내고, 받지 않았으면 기억을 지운다.
//! 서비스 중인 것은 관리자 연결을 열어 이 PC 업무처럼 다룬다([`crate::remote`] — 설정·모니터링·소통·참여자 승인).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use agent_protocol::client::{AgentClient, ClientError};
use agent_protocol::{AgentInfo, FolderList, ServiceState, ServiceStatus, StatusQuery};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use ts_rs::TS;

use crate::remote::Remotes;
use crate::{AppState, CmdResult};

const FILE: &str = "agents.json";
const POLL: Duration = Duration::from_secs(3);
pub(crate) const EVENT: &str = "agent-services";
/// 넘긴 뒤 이 PC 기록 파일에 남기는 메타(넘긴 곳).
pub(crate) const META_HANDED_OFF: &str = "handedOff";

/// 넘긴 서비스 하나(기억).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentRef {
    pub addr: String,
    pub id: String,
    pub token: String,
    pub job_id: String,
    pub title: String,
    pub workbook: String,
    pub agent_name: String,
    pub created_at_ms: i64,
    /// 넘기는 중(에이전트가 받았는지 아직 모른다).
    #[serde(default)]
    pub pending: bool,
    /// 이 PC의 업무를 끝냈다.
    #[serde(default)]
    pub local_ended: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct AgentsFile {
    last_addr: Option<String>,
    services: Vec<AgentRef>,
}

/// 사이드바·에이전트 서비스 화면에 보일 한 줄.
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AgentServiceView {
    pub id: String,
    /// 에이전트 주소(`http://PC:8090`).
    pub addr: String,
    pub agent_name: String,
    pub title: String,
    pub workbook: String,
    /// 마지막으로 받은 상태(에이전트에 닿지 않으면 그 전 것).
    #[ts(optional)]
    pub status: Option<ServiceStatus>,
    /// 지금 에이전트에 닿는다.
    pub reachable: bool,
    /// 닿지 않거나 찾지 못한 까닭.
    #[ts(optional)]
    pub problem: Option<String>,
    /// 넘기는 중(받았는지 확인하는 중).
    pub pending: bool,
    /// 관리자 연결이 열려 있다(설정·모니터링·소통을 이 PC 업무처럼 다룬다).
    pub linked: bool,
    /// 업무 화면으로 다룬다(서비스 중이고 관리자 연결을 두었다 — 잠시 끊겨도).
    pub live: bool,
    /// 참여자가 보낸 마지막 채팅 번호(안 읽은 채팅 표시, 없으면 0).
    pub last_chat: u32,
    #[ts(type = "number")]
    pub created_at_ms: i64,
}

/// 연결 확인 결과.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AgentConnection {
    /// 정리한 주소(`http://PC:8090`).
    pub addr: String,
    pub info: AgentInfo,
    /// 에이전트 시계 − 이 PC 시계(ms).
    #[ts(type = "number")]
    pub clock_skew_ms: i64,
    /// 이 앱 버전(에이전트와 다르면 알린다).
    pub app_version: String,
}

#[derive(Default)]
struct Last {
    status: Option<ServiceStatus>,
    reachable: bool,
    problem: Option<String>,
    /// 에이전트에 없거나 관리 키가 맞지 않는다.
    gone: bool,
}

#[derive(Default)]
pub(crate) struct Agents {
    file: Mutex<Option<PathBuf>>,
    data: Mutex<AgentsFile>,
    last: Mutex<HashMap<String, Last>>,
    views: Mutex<Vec<AgentServiceView>>,
    /// 지금 넘기는 중인 서비스(넘기기가 끝날 때까지 정리하지 않는다).
    busy: Mutex<HashSet<String>>,
}

impl Agents {
    /// 앱을 켤 때 기억을 읽는다.
    pub(crate) fn load(&self, app: &AppHandle) {
        let Ok(dir) = app.path().app_local_data_dir() else { return };
        let path = dir.join(FILE);
        if let Ok(bytes) = std::fs::read(&path) {
            match serde_json::from_slice::<AgentsFile>(&bytes) {
                Ok(f) => *self.data.lock().unwrap() = f,
                Err(e) => tracing::warn!("에이전트 기억을 읽을 수 없습니다({}): {e}", path.display()),
            }
        }
        *self.file.lock().unwrap() = Some(path);
        *self.views.lock().unwrap() = self.build(&Remotes::default());
    }

    fn save(&self) {
        let Some(path) = self.file.lock().unwrap().clone() else { return };
        let text = serde_json::to_vec_pretty(&*self.data.lock().unwrap()).expect("직렬화");
        let write = || -> std::io::Result<()> {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let tmp = path.with_extension("tmp");
            std::fs::write(&tmp, text)?;
            std::fs::rename(&tmp, &path)
        };
        if let Err(e) = write() {
            tracing::error!("에이전트 기억을 남길 수 없습니다: {e}");
        }
    }

    fn update(&self, f: impl FnOnce(&mut AgentsFile)) {
        f(&mut self.data.lock().unwrap());
        self.save();
    }

    pub(crate) fn add(&self, r: AgentRef) {
        self.update(|d| {
            d.services.retain(|x| x.id != r.id);
            d.services.push(r);
        });
    }

    pub(crate) fn set(&self, id: &str, f: impl FnOnce(&mut AgentRef)) {
        self.update(|d| {
            if let Some(r) = d.services.iter_mut().find(|r| r.id == id) {
                f(r);
            }
        });
    }

    pub(crate) fn remove(&self, id: &str) {
        self.update(|d| d.services.retain(|r| r.id != id));
        self.last.lock().unwrap().remove(id);
    }

    fn refs(&self) -> Vec<AgentRef> {
        self.data.lock().unwrap().services.clone()
    }

    pub(crate) fn find(&self, id: &str) -> CmdResult<AgentRef> {
        self.refs().into_iter().find(|r| r.id == id).ok_or_else(|| "기억하지 않는 에이전트 서비스입니다".into())
    }

    /// 넘기는 동안 표시해 둔다(돌려준 것을 버리면 풀린다).
    pub(crate) fn busy(&self, id: &str) -> Busy<'_> {
        self.busy.lock().unwrap().insert(id.to_string());
        Busy { agents: self, id: id.to_string() }
    }

    pub(crate) fn remember_addr(&self, addr: &str) {
        let addr = addr.to_string();
        self.update(|d| d.last_addr = Some(addr));
    }

    pub(crate) fn record_status(&self, id: &str, status: ServiceStatus) {
        self.last
            .lock()
            .unwrap()
            .insert(id.to_string(), Last { status: Some(status), reachable: true, problem: None, gone: false });
    }

    /// 관리자 연결을 둘 서비스: 받았고, 서비스 중이고, 에이전트가 아직 그 서비스를 안다.
    pub(crate) fn linkable(&self) -> Vec<AgentRef> {
        let last = self.last.lock().unwrap();
        self.refs()
            .into_iter()
            .filter(|r| {
                !r.pending
                    && last
                        .get(&r.id)
                        .is_some_and(|l| !l.gone && l.status.as_ref().is_some_and(|s| s.state == ServiceState::Running))
            })
            .collect()
    }

    fn build(&self, remotes: &Remotes) -> Vec<AgentServiceView> {
        let last = self.last.lock().unwrap();
        let mut out: Vec<AgentServiceView> = self
            .refs()
            .into_iter()
            .map(|r| {
                let l = last.get(&r.id);
                let remote = remotes.get(&r.id);
                let running = l.and_then(|l| l.status.as_ref()).is_some_and(|s| s.state == ServiceState::Running);
                AgentServiceView {
                    id: r.id.clone(),
                    addr: r.addr.clone(),
                    agent_name: r.agent_name.clone(),
                    title: r.title.clone(),
                    workbook: r.workbook.clone(),
                    status: l.and_then(|l| l.status.clone()),
                    reachable: l.is_some_and(|l| l.reachable),
                    problem: match l {
                        Some(l) => l.problem.clone().or_else(|| remote.as_ref().and_then(|x| x.stopped())),
                        None => Some("상태를 확인하는 중".into()),
                    },
                    pending: r.pending,
                    linked: remote.as_ref().is_some_and(|x| x.linked()),
                    live: running && remote.as_ref().is_some_and(|x| x.stopped().is_none()),
                    last_chat: remote.as_ref().map_or(0, |x| x.last_chat()),
                    created_at_ms: r.created_at_ms,
                }
            })
            .collect();
        out.sort_by_key(|v| std::cmp::Reverse(v.created_at_ms));
        out
    }

    fn views(&self) -> Vec<AgentServiceView> {
        self.views.lock().unwrap().clone()
    }

    pub(crate) fn view(&self, id: &str, remotes: &Remotes) -> Option<AgentServiceView> {
        self.build(remotes).into_iter().find(|v| v.id == id)
    }
}

pub(crate) struct Busy<'a> {
    agents: &'a Agents,
    id: String,
}

impl Drop for Busy<'_> {
    fn drop(&mut self) {
        self.agents.busy.lock().unwrap().remove(&self.id);
    }
}

/// 3초마다 상태를 묻는다(앱이 끝날 때까지).
pub(crate) fn spawn_poller(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut every = tokio::time::interval(POLL);
        loop {
            every.tick().await;
            refresh(&app).await;
        }
    });
}

/// 모든 에이전트에 상태를 묻고(에이전트마다 한 번), 넘기다 만 것을 정리하고, 화면에 보낸다.
pub(crate) async fn refresh(app: &AppHandle) {
    let state = app.state::<AppState>();
    let busy = state.agents.busy.lock().unwrap().clone();
    let refs: Vec<AgentRef> = state.agents.refs().into_iter().filter(|r| !busy.contains(&r.id)).collect();
    let mut by_addr: BTreeMap<String, Vec<AgentRef>> = BTreeMap::new();
    for r in refs {
        by_addr.entry(r.addr.clone()).or_default().push(r);
    }
    for (addr, refs) in by_addr {
        let Ok(client) = AgentClient::new(&addr) else { continue };
        let queries: Vec<StatusQuery> =
            refs.iter().map(|r| StatusQuery { id: r.id.clone(), token: r.token.clone() }).collect();
        match client.status(&queries).await {
            Ok(items) => {
                for item in items {
                    let Some(r) = refs.iter().find(|r| r.id == item.id) else { continue };
                    if item.missing && r.pending {
                        // 넘기다 응답을 놓쳤는데 에이전트가 받지 않았다: 이 PC의 업무는 그대로 둔다.
                        tracing::info!(id = %r.id, "에이전트가 받지 않은 넘기기를 지웁니다");
                        state.agents.remove(&r.id);
                        continue;
                    }
                    let gone = item.missing || item.forbidden;
                    let problem = if item.missing {
                        Some("에이전트에 없는 서비스입니다(끝난 지 오래되어 지웠거나 다른 에이전트)".into())
                    } else if item.forbidden {
                        Some("관리 키가 맞지 않습니다".into())
                    } else {
                        None
                    };
                    let found = item.status.is_some();
                    {
                        let mut last = state.agents.last.lock().unwrap();
                        let entry = last.entry(r.id.clone()).or_default();
                        entry.reachable = true;
                        entry.problem = problem;
                        entry.gone = gone;
                        if item.status.is_some() {
                            entry.status = item.status;
                        }
                    }
                    if found && (r.pending || !r.local_ended) {
                        state.agents.set(&r.id, |x| x.pending = false);
                        end_local_job(app, &state, r).await;
                    }
                }
            }
            Err(e) => {
                let problem = match e {
                    ClientError::Unreachable(_) => "에이전트에 연결할 수 없습니다(꺼졌거나 망이 다름)".to_string(),
                    other => other.to_string(),
                };
                let mut last = state.agents.last.lock().unwrap();
                for r in &refs {
                    let entry = last.entry(r.id.clone()).or_default();
                    entry.reachable = false;
                    entry.problem = Some(problem.clone());
                }
            }
        }
    }
    crate::remote::sync(app);
    emit(app);
}

/// 에이전트가 받은 업무를 이 PC에서 끝낸다(아직 되살리지 않았으면 다음 번에).
pub(crate) async fn end_local_job(app: &AppHandle, state: &AppState, r: &AgentRef) {
    let Ok(job) = state.job(&r.job_id) else {
        // 이 PC에 없는 업무: 이미 끝냈거나 아직 되살리지 않았다. 되살리기가 끝났는데도 없으면 끝난 것으로 둔다
        // (되살린 뒤면 다음 번에 끝낸다).
        if state.restored.initialized() {
            state.agents.set(&r.id, |x| x.local_ended = true);
        }
        return;
    };
    let meta = vec![(META_HANDED_OFF, serde_json::json!({ "addr": r.addr, "id": r.id }).to_string())];
    match crate::jobs::end_local(app, state, &job, meta).await {
        Ok(()) => state.agents.set(&r.id, |x| x.local_ended = true),
        Err(e) => tracing::warn!("넘긴 업무를 이 PC에서 끝낼 수 없습니다: {e}"),
    }
}

/// 화면에 보낸다(바뀌었을 때만).
pub(crate) fn emit(app: &AppHandle) {
    let state = app.state::<AppState>();
    let views = state.agents.build(&state.remotes);
    let mut last = state.agents.views.lock().unwrap();
    if *last != views {
        *last = views.clone();
        drop(last);
        let _ = app.emit(EVENT, views);
    }
}

fn client(addr: &str) -> CmdResult<AgentClient> {
    AgentClient::new(addr)
}

/// 에이전트 주소 입력란의 기본값: 마지막으로 연결한 주소, 없으면 빌드 때 넣은 주소(`MM_AGENT_URL` — Release 워크플로는
/// 저장소 변수 `AGENT_URL`).
#[tauri::command]
pub fn agent_last_addr(state: State<'_, AppState>) -> Option<String> {
    let last = state.agents.data.lock().unwrap().last_addr.clone();
    last.or_else(|| option_env!("MM_AGENT_URL").map(str::trim).filter(|a| !a.is_empty()).map(str::to_string))
}

/// 에이전트에 연결해 본다(PC 이름·버전·기본 저장 폴더·Excel·시각).
#[tauri::command]
pub async fn agent_connect(state: State<'_, AppState>, addr: String) -> CmdResult<AgentConnection> {
    let client = client(&addr)?;
    let info = client.info().await?;
    let addr = client.base_url();
    state.agents.remember_addr(&addr);
    Ok(AgentConnection {
        addr,
        clock_skew_ms: info.now_ms - chrono::Utc::now().timestamp_millis(),
        info,
        app_version: env!("CARGO_PKG_VERSION").to_string(),
    })
}

/// 에이전트 기본 저장 폴더 아래 폴더 하나.
#[tauri::command]
pub async fn agent_folders(addr: String, path: Vec<String>) -> CmdResult<FolderList> {
    Ok(client(&addr)?.folders(&path).await?)
}

#[tauri::command]
pub fn agent_services(state: State<'_, AppState>) -> Vec<AgentServiceView> {
    state.agents.views()
}

/// 지금 다시 묻는다.
#[tauri::command]
pub async fn agent_refresh(app: AppHandle, state: State<'_, AppState>) -> CmdResult<Vec<AgentServiceView>> {
    refresh(&app).await;
    Ok(state.agents.views())
}

async fn act<F, Fut>(app: &AppHandle, state: &AppState, id: &str, f: F) -> CmdResult<AgentServiceView>
where
    F: FnOnce(AgentClient, AgentRef) -> Fut,
    Fut: std::future::Future<Output = Result<ServiceStatus, ClientError>>,
{
    let r = state.agents.find(id)?;
    let status = f(client(&r.addr)?, r.clone()).await?;
    state.agents.record_status(id, status);
    emit(app);
    state.agents.views().into_iter().find(|v| v.id == id).ok_or_else(|| "에이전트 서비스를 찾을 수 없습니다".into())
}

/// 지금 저장하고 끝내기(`save`) / 저장 없이 끝내기.
#[tauri::command]
pub async fn agent_finish(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    save: bool,
) -> CmdResult<AgentServiceView> {
    act(&app, &state, &id, |c, r| async move { c.finish(&r.id, &r.token, save).await }).await
}

/// 저장 실패를 다시 저장한다.
#[tauri::command]
pub async fn agent_retry(app: AppHandle, state: State<'_, AppState>, id: String) -> CmdResult<AgentServiceView> {
    act(&app, &state, &id, |c, r| async move { c.retry(&r.id, &r.token).await }).await
}

/// 목록에서 지운다(끝난 서비스, 또는 더는 닿지 않는 에이전트). 에이전트의 서비스는 그대로다.
#[tauri::command]
pub fn agent_forget(app: AppHandle, state: State<'_, AppState>, id: String) -> Vec<AgentServiceView> {
    state.agents.remove(&id);
    state.remotes.remove(&id);
    emit(&app);
    state.agents.views()
}
