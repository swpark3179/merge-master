//! 에이전트 업무를 이 PC(관리자 PC)의 Excel로 저장한다: 넘길 때 보낸 원본 사본에 에이전트의 취합 데이터([`SaveData`])를
//! 써서 새 파일로 저장한다 — 에이전트의 마감 저장과 같은 결과(충돌 칸은 취합 값, 함께 넣기는 넘길 때 고른 것). 원본과 이
//! PC 사용자의 Excel은 건드리지 않는다.
//!
//! - 서비스 중이면 '지금 상태 저장'([`agent_save_now`]): 지금 상태를 받아 저장한다(서비스는 그대로 이어진다).
//! - 관리자 저장 대기(에이전트가 마감 때 엑셀로 저장하지 못했다)면 폴링이 취합 데이터를 이 PC에 받아 두고
//!   (`agent-results\<id>.mmsave`) 작업 표시줄을 깜빡인다. 관리자가 저장하면([`agent_save_result`]) 에이전트에 알려 그쪽
//!   데이터를 지우고 끝내게 한다. 알리지 못했으면 기억해 두었다가 다음 폴링 때 다시 알린다.
//! - 서비스가 끝나면(저장함·저장 없이 끝냄) 이 PC에 둔 원본 사본(`agent-sources\`)·취합 데이터를 지운다.

use std::path::{Path, PathBuf};

use agent_protocol::client::{AgentClient, ClientError};
use agent_protocol::save_data::SaveData;
use agent_protocol::{HostSaved, ServiceState, extension};
use serde::Serialize;
use tauri::{AppHandle, Manager, State};
use ts_rs::TS;

use crate::agent::{AgentRef, emit, remove_files};
use crate::{AppState, CmdResult, run_excel};

const SOURCES: &str = "agent-sources";
const RESULTS: &str = "agent-results";

/// 이 PC에 저장한 결과.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AgentSaved {
    pub path: String,
    /// 에이전트에 알렸다(관리자 저장 대기 — 그쪽 취합 데이터를 지우고 끝냈다). 알리지 못했으면 다음에 연결될 때 알린다.
    pub told: bool,
}

fn data_dir(app: &AppHandle, sub: &str) -> CmdResult<PathBuf> {
    let dir = app.path().app_local_data_dir().map_err(|e| e.to_string())?.join(sub);
    std::fs::create_dir_all(&dir).map_err(|e| format!("폴더를 만들 수 없습니다: {e}"))?;
    Ok(dir)
}

/// 넘긴 서비스의 원본 사본을 둘 곳.
pub(crate) fn source_path(app: &AppHandle, id: &str, ext: &str) -> CmdResult<PathBuf> {
    Ok(data_dir(app, SOURCES)?.join(format!("{id}.{ext}")))
}

/// 경로의 파일 이름(에이전트 PC의 Windows 경로도).
fn file_name_of(path: &str) -> Option<&str> {
    path.rsplit(['\\', '/']).next().filter(|n| !n.is_empty())
}

/// 이 PC에서 저장할 기본 경로: 원본 폴더(없으면 문서 폴더)에 관리자 저장 대기면 에이전트가 저장하려던 이름, 아니면
/// `원본명_취합_오늘`.
#[tauri::command]
pub fn agent_save_default(state: State<'_, AppState>, id: String) -> CmdResult<String> {
    let r = state.agents.find(&id)?;
    let folder = r
        .original
        .as_deref()
        .map(Path::new)
        .filter(|p| p.is_absolute())
        .and_then(Path::parent)
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let path = crate::save::default_copy_path(&r.workbook, &folder);
    let status = state.agents.last_status(&id).filter(|s| s.state == ServiceState::AwaitingHost);
    Ok(match status.as_ref().and_then(|s| file_name_of(&s.output_path)) {
        Some(name) => Path::new(&path).with_file_name(name).display().to_string(),
        None => path,
    })
}

/// 서비스 중인 에이전트 업무의 지금 상태를 이 PC에 새 파일로 저장한다(서비스는 그대로 이어진다).
#[tauri::command]
pub async fn agent_save_now(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    path: String,
) -> CmdResult<AgentSaved> {
    let r = state.agents.find(&id)?;
    let bytes = AgentClient::new(&r.addr)?.save_data(&r.id, &r.token).await?;
    let data = decode(bytes).await?;
    let saved = save_here(&app, &state, &r, data, PathBuf::from(path)).await?;
    tracing::info!(id, "에이전트 업무의 지금 상태를 이 PC에 저장했습니다: {saved}");
    Ok(AgentSaved { path: saved, told: false })
}

/// 관리자 저장 대기의 취합 데이터를 이 PC에 저장하고 에이전트에 알린다(그쪽 데이터를 지우고 끝낸다).
#[tauri::command]
pub async fn agent_save_result(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    path: String,
) -> CmdResult<AgentSaved> {
    let r = state.agents.find(&id)?;
    if let Some(h) = &r.host_saved {
        return Err(format!("이미 이 PC에 저장했습니다({}). 에이전트에 연결되면 알립니다", h.saved_path));
    }
    let file = match r.result.clone().filter(|p| p.is_file()) {
        Some(f) => f,
        None => fetch(&app, &state, &r).await?,
    };
    let bytes = tauri::async_runtime::spawn_blocking(move || std::fs::read(file))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("받아 둔 취합 데이터를 읽을 수 없습니다: {e}"))?;
    let data = decode(bytes).await?;
    let saved = save_here(&app, &state, &r, data, PathBuf::from(path)).await?;
    tracing::info!(id, "관리자 저장 대기를 이 PC에 저장했습니다: {saved}");
    let note = HostSaved { saved_path: saved.clone(), saved_at_ms: chrono::Utc::now().timestamp_millis() };
    state.agents.set(&id, |x| x.host_saved = Some(note));
    let told = tell(&state, &id).await;
    emit(&app);
    Ok(AgentSaved { path: saved, told })
}

async fn decode(bytes: Vec<u8>) -> CmdResult<SaveData> {
    tauri::async_runtime::spawn_blocking(move || SaveData::decode(&bytes)).await.map_err(|e| e.to_string())?
}

/// 원본 사본에 취합 데이터를 써서 `out`에 저장한다(이 PC의 숨은 Excel).
async fn save_here(app: &AppHandle, state: &AppState, r: &AgentRef, data: SaveData, out: PathBuf) -> CmdResult<String> {
    let source = ensure_source(app, state, r).await?;
    let report = run_excel(&state.excel, move |s| s.save_file_copy_multi(&source, &data.plan(), &out)).await?;
    if !report.mismatched.is_empty() {
        tracing::warn!("저장 뒤 다시 읽은 값이 다른 칸 {}개: {}", report.mismatched.len(), report.path);
    }
    Ok(report.path)
}

/// 넘길 때 보낸 원본 사본(이 PC에 없으면 에이전트에서 받는다).
async fn ensure_source(app: &AppHandle, state: &AppState, r: &AgentRef) -> CmdResult<PathBuf> {
    if let Some(p) = r.source.clone().filter(|p| p.is_file()) {
        return Ok(p);
    }
    let ext = extension(&r.workbook);
    let path = source_path(app, &r.id, if ext.is_empty() { "xlsx" } else { &ext })?;
    let bytes = AgentClient::new(&r.addr)?.source(&r.id, &r.token).await?;
    let to = path.clone();
    tauri::async_runtime::spawn_blocking(move || write_atomic(&to, &bytes)).await.map_err(|e| e.to_string())??;
    state.agents.set(&r.id, |x| x.source = Some(path.clone()));
    Ok(path)
}

/// 에이전트의 취합 데이터를 받아(읽을 수 있는지 본 뒤) 이 PC에 둔다.
async fn fetch(app: &AppHandle, state: &AppState, r: &AgentRef) -> CmdResult<PathBuf> {
    let bytes = AgentClient::new(&r.addr)?.save_data(&r.id, &r.token).await?;
    let path = data_dir(app, RESULTS)?.join(format!("{}.mmsave", r.id));
    let to = path.clone();
    tauri::async_runtime::spawn_blocking(move || -> CmdResult<()> {
        SaveData::decode(&bytes)?;
        write_atomic(&to, &bytes)
    })
    .await
    .map_err(|e| e.to_string())??;
    state.agents.set(&r.id, |x| x.result = Some(path.clone()));
    if state.agents.find(&r.id).is_err() {
        // 받는 사이 목록에서 지웠다.
        remove_files(std::slice::from_ref(&path));
        return Err("목록에서 지운 서비스입니다".into());
    }
    Ok(path)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> CmdResult<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)
        .and_then(|()| std::fs::rename(&tmp, path))
        .map_err(|e| format!("{}에 쓸 수 없습니다: {e}", path.display()))
}

/// 이 PC에 저장한 것을 에이전트에 알린다. 알렸거나 더 알릴 수 없으면 기억과 이 PC에 둔 파일을 지운다. 알렸으면 `true`.
async fn tell(state: &AppState, id: &str) -> bool {
    let Ok(r) = state.agents.find(id) else { return false };
    let Some(note) = r.host_saved.clone() else { return true };
    let Ok(client) = AgentClient::new(&r.addr) else { return false };
    match client.host_saved(&r.id, &r.token, &note).await {
        Ok(status) => {
            state.agents.record_status(id, status);
            forget_local(state, id);
            true
        }
        // 에이전트가 저장하는 중(트레이 '다시 저장')이면 다음에 다시 알린다. 그 밖의 거절(이미 끝냈다·없다·관리 키)은 더
        // 알릴 수 없다 — 이 PC에 저장한 파일은 그대로다.
        Err(ClientError::Api { status, message }) => {
            let saving = state.agents.last_status(id).is_some_and(|s| s.state == ServiceState::Saving);
            if !(status == 409 && saving) {
                tracing::warn!(id, "이 PC에 저장한 것을 에이전트에 알리지 못했습니다: {message}");
                forget_local(state, id);
            }
            false
        }
        Err(ClientError::Unreachable(_)) => false,
    }
}

/// 이 PC에 둔 원본 사본·취합 데이터를 지운다(기억은 남긴다).
fn forget_local(state: &AppState, id: &str) {
    let mut files = Vec::new();
    state.agents.set(id, |x| {
        files = x.local_files();
        x.source = None;
        x.result = None;
        x.host_saved = None;
    });
    remove_files(&files);
}

/// 폴링마다(상태를 받은 뒤): 알리지 못한 저장을 알리고, 관리자 저장 대기의 취합 데이터를 받아 두고, 끝난 서비스의 이 PC
/// 파일을 지운다.
pub(crate) async fn follow_up(app: &AppHandle) {
    let state = app.state::<AppState>();
    for r in state.agents.refs() {
        if r.pending {
            continue;
        }
        if r.host_saved.is_some() {
            tell(&state, &r.id).await;
            continue;
        }
        let Some(status) = state.agents.last_status(&r.id) else { continue };
        match status.state {
            ServiceState::AwaitingHost if status.result_ready && !r.result.as_ref().is_some_and(|p| p.is_file()) => {
                spawn_fetch(app, r);
            }
            ServiceState::Done | ServiceState::Cancelled if !r.local_files().is_empty() => forget_local(&state, &r.id),
            _ => {}
        }
    }
}

/// 취합 데이터를 뒤에서 받는다(서비스마다 하나만). 받으면 작업 표시줄을 깜빡여 알린다.
fn spawn_fetch(app: &AppHandle, r: AgentRef) {
    let state = app.state::<AppState>();
    if !state.agents.fetching.lock().unwrap().insert(r.id.clone()) {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        match fetch(&app, &state, &r).await {
            Ok(_) => {
                tracing::info!(id = %r.id, "관리자 저장 대기의 취합 데이터를 받았습니다");
                crate::monitor::request_attention(&app);
                emit(&app);
            }
            Err(e) => tracing::warn!(id = %r.id, "취합 데이터를 받지 못했습니다(다음에 다시 합니다): {e}"),
        }
        state.agents.fetching.lock().unwrap().remove(&r.id);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_of_agent_paths() {
        assert_eq!(file_name_of(r"C:\Users\a\문서\엑셀 취합 결과\budget_취합.xlsx"), Some("budget_취합.xlsx"));
        assert_eq!(file_name_of("/tmp/a.xlsx"), Some("a.xlsx"));
        assert_eq!(file_name_of(r"C:\dir\"), None);
    }
}
