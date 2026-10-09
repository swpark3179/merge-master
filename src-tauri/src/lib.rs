//! 호스트 앱(Tauri) 진입점: Excel 스레드·취합 세션·참여자 서버를 앱 상태로 들고, 웹뷰에 command로 노출한다.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use collect_core::SheetModel;
use collect_core::config::CollectConfig;
use collect_core::edit::ConfigEdit;
use collect_core::format::{FormatEdit, apply_format};
use collect_core::view::{ConfigView, config_view};
use collect_server::{LanAddress, ServerHandle, SessionHandle, SessionInit};
use excel_com::{
    ExcelStatus, ExcelThread, ReadOptions, ReadProgress, ReadStats, Session, SheetInspection, WorkbookInfo,
};
use serde::Serialize;
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, RunEvent, State};
use tokio::sync::Mutex;
use ts_rs::TS;

mod history;
mod monitor;
mod recalc;
mod recovery;
mod save;
mod service;

pub(crate) struct AppState {
    excel: Arc<ExcelThread>,
    /// 1a에서 읽었지만 아직 확정하지 않은 시트(미리보기)와 그 출처.
    pending: Mutex<Option<Preview>>,
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
}

pub(crate) struct Preview {
    model: SheetModel,
    workbook_id: String,
    sheet: String,
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
    *state.pending.lock().await = Some(Preview { model: model.clone(), workbook_id, sheet });
    Ok(ImportResult { model, stats })
}

#[tauri::command]
fn excel_cancel_read(state: State<'_, AppState>) {
    if let Some(flag) = state.read_cancel.lock().unwrap().take() {
        flag.store(true, Ordering::Relaxed);
    }
}

/// 미리보기로 읽은 시트를 확정한다. 다시 읽지 않아도 되는 조정(머리글 행 수, 틀 고정 열)은 여기서 반영한다.
///
/// 처음 확정하면 세션을 만들고, 다시 확정하면 세션의 시트·설정을 바꾼다(서비스 중이면 접속 중인 참여자도 새 시트를
/// 받고, 명단에서 빠진 사람은 끊긴다). 수식이 있으면 계산 사본을 뒤에서 연다.
#[tauri::command]
async fn import_commit(
    app: AppHandle,
    state: State<'_, AppState>,
    header_rows: u32,
    frozen_cols: u32,
) -> CmdResult<SheetModel> {
    let preview = state.pending.lock().await;
    let preview = preview.as_ref().ok_or("먼저 시트를 불러와 주세요")?;
    let mut model = preview.model.clone();
    model.header_rows = header_rows.min(model.rows);
    model.frozen_cols = frozen_cols.min(model.cols);
    // 다시 불러오면 설정 초안도 새 시트에 맞춰 다시 만든다.
    let config = CollectConfig::suggest(&model);

    // 셀 변경 기록·스냅샷은 확정할 때마다 새 파일에 남긴다(7단계 복구의 바탕).
    let store = open_store(&app, &preview.workbook_id, &preview.sheet);
    let existing = state.session.lock().unwrap().clone();
    let session = match existing {
        Some(session) => {
            session.set_recalc(None);
            let (m, c) = (model.clone(), config);
            session
                .with(move |s| {
                    s.replace(m, c);
                    if let Some(store) = store {
                        s.set_store(store);
                    }
                })
                .await?;
            session
        }
        None => {
            let session = SessionHandle::spawn(SessionInit { model: model.clone(), config, store, restore: None });
            service::forward_participants(&app, &session);
            monitor::forward_events(&app, &session);
            *state.session.lock().unwrap() = Some(session.clone());
            session
        }
    };
    state.recalc.start(&app, state.excel.clone(), session, &model, &preview.workbook_id, &preview.sheet);
    *state.source.lock().unwrap() =
        Some(save::Source { workbook_id: preview.workbook_id.clone(), sheet: preview.sheet.clone() });
    state.last_save.lock().unwrap().take();
    Ok(model)
}

/// `%LOCALAPPDATA%/<앱>/sessions/YYYYMMDD-HHMMSS.sqlite`. 열 수 없으면 기록은 메모리에만 둔다.
fn open_store(app: &AppHandle, workbook_id: &str, sheet: &str) -> Option<collect_store::Store> {
    let dir = recovery::sessions_dir(app)?;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    match collect_store::Store::open(dir.join(format!("{stamp}.sqlite"))) {
        Ok(store) => {
            store.meta("workbook", workbook_id);
            store.meta("sheet", sheet);
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

/// 1b 화면이 그릴 설정 보기. `viewer`를 주면 그 참여자 시점으로 계산한다(참여자 시점으로 보기).
#[tauri::command]
async fn config_view_get(state: State<'_, AppState>, viewer: Option<String>) -> CmdResult<ConfigView> {
    state.session()?.with(move |s| config_view(s.model, s.config, viewer.as_deref()).map_err(|e| e.to_string())).await?
}

/// 미리보던 사람이 명단에서 빠졌으면(범위 배정을 지움 등) 호스트 시점으로 돌아간다.
fn view_or_host(model: &SheetModel, config: &CollectConfig, viewer: Option<&str>) -> CmdResult<ConfigView> {
    config_view(model, config, viewer).or_else(|_| config_view(model, config, None)).map_err(|e| e.to_string())
}

#[tauri::command]
async fn config_apply(state: State<'_, AppState>, edit: ConfigEdit, viewer: Option<String>) -> CmdResult<ConfigView> {
    state
        .session()?
        .with(move |s| {
            s.config.apply(s.model, edit).map_err(|e| e.to_string())?;
            s.changed();
            view_or_host(s.model, s.config, viewer.as_deref())
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
async fn sheet_format(state: State<'_, AppState>, edit: FormatEdit, viewer: Option<String>) -> CmdResult<FormatResult> {
    state
        .session()?
        .with(move |s| {
            apply_format(s.model, edit).map_err(|e| e.to_string())?;
            s.changed();
            let view = view_or_host(s.model, s.config, viewer.as_deref())?;
            Ok(FormatResult { model: s.model.clone(), view })
        })
        .await?
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
            service::qr_svg,
            service::firewall_check,
            service::firewall_add,
        ])
        .setup(|app| {
            // 오래된 기록 파일 정리(최근 것만 남긴다).
            if let Some(dir) = recovery::sessions_dir(app.handle()) {
                recovery::cleanup(&dir);
            }
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
