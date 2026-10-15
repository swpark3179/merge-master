//! 4 진행 모니터링(1d)과 1i 소통 패널: 지금 시트·진행률·활동 피드, 셀 변경·소통 이벤트, 공지·강조·채팅 보내기.

use std::time::Duration;

use collect_core::access::ProgressStats;
use collect_core::history::{self, ChangeKind, HistoryFilter, HistoryPage};
use collect_core::view::ConfigView;
use collect_core::{CellRange, SheetModel};
use collect_server::protocol::{ChatReaction, ChatTo};
use collect_server::{CommsSnapshot, HighlightAudience, NoticeTarget, SessionHandle};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::broadcast::error::RecvError;
use ts_rs::TS;

use crate::jobs::event;
use crate::{AppState, CmdResult};

/// '최근 삭제된 셀' 필터가 보는 시간.
const RECENT_DELETE: Duration = Duration::from_secs(60 * 60);

#[derive(Serialize, TS)]
#[ts(export)]
pub struct MonitorData {
    /// 지금 시트(참여자 입력이 반영된 값).
    pub model: SheetModel,
    /// 호스트 시점 설정 보기(셀 상태·명단·행 담당자·진행률·열 이름).
    pub view: ConfigView,
}

/// 그 업무의 셀 변경(`sheet-changes:{업무}`)과 소통(`comms:{업무}`)을 웹뷰에 보낸다. 세션이 끝나면 함께 끝난다.
/// 마감 동작이 '공유 닫기'인 업무는 마감 시각이 되면(`DeadlineClose`) 이 업무의 공유를 닫는다.
pub(crate) fn forward_events(app: &AppHandle, job: &str, session: &SessionHandle) {
    let mut feed = session.host_events();
    let (app2, changes, id) = (app.clone(), event("sheet-changes", job), job.to_string());
    tauri::async_runtime::spawn(async move {
        loop {
            match feed.recv().await {
                Ok(collect_server::HostEvent::DeadlineClose) => {
                    let state = app2.state::<AppState>();
                    if let Ok(job) = state.job(&id)
                        && job.published()
                        && let Err(e) =
                            crate::service::unpublish_as(&app2, &state, &job, collect_server::Gone::Deadline).await
                    {
                        tracing::error!("마감 시각에 공유를 닫지 못했습니다: {e}");
                    }
                    let _ = app2.emit(&changes, collect_server::HostEvent::DeadlineClose);
                }
                Ok(ev) => {
                    let _ = app2.emit(&changes, ev);
                }
                // 너무 많이 밀렸다: 화면이 시트를 다시 읽게 한다.
                Err(RecvError::Lagged(_)) => {
                    let _ = app2.emit(&changes, collect_server::HostEvent::Reset);
                }
                Err(RecvError::Closed) => break,
            }
        }
    });
    let mut comms = session.subscribe_comms();
    let (app3, name) = (app.clone(), event("comms", job));
    tauri::async_runtime::spawn(async move {
        // 붙을 때 이미 있던 글(되살린 기록)에는 알리지 않는다.
        let mut last_chat = last_participant_chat(&comms.borrow());
        while comms.changed().await.is_ok() {
            let snap = comms.borrow_and_update().clone();
            let newest = last_participant_chat(&snap);
            if newest > last_chat {
                last_chat = newest;
                request_attention(&app3);
            }
            let _ = app3.emit(&name, snap);
        }
    });
}

/// 참여자가 보낸 마지막 채팅 번호(없으면 0).
fn last_participant_chat(comms: &CommsSnapshot) -> u32 {
    comms.chat.iter().filter(|m| !m.host).map(|m| m.id).max().unwrap_or(0)
}

/// 참여자 채팅이 왔는데 창이 뒤에 있으면 작업 표시줄 단추를 깜빡인다(Windows: 창이 앞으로 올 때까지).
fn request_attention(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main")
        && !window.is_focused().unwrap_or(false)
    {
        let _ = window.request_user_attention(Some(tauri::UserAttentionType::Informational));
    }
}

/// 그 시트의 지금 값과 호스트 시점 설정 보기.
#[tauri::command]
pub async fn monitor_get(state: State<'_, AppState>, job: String, sheet: usize) -> CmdResult<MonitorData> {
    state
        .session(&job)?
        .with(move |s| {
            let view = s.view(sheet, None)?;
            Ok(MonitorData { model: s.sheets[sheet].model.clone(), view })
        })
        .await?
}

/// 그 시트의 필수 진행률(세션이 센 것, 사람 인덱스 = 명단 색).
#[tauri::command]
pub async fn monitor_progress(state: State<'_, AppState>, job: String, sheet: usize) -> CmdResult<ProgressStats> {
    state.session(&job)?.with(move |s| s.progress(sheet).cloned().ok_or_else(|| "없는 시트입니다".to_string())).await?
}

#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Activity {
    /// 모든 시트의 최근 묶음(묶음마다 시트).
    pub page: HistoryPage,
    /// 그 시트에서 최근 1시간 안에 지워져 지금도 빈 칸이 있는 행('최근 삭제된 셀' 필터).
    pub deleted_rows: Vec<u32>,
    /// 그 시트에서 행 담당자가 없는 행 중 사람마다(명단 인덱스) 그 사람이 넣은 값이 남아 있는 행('담당자별' 필터).
    pub open_rows_by: Vec<Vec<u32>>,
}

/// 실시간 활동 피드: 최근 묶음(빨간 묶음 = 삭제가 든 여러 칸·큰 묶음). `sheet`는 '최근 삭제된 셀'을 볼 시트.
#[tauri::command]
pub async fn monitor_activity(
    state: State<'_, AppState>,
    job: String,
    sheet: usize,
    limit: Option<u32>,
) -> CmdResult<Activity> {
    let limit = limit.unwrap_or(30) as usize;
    let since = chrono::Utc::now().timestamp_millis() - RECENT_DELETE.as_millis() as i64;
    state
        .session(&job)?
        .with(move |s| {
            let page = history::query(s.history(), &HistoryFilter::default(), 0, limit);
            let mut rows: Vec<u32> = s
                .history()
                .iter()
                .filter(|e| {
                    e.kind == ChangeKind::Delete
                        && e.sheet as usize == sheet
                        && e.at_ms >= since
                        && s.sheets.get(sheet).and_then(|x| x.model.cell(e.at())).is_some_and(|c| c.v.is_empty())
                })
                .map(|e| e.row)
                .collect();
            rows.sort_unstable();
            rows.dedup();
            Activity { page, deleted_rows: rows, open_rows_by: s.open_rows_by(sheet) }
        })
        .await
}

// ------------------------------------------------------------ 1i 소통

#[tauri::command]
pub fn comms_get(state: State<'_, AppState>, job: String) -> CmdResult<CommsSnapshot> {
    Ok(state.session(&job)?.comms())
}

#[tauri::command]
pub async fn notice_send(
    state: State<'_, AppState>,
    job: String,
    text: String,
    important: bool,
    target: NoticeTarget,
) -> CmdResult<()> {
    if text.trim().is_empty() {
        return Err("공지 내용을 적어 주세요".into());
    }
    state.session(&job)?.with(move |s| s.send_notice(&text, important, target)).await
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
) -> CmdResult<()> {
    state.session(&job)?.with(move |s| s.highlight(sheet, range, &note, audience)).await
}

#[tauri::command]
pub async fn highlight_clear(state: State<'_, AppState>, job: String, id: u32) -> CmdResult<()> {
    state.session(&job)?.with(move |s| s.clear_highlight(id)).await
}

#[tauri::command]
pub async fn chat_send(state: State<'_, AppState>, job: String, to: ChatTo, text: String) -> CmdResult<()> {
    if text.trim().is_empty() {
        return Ok(());
    }
    state.session(&job)?.with(move |s| s.chat(to, &text)).await
}

/// 참여자 채팅에 리액션을 달거나 뗀다(같은 종류를 다시 누르면 뗀다).
#[tauri::command]
pub async fn chat_react(state: State<'_, AppState>, job: String, id: u32, kind: ChatReaction) -> CmdResult<()> {
    state.session(&job)?.with(move |s| s.chat_react(id, kind)).await
}
