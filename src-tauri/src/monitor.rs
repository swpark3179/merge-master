//! 4 진행 모니터링(1d)과 1i 소통 패널: 지금 시트·진행률·활동 피드, 셀 변경·소통 이벤트, 공지·강조·채팅 보내기.

use std::time::Duration;

use collect_core::access::ProgressStats;
use collect_core::history::{self, ChangeKind, HistoryFilter, HistoryPage};
use collect_core::view::{ConfigView, config_view};
use collect_core::{CellRange, SheetModel};
use collect_server::protocol::ChatTo;
use collect_server::{CommsSnapshot, HighlightAudience, NoticeTarget, SessionHandle};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::broadcast::error::RecvError;
use ts_rs::TS;

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

/// 셀 변경(`sheet-changes`)과 소통(`comms`)을 웹뷰에 보낸다. 세션이 끝나면 함께 끝난다.
pub(crate) fn forward_events(app: &AppHandle, session: &SessionHandle) {
    let mut feed = session.host_events();
    let app2 = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            match feed.recv().await {
                Ok(event) => {
                    let _ = app2.emit("sheet-changes", event);
                }
                // 너무 많이 밀렸다: 화면이 시트를 다시 읽게 한다.
                Err(RecvError::Lagged(_)) => {
                    let _ = app2.emit("sheet-changes", collect_server::HostEvent::Reset);
                }
                Err(RecvError::Closed) => break,
            }
        }
    });
    let mut comms = session.subscribe_comms();
    let app3 = app.clone();
    tauri::async_runtime::spawn(async move {
        while comms.changed().await.is_ok() {
            let snap = comms.borrow_and_update().clone();
            let _ = app3.emit("comms", snap);
        }
    });
}

#[tauri::command]
pub async fn monitor_get(state: State<'_, AppState>) -> CmdResult<MonitorData> {
    state
        .session()?
        .with(|s| {
            let view = config_view(s.model, s.config, None).map_err(|e| e.to_string())?;
            Ok(MonitorData { model: s.model.clone(), view })
        })
        .await?
}

#[tauri::command]
pub async fn monitor_progress(state: State<'_, AppState>) -> CmdResult<ProgressStats> {
    state.session()?.with(|s| collect_core::access::Access::compile(s.model, s.config).progress(s.model)).await
}

#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Activity {
    pub page: HistoryPage,
    /// 최근 1시간 안에 지워져 지금도 빈 칸이 있는 행('최근 삭제된 셀' 필터).
    pub deleted_rows: Vec<u32>,
}

/// 실시간 활동 피드: 최근 묶음(빨간 묶음 = 삭제가 든 여러 칸·큰 묶음).
#[tauri::command]
pub async fn monitor_activity(state: State<'_, AppState>, limit: Option<u32>) -> CmdResult<Activity> {
    let limit = limit.unwrap_or(30) as usize;
    let since = chrono::Utc::now().timestamp_millis() - RECENT_DELETE.as_millis() as i64;
    state
        .session()?
        .with(move |s| {
            let page = history::query(s.history(), &HistoryFilter::default(), 0, limit);
            let mut rows: Vec<u32> = s
                .history()
                .iter()
                .filter(|e| {
                    e.kind == ChangeKind::Delete
                        && e.at_ms >= since
                        && s.model.cell(e.at()).is_some_and(|c| c.v.is_empty())
                })
                .map(|e| e.row)
                .collect();
            rows.sort_unstable();
            rows.dedup();
            Activity { page, deleted_rows: rows }
        })
        .await
}

// ------------------------------------------------------------ 1i 소통

#[tauri::command]
pub fn comms_get(state: State<'_, AppState>) -> CmdResult<CommsSnapshot> {
    Ok(state.session()?.comms())
}

#[tauri::command]
pub async fn notice_send(
    state: State<'_, AppState>,
    text: String,
    important: bool,
    target: NoticeTarget,
) -> CmdResult<()> {
    if text.trim().is_empty() {
        return Err("공지 내용을 적어 주세요".into());
    }
    state.session()?.with(move |s| s.send_notice(&text, important, target)).await
}

/// `range`는 모델 좌표.
#[tauri::command]
pub async fn highlight_add(
    state: State<'_, AppState>,
    range: CellRange,
    note: String,
    audience: HighlightAudience,
) -> CmdResult<()> {
    state.session()?.with(move |s| s.highlight(range, &note, audience)).await
}

#[tauri::command]
pub async fn highlight_clear(state: State<'_, AppState>, id: u32) -> CmdResult<()> {
    state.session()?.with(move |s| s.clear_highlight(id)).await
}

#[tauri::command]
pub async fn chat_send(state: State<'_, AppState>, to: ChatTo, text: String) -> CmdResult<()> {
    if text.trim().is_empty() {
        return Ok(());
    }
    state.session()?.with(move |s| s.chat(to, &text)).await
}
