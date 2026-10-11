//! 1c IP 명단: IP별 참여자 이름. 이 명단에 있는 IP에서 접속하면 이름을 묻지 않고 그 이름으로 들어오고, 명단 밖 IP는
//! 이름을 입력한 뒤 호스트가 승인한다. 호스트 PC의 IP가 명단에 있으면 그 이름이 호스트 이름이다.
//!
//! 앱 설정 폴더의 `ip-names.json`에 두고(앱을 다시 켜도 남는다), 바꾸면 모든 업무의 세션에 알린다(명단은 업무 공통).
//! JSON 파일로 내보내고 불러온다.

use std::path::{Path, PathBuf};

use collect_server::{IpEntry, IpNames, IpNamesView, SessionHandle};
use tauri::{AppHandle, Manager, State};

use crate::{AppState, CmdResult};

const FILE: &str = "ip-names.json";

fn file(app: &AppHandle) -> Option<PathBuf> {
    Some(app.path().app_local_data_dir().ok()?.join(FILE))
}

fn read(path: &Path) -> Result<IpNames, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("파일을 읽을 수 없습니다: {e}"))?;
    IpNames::parse_json(&String::from_utf8_lossy(&bytes))
}

/// 앱을 켤 때 읽는다. 없거나 읽을 수 없으면 빈 명단(읽을 수 없는 파일은 그대로 둔다).
pub(crate) fn load(app: &AppHandle) -> IpNames {
    let Some(path) = file(app).filter(|p| p.exists()) else { return IpNames::default() };
    read(&path).unwrap_or_else(|e| {
        tracing::warn!("IP 명단을 읽을 수 없습니다({}): {e}", path.display());
        IpNames::default()
    })
}

/// 새로 만든 세션에 지금 명단을 넘긴다(시트를 처음 확정할 때, 이어서 하기).
pub(crate) fn attach(state: &AppState, session: &SessionHandle) {
    session.set_ip_names(state.ip_names.lock().unwrap().clone());
}

fn view(app: &AppHandle, names: &IpNames) -> IpNamesView {
    IpNamesView {
        entries: names.entries(),
        host_ips: collect_server::lan_addresses().into_iter().map(|a| a.ip).collect(),
        path: file(app).map(|p| p.display().to_string()).unwrap_or_default(),
    }
}

/// 명단을 바꾼다: 파일에 남기고 모든 업무의 세션과 참여자 업무 목록('내 담당')에 알린다.
fn replace(app: &AppHandle, state: &AppState, names: IpNames) -> CmdResult<IpNamesView> {
    let path = file(app).ok_or("앱 설정 폴더를 찾을 수 없습니다")?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("앱 설정 폴더를 만들 수 없습니다: {e}"))?;
    }
    std::fs::write(&path, names.to_json()).map_err(|e| format!("IP 명단을 저장할 수 없습니다: {e}"))?;
    for job in state.all_jobs() {
        if let Ok(session) = job.session() {
            session.set_ip_names(names.clone());
        }
    }
    state.hub.set_ip_names(names.clone());
    let out = view(app, &names);
    *state.ip_names.lock().unwrap() = names;
    Ok(out)
}

#[tauri::command]
pub fn ip_names_get(app: AppHandle, state: State<'_, AppState>) -> IpNamesView {
    view(&app, &state.ip_names.lock().unwrap())
}

/// 화면에서 고친 목록 전체.
#[tauri::command]
pub fn ip_names_set(app: AppHandle, state: State<'_, AppState>, entries: Vec<IpEntry>) -> CmdResult<IpNamesView> {
    let names = IpNames::from_entries(&entries)?;
    replace(&app, &state, names)
}

/// 한 사람을 더하거나 이름을 바꾼다(승인할 때 'IP 명단에 저장').
#[tauri::command]
pub fn ip_names_put(app: AppHandle, state: State<'_, AppState>, ip: String, name: String) -> CmdResult<IpNamesView> {
    let mut names = state.ip_names.lock().unwrap().clone();
    names.put(&ip, &name)?;
    replace(&app, &state, names)
}

/// JSON 파일을 불러와 명단을 바꾼다.
#[tauri::command]
pub fn ip_names_import(app: AppHandle, state: State<'_, AppState>, path: PathBuf) -> CmdResult<IpNamesView> {
    let names = read(&path)?;
    replace(&app, &state, names)
}

/// 지금 명단을 JSON 파일로 내보낸다. 쓴 사람 수를 돌려준다.
#[tauri::command]
pub fn ip_names_export(state: State<'_, AppState>, path: PathBuf) -> CmdResult<u32> {
    let names = state.ip_names.lock().unwrap().clone();
    std::fs::write(&path, names.to_json()).map_err(|e| format!("파일을 쓸 수 없습니다: {e}"))?;
    Ok(names.len() as u32)
}
