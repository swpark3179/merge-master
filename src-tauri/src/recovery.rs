//! 7단계 세션 복구: 앱을 다시 켰을 때 가장 최근 기록 파일에서 취합을 이어서 한다.
//!
//! 1a 화면이 [`recovery_check`]로 '이어서 하기'를 묻고, [`recovery_resume`]이 세션을 되살린 뒤 원본 Excel을 다시 찾고
//! (앞 앱이 남긴 숨은 Excel 정리), 계산 사본을 열고, 서비스가 켜져 있었으면 같은 포트로 연다. 참여자 브라우저는 스스로
//! 다시 붙는다(보내지 못한 입력은 대기열에서 다시 보내고, 이미 반영된 묶음은 한 번만 반영된다).

use std::path::{Path, PathBuf};

use collect_core::SheetModel;
use collect_server::SessionHandle;
use serde::Serialize;
use tauri::{AppHandle, Manager, State};
use ts_rs::TS;

use crate::service::ServiceInfo;
use crate::{AppState, CmdResult, run_excel};

/// 남겨 둘 기록 파일 수(더 오래된 것은 앱을 켤 때 지운다).
const KEEP_FILES: usize = 20;
const META_DISCARDED: &str = "discarded";

pub(crate) fn sessions_dir(app: &AppHandle) -> Option<PathBuf> {
    Some(app.path().app_local_data_dir().ok()?.join("sessions"))
}

/// 기록 파일(이름이 시각이라 이름순 = 시간순), 최근 것부터.
fn session_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|it| it.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "sqlite")).collect())
        .unwrap_or_default();
    files.sort();
    files.reverse();
    files
}

fn remove_with_sidecars(p: &Path) {
    let _ = std::fs::remove_file(p);
    for ext in ["-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{ext}", p.display()));
    }
}

/// 최근 [`KEEP_FILES`]개만 남긴다.
pub(crate) fn cleanup(dir: &Path) {
    for old in session_files(dir).into_iter().skip(KEEP_FILES) {
        remove_with_sidecars(&old);
    }
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RecoveryInfo {
    /// 원본 통합문서 이름과 시트들(불러온 순서).
    pub workbook: String,
    pub sheets: Vec<String>,
    /// 확정한 시각(`YYYYMMDD-HHMMSS`).
    pub created: String,
    #[ts(type = "number")]
    pub changes: u64,
    #[ts(optional, type = "number")]
    pub last_change_ms: Option<i64>,
    /// 서비스가 켜져 있었으면 그 포트·경로(이어서 하면 다시 연다).
    #[ts(optional)]
    pub port: Option<u16>,
    #[ts(optional)]
    pub slug: Option<String>,
    pub closed: bool,
}

/// 이어서 할 수 있는 가장 최근 기록 파일.
fn latest(app: &AppHandle) -> Option<(PathBuf, collect_store::Peek)> {
    let path = session_files(&sessions_dir(app)?).into_iter().next()?;
    let peek = collect_store::peek(&path).ok()?;
    (peek.resumable && !peek.meta.get(META_DISCARDED).is_some_and(|v| v == "1")).then_some((path, peek))
}

#[tauri::command]
pub fn recovery_check(app: AppHandle, state: State<'_, AppState>) -> Option<RecoveryInfo> {
    if state.session.lock().unwrap().is_some() {
        return None; // 이미 취합 중
    }
    let (_, peek) = latest(&app)?;
    let meta = &peek.meta;
    let server: Option<collect_server::ServerMeta> =
        meta.get(collect_server::META_SERVER).and_then(|s| serde_json::from_str(s).ok());
    let running = server.filter(|s| s.running);
    let workbook = meta.get("workbook").cloned().unwrap_or_default();
    Some(RecoveryInfo {
        workbook: Path::new(&workbook).file_name().map_or(workbook.clone(), |n| n.to_string_lossy().to_string()),
        sheets: collect_server::session::meta_sheets(meta),
        created: meta.get("created").cloned().unwrap_or_default(),
        changes: peek.changes,
        last_change_ms: peek.last_change_ms,
        port: running.as_ref().map(|s| s.port),
        slug: running.map(|s| s.slug),
        closed: meta.get("closed").is_some_and(|v| v == "1"),
    })
}

#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ResumeResult {
    /// 되살린 시트들(세션 안 순번 순서).
    pub models: Vec<SheetModel>,
    /// 다시 연 서비스(켜져 있었을 때).
    #[ts(optional)]
    pub service: Option<ServiceInfo>,
    /// 서비스를 다시 열지 못한 까닭(포트를 다른 프로그램이 씀 등). 취합은 이어서 할 수 있다.
    #[ts(optional)]
    pub service_error: Option<String>,
}

#[tauri::command]
pub async fn recovery_resume(app: AppHandle, state: State<'_, AppState>) -> CmdResult<ResumeResult> {
    if state.session.lock().unwrap().is_some() {
        return Err("이미 취합 중입니다".into());
    }
    let (path, _) = latest(&app).ok_or("이어서 할 취합이 없습니다")?;
    let rec = collect_server::recover(&path)?;
    let (workbook_id, sheets, server) = (rec.workbook_id.clone(), rec.sheets.clone(), rec.server.clone());
    let session = SessionHandle::spawn(rec.init);
    crate::people::attach(&state, &session);
    crate::service::forward_participants(&app, &session);
    crate::monitor::forward_events(&app, &session);
    *state.session.lock().unwrap() = Some(session.clone());
    *state.source.lock().unwrap() =
        Some(crate::save::Source { workbook_id: workbook_id.clone(), sheets: sheets.clone() });
    let models: Vec<SheetModel> = session.with(|s| s.sheets.iter().map(|x| x.model.clone()).collect()).await?;

    // 앞 앱이 남긴 숨은 Excel(계산 사본·원본)을 정리하고, 계산 사본을 다시 연다. Excel이 없어도 취합은 이어서 한다.
    let id = workbook_id.clone();
    match run_excel(&state.excel, move |s| s.adopt_orphans(Some(&id))).await {
        Ok(r) if r.closed_calc > 0 || r.adopted => tracing::info!("앞 앱이 남긴 Excel 정리: {r:?}"),
        Ok(_) => {}
        Err(e) => tracing::warn!("Excel 정리 실패: {e}"),
    }
    state.recalc.start(&app, state.excel.clone(), session, &models, &workbook_id, &sheets);

    let (mut service, mut service_error) = (None, None);
    if let Some(s) = server.filter(|s| s.running) {
        match crate::service::start_service(&state, s.port, s.slug).await {
            Ok(info) => service = Some(info),
            Err(e) => service_error = Some(e),
        }
    }
    Ok(ResumeResult { models, service, service_error })
}

/// '버리고 새로 시작': 그 파일은 다시 묻지 않는다(파일은 남는다).
#[tauri::command]
pub fn recovery_discard(app: AppHandle) -> CmdResult<()> {
    let (path, _) = latest(&app).ok_or("이어서 할 취합이 없습니다")?;
    let store = collect_store::Store::open(&path).map_err(|e| e.to_string())?;
    store.meta(META_DISCARDED, "1");
    store.flush().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_only_recent_files() {
        let dir = std::env::temp_dir().join(format!("mm-sessions-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for i in 0..25 {
            let p = dir.join(format!("20261009-1200{i:02}.sqlite"));
            std::fs::write(&p, b"x").unwrap();
            std::fs::write(format!("{}-wal", p.display()), b"x").unwrap();
        }
        std::fs::write(dir.join("다른 파일.txt"), b"x").unwrap();
        cleanup(&dir);
        let left = session_files(&dir);
        assert_eq!(left.len(), KEEP_FILES);
        assert!(left[0].ends_with("20261009-120024.sqlite"), "최근 것부터");
        assert!(!dir.join("20261009-120000.sqlite-wal").exists(), "딸린 파일도 지운다");
        assert!(dir.join("다른 파일.txt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
