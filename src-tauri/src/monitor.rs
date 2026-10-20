//! 4 진행 모니터링(1d)과 1i 소통 패널의 이벤트: 셀 변경·소통을 웹뷰에 보낸다(보기·보내기 command는 [`crate::host`]).

use collect_server::{CommsSnapshot, SessionHandle};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::broadcast::error::RecvError;

use crate::AppState;
use crate::jobs::event;

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
