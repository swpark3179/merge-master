//! 1c 서비스·접속자: 업무 공개/공개 중지, 접속자 표 이벤트, 승인·거절·연결 끊기·잠금 해제.
//!
//! 참여자 서버는 하나(포트 하나)이고 업무마다 주소 경로(slug)로 공개한다([`collect_server::Hub`]). 공개 중인 업무가 하나라도
//! 있으면 서버를 열어 두고, 마지막 업무의 공개를 멈추면 포트를 닫는다. 포트는 모든 업무가 함께 쓴다.

use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use std::time::Duration;

use collect_core::CellRef;
use collect_core::a1::parse_sheet_range;
use collect_server::{Gone, HostSnapshot, ServerConfig, SessionHandle};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use ts_rs::TS;

use crate::jobs::{Job, emit_jobs, event};
use crate::{AppState, CmdResult};

/// 접속자 표 이벤트를 웹뷰에 보내는 최소 간격(커서가 움직일 때마다 보내지 않도록).
const EMIT_EVERY: Duration = Duration::from_millis(250);

#[derive(Serialize, TS)]
#[ts(export)]
pub struct ServiceUrl {
    name: String,
    ip: String,
    url: String,
}

#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ServiceInfo {
    port: u16,
    slug: String,
    /// 이 업무 주소(`http://IP:포트/{slug}`), 네트워크마다.
    urls: Vec<ServiceUrl>,
    /// 공개 중인 업무 목록 주소(`http://IP:포트/`), 네트워크마다.
    list_urls: Vec<ServiceUrl>,
}

/// 참여자 서버 상태(모든 업무 공통).
#[derive(Serialize, TS)]
#[ts(export)]
pub struct ServerStatus {
    running: bool,
    /// 열려 있으면 그 포트, 아니면 다음에 열 포트(마지막에 쓴 포트).
    port: u16,
    /// 공개 중인 업무 수.
    published: u32,
}

fn urls(port: u16, path: &str) -> Vec<ServiceUrl> {
    collect_server::lan_addresses()
        .into_iter()
        .map(|a| ServiceUrl { url: format!("http://{}:{port}/{path}", a.ip), ip: a.ip, name: a.name })
        .collect()
}

fn info(port: u16, slug: &str) -> ServiceInfo {
    ServiceInfo { port, slug: slug.to_string(), urls: urls(port, slug), list_urls: urls(port, "") }
}

/// 접속자 표가 바뀌면 웹뷰에 알린다(세션이 살아 있는 동안 — 공개를 멈춰도 진행률은 계속 보인다).
pub(crate) fn forward_participants(app: &AppHandle, job: &str, session: &SessionHandle) {
    let mut rx = session.subscribe();
    let app = app.clone();
    let name = event("participants", job);
    let _ = app.emit(&name, rx.borrow().clone());
    tauri::async_runtime::spawn(async move {
        while rx.changed().await.is_ok() {
            let snap = rx.borrow_and_update().clone();
            let _ = app.emit(&name, snap);
            tokio::time::sleep(EMIT_EVERY).await;
        }
    });
}

/// 기록 파일에 공개 상태를 남긴다(앱을 다시 켜면 같은 포트·주소로 다시 공개한다).
async fn remember_server(session: &SessionHandle, port: u16, slug: &str, running: bool) {
    let meta = collect_server::ServerMeta { port, slug: slug.to_string(), running };
    let json = serde_json::to_string(&meta).unwrap_or_default();
    let _ = session.with(move |s| s.set_meta(collect_server::META_SERVER, &json)).await;
}

/// 업무를 이 주소 경로로 공개한다. 서버가 닫혀 있으면 `port`로 연다. 다른 업무가 공개 중이면 포트는 바꿀 수 없다.
pub(crate) async fn publish(
    app: &AppHandle,
    state: &AppState,
    job: &Arc<Job>,
    port: u16,
    slug: String,
) -> CmdResult<ServiceInfo> {
    let slug = slug.trim().to_string();
    collect_server::validate_slug(&slug)?;
    if !state.slug_free(&slug, Some(&job.id)) {
        return Err(format!("‘{slug}’ 주소는 다른 업무가 쓰고 있습니다. 다른 주소 경로를 정해 주세요"));
    }
    let session = job.session()?;
    let mut server = state.server.lock().await;
    if let Some(h) = server.as_ref()
        && h.port() != port
    {
        if state.hub.sessions().iter().any(|(_, s)| !s.same(&session)) {
            return Err(format!(
                "포트는 모든 업무가 함께 씁니다. 다른 업무가 포트 {}로 공개 중이라 바꿀 수 없습니다",
                h.port()
            ));
        }
        // 이 업무만 공개 중이었다: 새 포트로 다시 연다.
        if let Some(old) = state.hub.slug_of(&session) {
            state.hub.unpublish(&old, false);
        }
        if let Some(old) = server.take() {
            old.stop().await;
        }
    }
    // 다른 주소로 공개 중이었으면 옮긴다.
    if let Some(old) = state.hub.slug_of(&session).filter(|s| !s.eq_ignore_ascii_case(&slug)) {
        state.hub.unpublish(&old, false);
    }
    if server.is_none() {
        let config = ServerConfig { bind: IpAddr::V4(Ipv4Addr::UNSPECIFIED), port, dev_admin: false };
        *server = Some(collect_server::start(config, state.hub.clone()).await.map_err(|e| e.to_string())?);
    }
    let port = server.as_ref().map_or(port, |h| h.port());
    if let Err(e) = state.hub.publish(&slug, session.clone()) {
        if state.hub.is_empty()
            && let Some(h) = server.take()
        {
            h.stop().await;
        }
        return Err(e);
    }
    drop(server);
    *state.port.lock().unwrap() = port;
    *job.service.lock().unwrap() = crate::jobs::JobService { slug: slug.clone(), published: true };
    remember_server(&session, port, &slug, true).await;
    emit_jobs(app);
    Ok(info(port, &slug))
}

/// 업무의 공개를 멈춘다(그 업무의 참여자 연결만 닫힌다, `ended`면 '업무를 끝냈습니다'). 공개 중인 업무가 없으면 서버를
/// 닫는다(포트를 닫는다). 세션(입력한 값·이력)은 그대로 남아 다시 공개하면 이어서 받는다.
pub(crate) async fn unpublish(app: &AppHandle, state: &AppState, job: &Job, ended: bool) -> CmdResult<()> {
    unpublish_as(app, state, job, if ended { Gone::Ended } else { Gone::Paused }).await
}

/// [`unpublish`]와 같지만 참여자에게 알릴 까닭을 고른다(마감 시각에 공유 닫기).
pub(crate) async fn unpublish_as(app: &AppHandle, state: &AppState, job: &Job, why: Gone) -> CmdResult<()> {
    let mut server = state.server.lock().await;
    let session = job.session.lock().unwrap().clone();
    if let Some(session) = &session
        && let Some(slug) = state.hub.slug_of(session)
    {
        state.hub.unpublish_as(&slug, why);
    }
    let was = std::mem::replace(&mut job.service.lock().unwrap().published, false);
    if state.hub.is_empty()
        && let Some(h) = server.take()
    {
        h.stop().await;
    }
    drop(server);
    if was && let Some(session) = &session {
        let port = *state.port.lock().unwrap();
        remember_server(session, port, &job.slug(), false).await;
    }
    emit_jobs(app);
    Ok(())
}

/// 공개 중이면 그 업무의 주소.
pub(crate) async fn info_of(state: &AppState, job: &Job) -> Option<ServiceInfo> {
    let port = state.server.lock().await.as_ref().map(|h| h.port())?;
    job.published().then(|| info(port, &job.slug()))
}

#[tauri::command]
pub async fn service_publish(
    app: AppHandle,
    state: State<'_, AppState>,
    job: String,
    port: u16,
    slug: String,
) -> CmdResult<ServiceInfo> {
    let job = state.job(&job)?;
    publish(&app, &state, &job, port, slug).await
}

#[tauri::command]
pub async fn service_unpublish(app: AppHandle, state: State<'_, AppState>, job: String) -> CmdResult<()> {
    let job = state.job(&job)?;
    unpublish(&app, &state, &job, false).await
}

/// 그 업무가 공개 중이면 주소(화면을 다시 그릴 때).
#[tauri::command]
pub async fn service_info(state: State<'_, AppState>, job: String) -> CmdResult<Option<ServiceInfo>> {
    let job = state.job(&job)?;
    Ok(info_of(&state, &job).await)
}

#[tauri::command]
pub async fn server_status(state: State<'_, AppState>) -> CmdResult<ServerStatus> {
    let running = state.server.lock().await.as_ref().map(|h| h.port());
    Ok(ServerStatus {
        running: running.is_some(),
        port: running.unwrap_or(*state.port.lock().unwrap()),
        published: state.hub.len() as u32,
    })
}

/// 시트를 확정했으면 지금의 접속자 표.
#[tauri::command]
pub fn participants_get(state: State<'_, AppState>, job: String) -> Option<HostSnapshot> {
    state.session(&job).ok().map(|s| s.snapshot())
}

/// 명단에 없는 이름을 승인한다. `range`는 `sheet` 시트의 주소(예: H500:H620, `인력계획!E5:E9`처럼 시트 이름을 붙여도
/// 된다)로, 주면 그 범위를 배정한다.
#[tauri::command]
pub async fn participant_approve(
    app: AppHandle,
    state: State<'_, AppState>,
    job: String,
    name: String,
    sheet: u32,
    range: Option<String>,
    note: String,
) -> CmdResult<()> {
    let session = state.session(&job)?;
    let range = match range.as_deref().map(str::trim).filter(|r| !r.is_empty()) {
        Some(text) => {
            let sheets: Vec<(String, CellRef)> =
                session.with(|s| s.sheets.iter().map(|x| (x.model.sheet.clone(), x.model.origin)).collect()).await?;
            let names: Vec<&str> = sheets.iter().map(|(n, _)| n.as_str()).collect();
            let (i, range) = parse_sheet_range(text, &names, sheet as usize).map_err(|e| e.to_string())?;
            let origin = sheets.get(i).map(|(_, o)| *o).ok_or("없는 시트입니다")?;
            let range = range.relative_to(origin).ok_or_else(|| format!("{text}은(는) 불러온 표 밖입니다"))?;
            Some((i as u32, range))
        }
        None => None,
    };
    // 설정은 세션이 가진다: 1b 화면은 바뀌었다는 알림을 받고 세션에서 다시 읽는다.
    session.approve(&name, range, &note).await?;
    let _ = app.emit(&event("config-changed", &job), ());
    Ok(())
}

#[tauri::command]
pub async fn participant_reject(state: State<'_, AppState>, job: String, name: String) -> CmdResult<()> {
    state.session(&job)?.reject(&name).await
}

#[tauri::command]
pub async fn participant_kick(state: State<'_, AppState>, job: String, name: String) -> CmdResult<()> {
    state.session(&job)?.kick(&name).await
}

/// 그 사람이 잡고 있는 편집 잠금을 푼다(편집기를 열어 둔 채 자리를 비운 경우).
#[tauri::command]
pub async fn participant_unlock(state: State<'_, AppState>, job: String, name: String) -> CmdResult<()> {
    state.session(&job)?.unlock(&name).await
}
