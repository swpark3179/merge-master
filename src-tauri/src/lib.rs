//! 호스트 앱(Tauri) 진입점: Excel 스레드·취합 세션·참여자 서버를 앱 상태로 들고, 웹뷰에 command로 노출한다.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use collect_core::config::CollectConfig;
use collect_core::edit::ConfigEdit;
use collect_core::format::{FormatEdit, apply_format};
use collect_core::history::BatchOrigin;
use collect_core::view::ConfigView;
use collect_core::{CellRef, CellValue, SheetModel};
use collect_server::{LanAddress, ServerHandle, SessionHandle, SessionInit, Shared};
use excel_com::{
    ExcelStatus, ExcelThread, ReadOptions, ReadProgress, ReadStats, Session, SheetInspection, WorkbookInfo,
};
use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, RunEvent, State};
use tokio::sync::Mutex;
use ts_rs::TS;

mod history;
mod monitor;
mod people;
mod recalc;
mod recovery;
mod save;
mod service;

pub(crate) struct AppState {
    excel: Arc<ExcelThread>,
    /// 1a에서 읽었지만 아직 확정하지 않은 시트들(미리보기, 시트 이름별)과 그 통합문서. 다른 통합문서를 읽으면 비운다.
    pending: Mutex<Option<Previews>>,
    /// 확정한 시트·취합 설정·편집 내용을 가진 세션. 시트를 처음 확정할 때 만들고, 다시 확정하면 내용을 바꾼다.
    session: std::sync::Mutex<Option<SessionHandle>>,
    /// 실행 중인 참여자 서버(세션에 붙었다 떨어진다).
    server: Mutex<Option<ServerHandle>>,
    /// 진행 중인 읽기의 취소 표시. Excel 스레드가 읽는 중이어도 바로 바꿀 수 있게 Excel 스레드 밖에 둔다.
    read_cancel: std::sync::Mutex<Option<Arc<AtomicBool>>>,
    recalc: recalc::Recalc,
    /// 확정한 시트의 출처(저장할 원본).
    source: std::sync::Mutex<Option<save::Source>>,
    last_save: std::sync::Mutex<Option<save::LastSave>>,
    /// IP별 참여자 이름(1c IP 명단, 앱 설정 폴더의 JSON 파일).
    ip_names: std::sync::Mutex<collect_server::IpNames>,
}

pub(crate) struct Previews {
    workbook_id: String,
    sheets: std::collections::HashMap<String, SheetModel>,
}

pub(crate) type CmdResult<T> = Result<T, String>;

pub(crate) const NO_SHEET: &str = "먼저 시트를 불러와 확정해 주세요";

impl AppState {
    pub(crate) fn session(&self) -> CmdResult<SessionHandle> {
        self.session.lock().unwrap().clone().ok_or_else(|| NO_SHEET.into())
    }
}

/// Excel 작업은 STA 스레드에서 블로킹으로 돈다. 웹뷰 이벤트 루프를 막지 않도록 blocking 풀에서 기다린다.
pub(crate) async fn run_excel<R, F>(excel: &Arc<ExcelThread>, f: F) -> CmdResult<R>
where
    R: Send + 'static,
    F: FnOnce(&mut Session) -> excel_com::Result<R> + Send + 'static,
{
    let excel = excel.clone();
    tauri::async_runtime::spawn_blocking(move || excel.run(f))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
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

/// 시트를 읽어 미리보기로 둔다. 앞서 진행 중이던 읽기는 취소한다(Excel 스레드는 한 번에 하나만 처리한다).
#[tauri::command]
async fn excel_read_sheet(
    state: State<'_, AppState>,
    workbook_id: String,
    sheet: String,
    options: ReadOptions,
    on_progress: Channel<ReadProgress>,
) -> CmdResult<ImportResult> {
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
    let mut pending = state.pending.lock().await;
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
/// 처음 확정하면 세션을 만들고, 다시 확정하면 세션의 시트·설정을 바꾼다(서비스 중이면 접속 중인 참여자도 새 시트를
/// 받고, 명단에서 빠진 사람은 끊긴다). 수식이 있으면 계산 사본을 뒤에서 연다.
#[tauri::command]
async fn import_commit(
    app: AppHandle,
    state: State<'_, AppState>,
    workbook_id: String,
    sheets: Vec<CommitSheet>,
) -> CmdResult<Vec<SheetModel>> {
    if sheets.is_empty() {
        return Err("불러올 시트를 하나 이상 골라 주세요".into());
    }
    let pending = state.pending.lock().await;
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

    // 셀 변경 기록·스냅샷은 확정할 때마다 새 파일에 남긴다(7단계 복구의 바탕).
    let store = open_store(&app, &workbook_id, &names);
    let existing = state.session.lock().unwrap().clone();
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
            people::attach(&state, &session);
            service::forward_participants(&app, &session);
            monitor::forward_events(&app, &session);
            *state.session.lock().unwrap() = Some(session.clone());
            session
        }
    };
    state.recalc.start(&app, state.excel.clone(), session, &models, &workbook_id, &names);
    *state.source.lock().unwrap() = Some(save::Source { workbook_id, sheets: names });
    state.last_save.lock().unwrap().take();
    Ok(models)
}

/// `%LOCALAPPDATA%/<앱>/sessions/YYYYMMDD-HHMMSS.sqlite`. 열 수 없으면 기록은 메모리에만 둔다.
fn open_store(app: &AppHandle, workbook_id: &str, sheets: &[String]) -> Option<collect_store::Store> {
    let dir = recovery::sessions_dir(app)?;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    match collect_store::Store::open(dir.join(format!("{stamp}.sqlite"))) {
        Ok(store) => {
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

/// 1b 화면이 그릴 설정 보기(그 시트). `viewer`를 주면 그 참여자 시점으로 계산한다(참여자 시점으로 보기).
#[tauri::command]
async fn config_view_get(state: State<'_, AppState>, sheet: usize, viewer: Option<String>) -> CmdResult<ConfigView> {
    state.session()?.with(move |s| s.view(sheet, viewer.as_deref())).await?
}

/// 미리보던 사람이 명단에서 빠졌으면(범위 배정을 지움 등) 호스트 시점으로 돌아간다. 작업 안에서 바꾼 설정을 바로 본다.
fn view_or_host(s: &Shared<'_>, sheet: usize, viewer: Option<&str>) -> CmdResult<ConfigView> {
    s.view(sheet, viewer).or_else(|_| s.view(sheet, None))
}

/// 1b 설정 편집(그 시트). 제목·안내·마감, 직접 넣은 명단은 모든 시트가 같이 쓴다.
#[tauri::command]
async fn config_apply(
    state: State<'_, AppState>,
    sheet: usize,
    edit: ConfigEdit,
    viewer: Option<String>,
) -> CmdResult<ConfigView> {
    state
        .session()?
        .with(move |s| {
            s.apply(sheet, edit)?;
            view_or_host(s, sheet, viewer.as_deref())
        })
        .await?
}

#[derive(Serialize, TS)]
#[ts(export)]
struct FormatResult {
    model: SheetModel,
    view: ConfigView,
}

/// 서식 도구 줄. 병합이 바뀌면 입력 가능 셀도 바뀌므로 설정 보기를 함께 돌려준다.
#[tauri::command]
async fn sheet_format(
    state: State<'_, AppState>,
    sheet: usize,
    edit: FormatEdit,
    viewer: Option<String>,
) -> CmdResult<FormatResult> {
    state
        .session()?
        .with(move |s| {
            let slot = s.sheets.get_mut(sheet).ok_or("없는 시트입니다")?;
            apply_format(&mut slot.model, edit).map_err(|e| e.to_string())?;
            let model = slot.model.clone();
            s.changed();
            let view = view_or_host(s, sheet, viewer.as_deref())?;
            Ok(FormatResult { model, view })
        })
        .await?
}

/// 호스트가 칸에 넣을 값(모델 좌표). TS: `apps/host/src/api.ts`의 `CellWrite`.
#[derive(Deserialize)]
struct CellWrite {
    row: u32,
    col: u32,
    #[serde(default)]
    value: CellValue,
}

/// 호스트가 칸 내용을 바꾼다(1b 설정·4 모니터링 그리드, 그 시트). 참여자 입력처럼 버전·이력·진행률에 반영되고 모두에게
/// 간다. 수식 칸·같은 값은 세션이 건너뛴다. 머리글(열 이름)이나 행 배정 기준 열(명단)을 바꿨으면 권한을 다시 계산한다 —
/// 같은 작업 안에서는 다시 계산이 값 반영보다 먼저 돌아서 따로 보낸다.
#[tauri::command]
async fn sheet_set_cells(state: State<'_, AppState>, sheet: usize, cells: Vec<CellWrite>) -> CmdResult<()> {
    let session = state.session()?;
    let structural = session
        .with(move |s| {
            let slot = s.sheets.get(sheet).ok_or("없는 시트입니다")?;
            let header_rows = slot.model.header_rows;
            let key = slot.config.assignment.key_col;
            let structural = cells.iter().any(|c| c.row < header_rows || Some(c.col) == key);
            let user = s.host_name().to_string();
            let cells = cells.into_iter().map(|c| (CellRef::new(c.row, c.col), c.value)).collect();
            s.host_set(&user, BatchOrigin::Edit, sheet, cells);
            Ok::<_, String>(structural)
        })
        .await??;
    if structural {
        session.with(|s| s.changed()).await?;
    }
    Ok(())
}

pub fn run() {
    tracing_subscriber::fmt().with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into())).init();
    let excel = Arc::new(ExcelThread::spawn().expect("Excel 스레드를 시작할 수 없습니다"));

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState {
            excel,
            pending: Mutex::new(None),
            session: std::sync::Mutex::new(None),
            server: Mutex::new(None),
            read_cancel: std::sync::Mutex::new(None),
            recalc: recalc::Recalc::default(),
            source: std::sync::Mutex::new(None),
            last_save: std::sync::Mutex::new(None),
            ip_names: std::sync::Mutex::new(collect_server::IpNames::default()),
        })
        .invoke_handler(tauri::generate_handler![
            excel_list_workbooks,
            excel_open_file,
            excel_inspect_sheet,
            excel_read_sheet,
            excel_cancel_read,
            import_commit,
            config_view_get,
            config_apply,
            sheet_format,
            sheet_set_cells,
            lan_addresses,
            recalc::recalc_status,
            monitor::monitor_get,
            monitor::monitor_progress,
            monitor::monitor_activity,
            monitor::comms_get,
            monitor::notice_send,
            monitor::highlight_add,
            monitor::highlight_clear,
            monitor::chat_send,
            history::history_query,
            history::history_timeline,
            history::history_revert,
            history::history_revert_batch,
            history::history_set_value,
            history::history_export_csv,
            history::snapshot_list,
            history::snapshot_take,
            history::snapshot_diff,
            history::snapshot_restore,
            save::save_info,
            save::save_check,
            save::save_run,
            save::collect_close,
            save::reveal_file,
            service::server_start,
            service::server_info,
            recovery::recovery_check,
            recovery::recovery_resume,
            recovery::recovery_discard,
            service::server_stop,
            service::participants_get,
            service::participant_approve,
            service::participant_reject,
            service::participant_kick,
            service::participant_unlock,
            people::ip_names_get,
            people::ip_names_set,
            people::ip_names_put,
            people::ip_names_import,
            people::ip_names_export,
        ])
        .setup(|app| {
            // 오래된 기록 파일 정리(최근 것만 남긴다).
            if let Some(dir) = recovery::sessions_dir(app.handle()) {
                recovery::cleanup(&dir);
            }
            *app.state::<AppState>().ip_names.lock().unwrap() = people::load(app.handle());
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("앱을 만들 수 없습니다");

    app.run(|app, event| {
        if let RunEvent::Exit = event {
            // 프로세스가 끝나면 Drop이 돌지 않을 수 있어, 앱이 띄운 숨은 Excel(파일 열기·계산 사본)을 여기서 닫는다.
            let _ = app.state::<AppState>().excel.run(|s| {
                s.close_calc();
                s.close_background();
            });
        }
    });
}
