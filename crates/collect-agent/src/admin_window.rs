//! 트레이 'IP 명단 관리…' 창(Windows, WebView2). 화면([`admin_ui`](./admin_ui))은 `window.ipc.postMessage`로 요청을
//! 보내고 답은 `mm.reply(번호, 답)`으로 받는다. 요청 처리는 [`crate::admin`]이 한다. HTTP 경로가 없어 이 PC에서만 다룬다.
//!
//! - 창은 하나(열려 있으면 앞으로), 닫으면 없앤다(WebView2 프로세스도 끝난다).
//! - JSON 불러오기·내보내기의 파일 대화상자는 트레이 이벤트 처리기에서 연다(WebView2 콜백 안에서 모달 창을 띄우지 않게).
//! - WebView2 데이터는 에이전트 데이터 폴더의 `webview\`에 둔다(실행 파일 옆에 폴더를 만들지 않게).

use std::cell::Cell;
use std::sync::Arc;

use serde::Deserialize;
use tao::dpi::LogicalSize;
use tao::event_loop::{EventLoopProxy, EventLoopWindowTarget};
use tao::window::{Window, WindowBuilder, WindowId};
use wry::{NewWindowResponse, WebContext, WebView, WebViewBuilder, WebViewBuilderExtWindows};

use crate::admin::{self, Request};
use crate::agent::Agent;
use crate::win;

/// 창 화면: index.html에 스타일·스크립트를 끼워 넣은 한 장.
fn page() -> String {
    include_str!("admin_ui/index.html")
        .replace("/*__CSS__*/", include_str!("admin_ui/admin.css"))
        .replace("/*__JS__*/", include_str!("admin_ui/admin.js"))
}

/// 트레이 이벤트 고리로 보내는 것.
pub enum Msg {
    /// 화면 요청의 답(요청 번호, JSON).
    Reply(u64, String),
    /// JSON 파일을 골라 명단을 바꾼다(요청 번호).
    Import(u64),
    /// 명단을 JSON 파일로 내보낸다(요청 번호).
    Export(u64),
}

/// 화면이 보내는 한 통: `{id, req}`(요청) 또는 `{id, pick: "import"|"export"}`(파일 대화상자).
#[derive(Deserialize)]
struct Envelope {
    id: u64,
    #[serde(default)]
    req: Option<serde_json::Value>,
    #[serde(default)]
    pick: Option<String>,
}

/// 열린 창. 필드 순서대로 없앤다(화면 → WebView2 데이터 → 창).
pub struct AdminWindow {
    webview: WebView,
    _context: WebContext,
    window: Window,
}

impl AdminWindow {
    pub fn id(&self) -> WindowId {
        self.window.id()
    }

    /// 앞으로 가져온다(이미 열려 있을 때).
    pub fn focus(&self) {
        self.window.set_visible(true);
        self.window.set_minimized(false);
        self.window.set_focus();
    }

    /// 서비스·명단이 바뀌었다(화면이 다시 읽는다).
    pub fn changed(&self) {
        let _ = self.webview.evaluate_script("window.mm && mm.changed()");
    }

    fn reply(&self, id: u64, json: &str) {
        let _ = self.webview.evaluate_script(&format!("window.mm && mm.reply({id}, {json})"));
    }

    /// 트레이 이벤트 고리에서 받은 일을 한다.
    pub fn handle(&self, msg: Msg, agent: &Arc<Agent>, rt: &tokio::runtime::Runtime) {
        match msg {
            Msg::Reply(id, json) => self.reply(id, &json),
            Msg::Import(id) => {
                let picked = rfd::FileDialog::new()
                    .set_title("IP 명단 불러오기")
                    .add_filter("JSON", &["json"])
                    .set_parent(&self.window)
                    .pick_file();
                let notice = picked.and_then(|path| {
                    let text = match std::fs::read_to_string(&path) {
                        Ok(t) => t,
                        Err(e) => return Some(Err(format!("파일을 읽을 수 없습니다: {e}"))),
                    };
                    let names = match collect_server::IpNames::parse_json(&text) {
                        Ok(n) => n,
                        Err(e) => return Some(Err(e)),
                    };
                    let ask =
                        format!("지금 명단 {}명을 파일의 {}명으로 바꿀까요?", agent.ip_names().len(), names.len());
                    if !win::confirm(&ask) {
                        return None;
                    }
                    let n = names.len();
                    Some(
                        rt.block_on(admin::handle(agent, Request::ImportText { text }))
                            .map(|_| format!("{n}명을 불러왔습니다")),
                    )
                });
                self.reply(id, &answer(agent, notice));
            }
            Msg::Export(id) => {
                let picked = rfd::FileDialog::new()
                    .set_title("IP 명단 내보내기")
                    .add_filter("JSON", &["json"])
                    .set_file_name("IP명단.json")
                    .set_parent(&self.window)
                    .save_file();
                let notice = picked.map(|path| {
                    let names = agent.ip_names();
                    std::fs::write(&path, names.to_json())
                        .map(|_| format!("{}명을 {}에 저장했습니다", names.len(), path.display()))
                        .map_err(|e| format!("파일에 쓸 수 없습니다: {e}"))
                });
                self.reply(id, &answer(agent, notice));
            }
        }
    }
}

/// 파일 대화상자 뒤의 답: 지금 상태와 알림(취소했으면 알림 없음).
fn answer(agent: &Arc<Agent>, notice: Option<Result<String, String>>) -> String {
    let view = admin::view(agent);
    match notice {
        Some(Err(message)) => serde_json::json!({ "ok": false, "message": message, "view": view }),
        Some(Ok(notice)) => serde_json::json!({ "ok": true, "view": view, "notice": notice }),
        None => serde_json::json!({ "ok": true, "view": view }),
    }
    .to_string()
}

/// 창을 연다. WebView2 런타임이 없거나 만들 수 없으면 까닭을 알리고 `None`.
pub fn open<T: Send + 'static>(
    target: &EventLoopWindowTarget<T>,
    agent: &Arc<Agent>,
    rt: tokio::runtime::Handle,
    proxy: EventLoopProxy<T>,
    wrap: fn(Msg) -> T,
) -> Option<AdminWindow> {
    let fallback = format!(
        "IP 명단 창을 열 수 없습니다(Microsoft Edge WebView2 런타임이 필요합니다 — \
         https://go.microsoft.com/fwlink/p/?LinkId=2124703 에서 설치).\n대신 {}을(를) 고친 뒤 에이전트를 다시 켜도 됩니다.",
        agent.ip_names_path().display()
    );
    if let Err(e) = wry::webview_version() {
        tracing::warn!("WebView2 런타임을 찾을 수 없습니다: {e}");
        win::message(&fallback);
        return None;
    }
    let window = match WindowBuilder::new()
        .with_title("IP 명단 관리 — 엑셀 취합 에이전트")
        .with_inner_size(LogicalSize::new(960.0, 680.0))
        .with_min_inner_size(LogicalSize::new(640.0, 420.0))
        .build(target)
    {
        Ok(w) => w,
        Err(e) => {
            win::message(&format!("창을 만들 수 없습니다: {e}"));
            return None;
        }
    };
    let mut context = WebContext::new(Some(agent.data_dir().join("webview")));
    let handler_agent = agent.clone();
    let first = Cell::new(true);
    let built = WebViewBuilder::new_with_web_context(&mut context)
        .with_html(page())
        .with_devtools(cfg!(debug_assertions))
        .with_browser_accelerator_keys(false)
        // 이 화면 말고 다른 곳으로 가거나 새 창을 열지 않는다.
        .with_navigation_handler(move |_| first.replace(false))
        .with_new_window_req_handler(|_, _| NewWindowResponse::Deny)
        .with_ipc_handler(move |req| {
            let Ok(env) = serde_json::from_str::<Envelope>(req.body()) else { return };
            match (env.pick.as_deref(), env.req) {
                (Some("import"), _) => {
                    let _ = proxy.send_event(wrap(Msg::Import(env.id)));
                }
                (Some("export"), _) => {
                    let _ = proxy.send_event(wrap(Msg::Export(env.id)));
                }
                (_, Some(body)) => {
                    let (agent, proxy, id) = (handler_agent.clone(), proxy.clone(), env.id);
                    rt.spawn(async move {
                        let json = admin::handle_json(&agent, &body.to_string()).await;
                        let _ = proxy.send_event(wrap(Msg::Reply(id, json)));
                    });
                }
                _ => {}
            }
        })
        .build(&window);
    match built {
        Ok(webview) => Some(AdminWindow { webview, _context: context, window }),
        Err(e) => {
            tracing::warn!("IP 명단 창의 WebView2를 만들 수 없습니다: {e}");
            win::message(&fallback);
            None
        }
    }
}
