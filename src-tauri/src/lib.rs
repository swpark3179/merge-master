//! 호스트 앱(Tauri) 진입점: Excel 스레드·취합 업무들(업무마다 세션)·참여자 서버를 앱 상태로 들고, 웹뷰에 command로
//! 노출한다. 업무에 딸린 command는 모두 `job`(업무 id)을 받는다([`jobs`]).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use collect_core::SheetModel;
use collect_core::config::CollectConfig;
use collect_server::{Hub, LanAddress, ServerHandle, SessionHandle, SessionInit};
use excel_com::{
    ExcelStatus, ExcelThread, ReadOptions, ReadProgress, ReadStats, Session, SheetInspection, WorkbookInfo,
};
use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, RunEvent, State};
use tokio::sync::Mutex;
use ts_rs::TS;

mod agent;
mod handoff;
mod host;
mod jobs;
mod monitor;
mod people;
mod recalc;
mod recovery;
mod remote;
mod save;
mod service;

pub(crate) struct AppState {
    excel: Arc<ExcelThread>,
    /// 진행 중인 취합 업무(업무 id 순 = 만든 순).
    jobs: std::sync::Mutex<std::collections::BTreeMap<String, Arc<jobs::Job>>>,
    /// 참여자에게 공개한 업무들(주소 경로별). 서버가 이 목록을 서비스한다.
    hub: Hub,
    /// 실행 중인 참여자 서버(포트 하나, 공개 중인 업무가 있는 동안).
    server: Mutex<Option<ServerHandle>>,
    /// 서버 포트(모든 업무 공통, 마지막에 연 포트).
    port: std::sync::Mutex<u16>,
    /// 진행 중인 읽기의 취소 표시. Excel 스레드가 읽는 중이어도 바로 바꿀 수 있게 Excel 스레드 밖에 둔다.
    read_cancel: std::sync::Mutex<Option<Arc<AtomicBool>>>,
    /// IP별 참여자 이름(1c IP 명단, 앱 설정 폴더의 JSON 파일). 모든 업무가 같이 쓴다.
    ip_names: std::sync::Mutex<collect_server::IpNames>,
    /// 앱을 켤 때 되살린 업무(한 번만).
    restored: tokio::sync::OnceCell<jobs::RestoreReport>,
    /// 마지막으로 보낸 업무 목록(바뀌었을 때만 보낸다).
    last_jobs: std::sync::Mutex<Vec<jobs::JobSummary>>,
    /// 에이전트 PC로 넘긴 서비스(`agents.json`).
    agents: agent::Agents,
    /// 서비스 중인 에이전트 업무의 관리자 연결(서비스 id별).
    remotes: remote::Remotes,
}

/// 1a에서 읽었지만 아직 확정하지 않은 시트들(미리보기, 시트 이름별)과 그 통합문서. 다른 통합문서를 읽으면 비운다.
pub(crate) struct Previews {
    workbook_id: String,
    sheets: std::collections::HashMap<String, SheetModel>,
}

pub(crate) type CmdResult<T> = Result<T, String>;

pub(crate) const NO_SHEET: &str = "먼저 시트를 불러와 확정해 주세요";

/// Excel 작업은 STA 스레드에서 블로킹으로 돈다. 웹뷰 이벤트 루프를 막지 않도록 blocking 풀에서 기다린다.
pub(crate) async fn run_excel<R, F>(excel: &Arc<ExcelThread>, f: F) -> CmdResult<R>
where
    R: Send + 'static,
    F: FnOnce(&mut Session) -> excel_com::Result<R> + Send + 'static,
{
    collect_excel::run_excel(excel, f).await
}

async fn on_excel<R, F>(state: &AppState, f: F) -> CmdResult<R>
where
    R: Send + 'static,
    F: FnOnce(&mut Session) -> excel_com::Result<R> + Send + 'static,
{
    run_excel(&state.excel, f).await
}

#[tauri::command]
async fn excel_list_workbooks(state: State<'_, AppState>) -> CmdResult<ExcelStatus> {
    on_excel(&state, |s| s.list_workbooks()).await
}

#[tauri::command]
async fn excel_open_file(state: State<'_, AppState>, path: PathBuf) -> CmdResult<WorkbookInfo> {
    on_excel(&state, move |s| s.open_file(&path)).await
}

#[tauri::command]
async fn excel_inspect_sheet(
    state: State<'_, AppState>,
    workbook_id: String,
    sheet: String,
) -> CmdResult<SheetInspection> {
    on_excel(&state, move |s| s.inspect_sheet(&workbook_id, &sheet)).await
}

#[derive(Serialize, TS)]
#[ts(export)]
struct ImportResult {
    model: SheetModel,
    stats: ReadStats,
}

/// 시트를 읽어 그 업무의 미리보기로 둔다. 앞서 진행 중이던 읽기(다른 업무의 것도)는 취소한다(Excel 스레드는 한 번에
/// 하나만 처리한다).
#[tauri::command]
async fn excel_read_sheet(
    state: State<'_, AppState>,
    job: String,
    workbook_id: String,
    sheet: String,
    options: ReadOptions,
    on_progress: Channel<ReadProgress>,
) -> CmdResult<ImportResult> {
    let job = state.job(&job)?;
    let cancel = Arc::new(AtomicBool::new(false));
    if let Some(previous) = state.read_cancel.lock().unwrap().replace(cancel.clone()) {
        previous.store(true, Ordering::Relaxed);
    }
    let flag = cancel.clone();
    let (id, name) = (workbook_id.clone(), sheet.clone());
    let (model, stats) = on_excel(&state, move |s| {
        s.read_sheet(&id, &name, &options, &mut |p| {
            let _ = on_progress.send(p);
            !flag.load(Ordering::Relaxed)
        })
    })
    .await?;
    let mut pending = job.pending.lock().await;
    let previews = match pending.take() {
        Some(p) if p.workbook_id == workbook_id => pending.insert(p),
        _ => pending.insert(Previews { workbook_id, sheets: Default::default() }),
    };
    previews.sheets.insert(sheet, model.clone());
    Ok(ImportResult { model, stats })
}

#[tauri::command]
fn excel_cancel_read(state: State<'_, AppState>) {
    if let Some(flag) = state.read_cancel.lock().unwrap().take() {
        flag.store(true, Ordering::Relaxed);
    }
}

/// 확정할 시트 하나(미리보기로 읽은 것)와 다시 읽지 않아도 되는 조정.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CommitSheet {
    sheet: String,
    header_rows: u32,
    frozen_cols: u32,
}

/// 미리보기로 읽은 시트들을 확정한다(통합문서 순서대로 넘긴다). 다시 읽지 않아도 되는 조정(머리글 행 수, 틀 고정 열)은
/// 여기서 반영한다. 설정 초안은 시트마다 만든다(공통 설정 — 제목 등 — 은 첫 시트 것을 모든 시트가 같이 쓴다).
///
/// 그 업무에서 처음 확정하면 세션을 만들고, 다시 확정하면 세션의 시트·설정을 바꾼다(공개 중이면 접속 중인 참여자도
/// 새 시트를 받고, 명단에서 빠진 사람은 끊긴다). 수식이 있으면 그 업무의 계산 사본을 뒤에서 연다.
#[tauri::command]
async fn import_commit(
    app: AppHandle,
    state: State<'_, AppState>,
    job: String,
    workbook_id: String,
    sheets: Vec<CommitSheet>,
) -> CmdResult<Vec<SheetModel>> {
    if sheets.is_empty() {
        return Err("불러올 시트를 하나 이상 골라 주세요".into());
    }
    let job = state.job(&job)?;
    let pending = job.pending.lock().await;
    let previews = pending.as_ref().filter(|p| p.workbook_id == workbook_id).ok_or("먼저 시트를 불러와 주세요")?;
    let mut parts = Vec::with_capacity(sheets.len());
    for c in &sheets {
        let mut model =
            previews.sheets.get(&c.sheet).ok_or_else(|| format!("‘{}’ 시트를 먼저 읽어 주세요", c.sheet))?.clone();
        model.header_rows = c.header_rows.min(model.rows);
        model.frozen_cols = c.frozen_cols.min(model.cols);
        // 다시 불러오면 설정 초안도 새 시트에 맞춰 다시 만든다.
        let config = CollectConfig::suggest(&model);
        parts.push((model, config));
    }
    drop(pending);
    let models: Vec<SheetModel> = parts.iter().map(|(m, _)| m.clone()).collect();
    let names: Vec<String> = sheets.iter().map(|c| c.sheet.clone()).collect();

    if job.ending() {
        return Err(jobs::NO_JOB.into());
    }
    // 셀 변경 기록·스냅샷은 확정할 때마다 새 파일에 남긴다(7단계 복구의 바탕).
    let store = open_store(&app, &job.id, &workbook_id, &names);
    let existing = job.session.lock().unwrap().clone();
    let session = match existing {
        Some(session) => {
            session.set_recalc(None);
            session
                .with(move |s| {
                    s.replace(parts);
                    if let Some(store) = store {
                        s.set_store(store);
                    }
                })
                .await?;
            session
        }
        None => {
            let session = SessionHandle::spawn(SessionInit { sheets: parts, store, restore: None });
            *job.session.lock().unwrap() = Some(session.clone());
            jobs::attach(&app, &state, &job, &session).await;
            session
        }
    };
    if job.ending() {
        // 확정하는 사이 업무를 끝냈다: 새로 만든 세션을 놓는다(기록 파일에는 '끝냄'을 남긴다).
        job.session.lock().unwrap().take();
        let _ = session.with(|s| s.set_meta(collect_server::META_ENDED, "1")).await;
        return Err(jobs::NO_JOB.into());
    }
    let source = collect_excel::CalcSource::Workbook(workbook_id.clone());
    job.recalc.start(state.excel.clone(), &job.id, session, source, &names, recalc::notify(&app, &job.id));
    *job.source.lock().unwrap() = Some(save::Source { workbook_id, sheets: names });
    job.last_save.lock().unwrap().take();
    jobs::emit_jobs(&app);
    Ok(models)
}

/// `%LOCALAPPDATA%/<앱>/sessions/YYYYMMDD-HHMMSS-mmm.sqlite`. 열 수 없으면 기록은 메모리에만 둔다.
fn open_store(app: &AppHandle, job: &str, workbook_id: &str, sheets: &[String]) -> Option<collect_store::Store> {
    let dir = recovery::sessions_dir(app)?;
    let _ = std::fs::create_dir_all(&dir);
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    match collect_store::Store::open(recovery::new_file(&dir)) {
        Ok(store) => {
            store.meta(collect_server::META_JOB, job);
            store.meta("workbook", workbook_id);
            // 여러 시트 전 앱도 읽을 수 있게 첫 시트는 `sheet`에도 남긴다.
            store.meta("sheet", sheets.first().map_or("", String::as_str));
            store.meta(collect_server::session::META_SHEETS, &serde_json::to_string(sheets).unwrap_or_default());
            store.meta("created", &stamp);
            Some(store)
        }
        Err(e) => {
            tracing::warn!("기록 파일을 열 수 없습니다(메모리에만 둡니다): {e}");
            None
        }
    }
}

#[tauri::command]
fn lan_addresses() -> Vec<LanAddress> {
    collect_server::lan_addresses()
}

pub fn run() {
    tracing_subscriber::fmt().with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into())).init();
    let excel = Arc::new(ExcelThread::spawn().expect("Excel 스레드를 시작할 수 없습니다"));

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState {
            excel,
            jobs: Default::default(),
            hub: Hub::new(),
            server: Mutex::new(None),
            port: std::sync::Mutex::new(8080),
            read_cancel: std::sync::Mutex::new(None),
            ip_names: std::sync::Mutex::new(collect_server::IpNames::default()),
            restored: tokio::sync::OnceCell::new(),
            last_jobs: Default::default(),
            agents: Default::default(),
            remotes: Default::default(),
        })
        .invoke_handler(tauri::generate_handler![
            excel_list_workbooks,
            excel_open_file,
            excel_inspect_sheet,
            excel_read_sheet,
            excel_cancel_read,
            import_commit,
            host::config_view_get,
            host::config_apply,
            host::sheet_format,
            host::sheet_structure,
            host::sheet_set_cells,
            lan_addresses,
            recalc::recalc_status,
            host::monitor_get,
            host::monitor_progress,
            host::monitor_activity,
            host::comms_get,
            host::notice_send,
            host::highlight_add,
            host::highlight_clear,
            host::chat_send,
            host::chat_react,
            host::history_query,
            host::history_timeline,
            host::history_revert,
            host::history_revert_batch,
            host::history_set_value,
            host::history_export_csv,
            host::snapshot_list,
            host::snapshot_take,
            host::snapshot_diff,
            host::snapshot_restore,
            save::save_info,
            save::save_check,
            save::save_run,
            host::collect_close,
            save::reveal_file,
            jobs::jobs_list,
            jobs::job_create,
            jobs::job_state,
            jobs::job_end,
            jobs::jobs_restore,
            service::service_publish,
            service::service_unpublish,
            service::service_info,
            service::server_status,
            host::participants_get,
            host::participant_approve,
            host::participant_reject,
            host::participant_kick,
            host::participant_unlock,
            people::ip_names_get,
            people::ip_names_set,
            people::ip_names_put,
            people::ip_names_import,
            people::ip_names_export,
            agent::agent_last_addr,
            agent::agent_connect,
            agent::agent_folders,
            agent::agent_services,
            agent::agent_refresh,
            agent::agent_finish,
            agent::agent_retry,
            agent::agent_forget,
            handoff::agent_handoff_check,
            handoff::agent_handoff,
        ])
        .setup(|app| {
            // 오래된 기록 파일 정리(최근 것만 남긴다).
            if let Some(dir) = recovery::sessions_dir(app.handle()) {
                recovery::cleanup(&dir);
            }
            let state = app.state::<AppState>();
            let names = people::load(app.handle());
            state.hub.set_ip_names(names.clone());
            *state.ip_names.lock().unwrap() = names;
            // 에이전트로 넘긴 서비스: 기억을 읽고 상태를 묻기 시작한다.
            state.agents.load(app.handle());
            agent::spawn_poller(app.handle().clone());
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("앱을 만들 수 없습니다");

    app.run(|app, event| {
        if let RunEvent::Exit = event {
            // 프로세스가 끝나면 Drop이 돌지 않을 수 있어, 앱이 띄운 숨은 Excel(파일 열기·모든 업무의 계산 사본)을 여기서
            // 닫는다. 공개 상태는 기록 파일에 그대로 두어 다시 켜면 다시 공개한다.
            let _ = app.state::<AppState>().excel.run(|s| {
                s.close_all_calc();
                s.close_background();
            });
        }
    });
}
