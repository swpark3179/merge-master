//! 수식 재계산: 계산 사본(숨은 Excel)은 [`collect_excel::Recalc`]가 다루고, 여기서는 상태를 화면에 알린다
//! (`recalc-status:{업무 id}`).

use std::sync::Arc;

use collect_excel::Notify;
use collect_server::RecalcStatus;
use tauri::{AppHandle, Emitter, State};

use crate::jobs::event;
use crate::{AppState, CmdResult};

/// 업무(`job`)의 재계산 상태가 바뀌면 화면에 알린다.
pub(crate) fn notify(app: &AppHandle, job: &str) -> Notify {
    let (app, name) = (app.clone(), event("recalc-status", job));
    Arc::new(move |status: &RecalcStatus| {
        let _ = app.emit(&name, status);
    })
}

#[tauri::command]
pub(crate) fn recalc_status(state: State<'_, AppState>, job: String) -> CmdResult<RecalcStatus> {
    Ok(state.job(&job)?.recalc.status())
}
