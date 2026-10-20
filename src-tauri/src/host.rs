//! 업무에 딸린 호스트 command(1b 설정·4 모니터링·1i 소통·5 히스토리·1c 참여자·6 취합 마감). 하는 일은
//! [`collect_server::host`]가 하고, 여기서는 그 업무의 세션을 찾아 요청을 넘긴다(command 이름·인수는 그대로).
//! 결과는 JSON 그대로 웹뷰에 간다(TS 타입은 `@mm/shared`의 생성 타입).

use std::path::PathBuf;

use collect_core::edit::ConfigEdit;
use collect_core::format::FormatEdit;
use collect_core::history::HistoryFilter;
use collect_core::structure::StructureEdit;
use collect_core::{CellRange, CellRef, CellValue};
use collect_server::host::{CellWrite, CsvExport, HostRequest};
use collect_server::protocol::{ChatReaction, ChatTo};
use collect_server::{HighlightAudience, NoticeTarget};
use serde_json::Value;
use tauri::{AppHandle, Emitter, State};

use crate::jobs::event;
use crate::{AppState, CmdResult};

/// 그 업무의 세션에서 요청 하나를 한다.
pub(crate) async fn run(state: &AppState, job: &str, req: HostRequest) -> CmdResult<Value> {
    collect_server::host::execute(&state.session(job)?, req).await
}

/// 1b 화면이 그릴 설정 보기(그 시트). `viewer`를 주면 그 참여자 시점으로 계산한다(참여자 시점으로 보기).
#[tauri::command]
pub async fn config_view_get(
    state: State<'_, AppState>,
    job: String,
    sheet: usize,
    viewer: Option<String>,
) -> CmdResult<Value> {
    run(&state, &job, HostRequest::ConfigView { sheet, viewer }).await
}

/// 1b 설정 편집(그 시트). 제목·안내·마감, 직접 넣은 명단은 모든 시트가 같이 쓴다.
#[tauri::command]
pub async fn config_apply(
    state: State<'_, AppState>,
    job: String,
    sheet: usize,
    edit: ConfigEdit,
    viewer: Option<String>,
) -> CmdResult<Value> {
    run(&state, &job, HostRequest::ConfigApply { sheet, edit, viewer }).await
}

/// 서식 도구 줄·크기 조절(1b, 열 너비·행 높이는 1d에서도).
#[tauri::command]
pub async fn sheet_format(
    state: State<'_, AppState>,
    job: String,
    sheet: usize,
    edit: FormatEdit,
    viewer: Option<String>,
) -> CmdResult<Value> {
    run(&state, &job, HostRequest::SheetFormat { sheet, edit, viewer }).await
}

/// 표 구조 바꾸기(1b·1d 오른쪽 클릭 메뉴): 표 아래에 행·오른쪽에 열 붙이기, 행 지우기(엑셀에서 불러온 행도), 앱에서
/// 붙인 열 지우기, 행 취소 표시. 지울 수 없으면 이유를 돌려준다. 바뀐 시트와 설정 보기를 돌려준다.
#[tauri::command]
pub async fn sheet_structure(
    state: State<'_, AppState>,
    job: String,
    sheet: usize,
    edit: StructureEdit,
    viewer: Option<String>,
) -> CmdResult<Value> {
    run(&state, &job, HostRequest::SheetStructure { sheet, edit, viewer }).await
}

/// 호스트가 칸 내용을 바꾼다(1b 설정·4 모니터링 그리드, 그 시트). 참여자 입력처럼 버전·이력·진행률에 반영되고 모두에게
/// 간다. 같은 값은 세션이 건너뛴다(수식 칸은 같은 값이어도 수식을 그 값으로 바꾼다).
#[tauri::command]
pub async fn sheet_set_cells(
    state: State<'_, AppState>,
    job: String,
    sheet: usize,
    cells: Vec<CellWrite>,
) -> CmdResult<Value> {
    run(&state, &job, HostRequest::SheetSetCells { sheet, cells }).await
}

/// 그 시트의 지금 값과 호스트 시점 설정 보기.
#[tauri::command]
pub async fn monitor_get(state: State<'_, AppState>, job: String, sheet: usize) -> CmdResult<Value> {
    run(&state, &job, HostRequest::MonitorGet { sheet }).await
}

/// 그 시트의 필수 진행률(세션이 센 것, 사람 인덱스 = 명단 색).
#[tauri::command]
pub async fn monitor_progress(state: State<'_, AppState>, job: String, sheet: usize) -> CmdResult<Value> {
    run(&state, &job, HostRequest::MonitorProgress { sheet }).await
}

/// 실시간 활동 피드: 최근 묶음(빨간 묶음 = 삭제가 든 여러 칸·큰 묶음). `sheet`는 '최근 삭제된 셀'을 볼 시트.
#[tauri::command]
pub async fn monitor_activity(
    state: State<'_, AppState>,
    job: String,
    sheet: usize,
    limit: Option<u32>,
) -> CmdResult<Value> {
    run(&state, &job, HostRequest::MonitorActivity { sheet, limit }).await
}

#[tauri::command]
pub async fn comms_get(state: State<'_, AppState>, job: String) -> CmdResult<Value> {
    run(&state, &job, HostRequest::CommsGet).await
}

#[tauri::command]
pub async fn notice_send(
    state: State<'_, AppState>,
    job: String,
    text: String,
    important: bool,
    target: NoticeTarget,
) -> CmdResult<Value> {
    run(&state, &job, HostRequest::NoticeSend { text, important, target }).await
}

/// `range`는 그 시트의 모델 좌표.
#[tauri::command]
pub async fn highlight_add(
    state: State<'_, AppState>,
    job: String,
    sheet: usize,
    range: CellRange,
    note: String,
    audience: HighlightAudience,
) -> CmdResult<Value> {
    run(&state, &job, HostRequest::HighlightAdd { sheet, range, note, audience }).await
}

#[tauri::command]
pub async fn highlight_clear(state: State<'_, AppState>, job: String, id: u32) -> CmdResult<Value> {
    run(&state, &job, HostRequest::HighlightClear { id }).await
}

#[tauri::command]
pub async fn chat_send(state: State<'_, AppState>, job: String, to: ChatTo, text: String) -> CmdResult<Value> {
    run(&state, &job, HostRequest::ChatSend { to, text }).await
}

/// 참여자 채팅에 리액션을 달거나 뗀다(같은 종류를 다시 누르면 뗀다).
#[tauri::command]
pub async fn chat_react(state: State<'_, AppState>, job: String, id: u32, kind: ChatReaction) -> CmdResult<Value> {
    run(&state, &job, HostRequest::ChatReact { id, kind }).await
}

#[tauri::command]
pub async fn history_query(
    state: State<'_, AppState>,
    job: String,
    filter: HistoryFilter,
    offset: u32,
    limit: u32,
) -> CmdResult<Value> {
    run(&state, &job, HostRequest::HistoryQuery { filter, offset, limit }).await
}

#[tauri::command]
pub async fn history_timeline(state: State<'_, AppState>, job: String, sheet: u32, at: CellRef) -> CmdResult<Value> {
    run(&state, &job, HostRequest::HistoryTimeline { sheet, at }).await
}

/// 칸 하나(또는 몇 개)의 기록을 되돌린다.
#[tauri::command]
pub async fn history_revert(state: State<'_, AppState>, job: String, seqs: Vec<u64>, force: bool) -> CmdResult<Value> {
    run(&state, &job, HostRequest::HistoryRevert { seqs, force }).await
}

/// 묶음 전체를 되돌린다(범위 지우기·붙여넣기 '묶음 복원').
#[tauri::command]
pub async fn history_revert_batch(
    state: State<'_, AppState>,
    job: String,
    batch: String,
    force: bool,
) -> CmdResult<Value> {
    run(&state, &job, HostRequest::HistoryRevertBatch { batch, force }).await
}

/// 셀 타임라인의 '이 값으로'.
#[tauri::command]
pub async fn history_set_value(
    state: State<'_, AppState>,
    job: String,
    sheet: u32,
    at: CellRef,
    value: CellValue,
) -> CmdResult<Value> {
    run(&state, &job, HostRequest::HistorySetValue { sheet, at, value }).await
}

/// CSV로 내보낸다(필터에 걸린 기록 전부, 이 PC 시간대). 쓴 줄 수를 돌려준다.
#[tauri::command]
pub async fn history_export_csv(
    state: State<'_, AppState>,
    job: String,
    filter: HistoryFilter,
    path: PathBuf,
) -> CmdResult<u32> {
    let req = HostRequest::HistoryCsv { filter, offset_minutes: crate::save::utc_offset_minutes() };
    let out: CsvExport = serde_json::from_value(run(&state, &job, req).await?).map_err(|e| e.to_string())?;
    std::fs::write(&path, out.csv).map_err(|e| format!("파일을 쓸 수 없습니다: {e}"))?;
    Ok(out.count)
}

#[tauri::command]
pub async fn snapshot_list(state: State<'_, AppState>, job: String) -> CmdResult<Value> {
    run(&state, &job, HostRequest::SnapshotList).await
}

#[tauri::command]
pub async fn snapshot_take(state: State<'_, AppState>, job: String, label: String) -> CmdResult<Value> {
    run(&state, &job, HostRequest::SnapshotTake { label }).await
}

#[tauri::command]
pub async fn snapshot_diff(state: State<'_, AppState>, job: String, id: u32) -> CmdResult<Value> {
    run(&state, &job, HostRequest::SnapshotDiff { id }).await
}

/// 스냅샷 시점으로 복원한다(복원 직전 상태는 자동으로 스냅샷에 남는다). 바꾼 칸 수.
#[tauri::command]
pub async fn snapshot_restore(state: State<'_, AppState>, job: String, id: u32) -> CmdResult<Value> {
    run(&state, &job, HostRequest::SnapshotRestore { id }).await
}

/// 시트를 확정했으면 지금의 접속자 표(아니면 `null`).
#[tauri::command]
pub async fn participants_get(state: State<'_, AppState>, job: String) -> CmdResult<Value> {
    Ok(run(&state, &job, HostRequest::ParticipantsGet).await.unwrap_or(Value::Null))
}

/// 승인 대기 중인 사람을 들인다. `range`는 `sheet` 시트의 주소(예: H500:H620, `인력계획!E5:E9`처럼 시트 이름을 붙여도
/// 된다)로, 주면 그 범위를 배정한다. 설정은 세션이 가진다: 1b 화면은 바뀌었다는 알림을 받고 세션에서 다시 읽는다.
#[tauri::command]
pub async fn participant_approve(
    app: AppHandle,
    state: State<'_, AppState>,
    job: String,
    name: String,
    sheet: u32,
    range: Option<String>,
    note: String,
) -> CmdResult<Value> {
    let out = run(&state, &job, HostRequest::ParticipantApprove { name, sheet, range, note }).await?;
    let _ = app.emit(&event("config-changed", &job), ());
    Ok(out)
}

#[tauri::command]
pub async fn participant_reject(state: State<'_, AppState>, job: String, name: String) -> CmdResult<Value> {
    run(&state, &job, HostRequest::ParticipantReject { name }).await
}

#[tauri::command]
pub async fn participant_kick(state: State<'_, AppState>, job: String, name: String) -> CmdResult<Value> {
    run(&state, &job, HostRequest::ParticipantKick { name }).await
}

/// 그 사람이 잡고 있는 편집 잠금을 푼다(편집기를 열어 둔 채 자리를 비운 경우).
#[tauri::command]
pub async fn participant_unlock(state: State<'_, AppState>, job: String, name: String) -> CmdResult<Value> {
    run(&state, &job, HostRequest::ParticipantUnlock { name }).await
}

/// 취합 마감(참여자 입력을 막음) / 다시 열기.
#[tauri::command]
pub async fn collect_close(state: State<'_, AppState>, job: String, closed: bool) -> CmdResult<Value> {
    run(&state, &job, HostRequest::CollectClose { closed }).await
}
