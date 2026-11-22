//! 트레이 아이콘과 메뉴(Windows). 메뉴는 서비스 목록·상태가 바뀔 때마다 다시 만든다.
//!
//! - 버전·주소(복사), 서비스 n개 / 저장 실패 n개, 진행 중인 서비스 ▸(주소 복사·지금 저장하고 끝내기·저장 없이 끝내기,
//!   백업본을 엑셀로 저장 ▸ 백업 시각마다(이 PC에 Excel이 있을 때 — [`crate::backup`])·백업 폴더 열기)
//! - IP 명단 관리…(창, [`crate::admin_window`]), 기본 저장 폴더 표시·바꾸기·열기, Windows 시작 시 실행, 로그 폴더 열기,
//!   설정 파일 열기, 끝내기
//! - 서비스가 있는 동안 PC가 절전에 들어가지 않게 한다. 'Windows 시작 시 실행'은 처음 한 번 켜 둔다(재부팅해도 이어서
//!   서비스하게 — 그 뒤 사용자가 끄면 다시 켜지 않는다).

use std::path::Path;

use std::sync::Arc;

use agent_protocol::ServiceState;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

use crate::admin_window::{self, AdminWindow};
use crate::agent::{Agent, Overview};
use crate::{Running, win};

enum UserEvent {
    Menu(MenuEvent),
    Refresh,
    /// IP 명단 창의 답·파일 대화상자.
    Admin(admin_window::Msg),
    /// 백업본을 엑셀로 저장한 결과(저장한 파일 경로 또는 오류).
    Exported(Result<String, String>),
}

/// 트레이를 띄우고 끝날 때까지 돈다(끝내기를 고르면 공유를 닫고 프로세스를 끝낸다).
pub fn run(rt: tokio::runtime::Runtime, running: Running) -> ! {
    let agent = running.agent.clone();
    autostart_once(&agent);
    win::refresh_autostart();
    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    MenuEvent::set_event_handler(Some(move |e| {
        let _ = proxy.send_event(UserEvent::Menu(e));
    }));
    // 서비스가 바뀌면 메뉴를 다시 그린다.
    let proxy = event_loop.create_proxy();
    let mut changes = agent.subscribe();
    rt.spawn(async move {
        while changes.changed().await.is_ok() {
            if proxy.send_event(UserEvent::Refresh).is_err() {
                break;
            }
            // 짧은 사이에 여러 번 바뀌면 한 번만 그린다.
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        }
    });

    let icon = Icon::from_resource(1, Some((32, 32))).ok();
    let mut builder = TrayIconBuilder::new().with_tooltip(tooltip(&agent)).with_menu(Box::new(menu(&agent)));
    if let Some(icon) = icon {
        builder = builder.with_icon(icon);
    }
    let tray: TrayIcon = match builder.build() {
        Ok(t) => t,
        Err(e) => {
            win::message(&format!("트레이 아이콘을 만들 수 없습니다: {e}"));
            std::process::exit(1);
        }
    };
    keep_awake(&agent);

    let mut running = Some(running);
    let mut rt = Some(rt);
    let mut admin: Option<AdminWindow> = None;
    let admin_proxy = event_loop.create_proxy();
    let export_proxy = event_loop.create_proxy();
    event_loop.run(move |event, target, control_flow| {
        *control_flow = ControlFlow::Wait;
        let event = match event {
            Event::UserEvent(e) => e,
            Event::WindowEvent { window_id, event: WindowEvent::CloseRequested, .. } => {
                if admin.as_ref().is_some_and(|w| w.id() == window_id) {
                    admin = None; // 닫으면 없앤다(WebView2도 끝난다)
                }
                return;
            }
            _ => return,
        };
        match event {
            // 창의 답은 메뉴를 다시 만들지 않는다(열린 트레이 메뉴가 닫히지 않게).
            UserEvent::Admin(msg) => {
                if let (Some(w), Some(rt)) = (&admin, &rt) {
                    w.handle(msg, &agent, rt);
                }
                return;
            }
            UserEvent::Exported(result) => {
                match result {
                    Ok(path) => {
                        if win::confirm(&format!("백업본을 엑셀로 저장했습니다.\n{path}\n\n저장한 파일을 폴더에서 볼까요?")) {
                            win::reveal(Path::new(&path));
                        }
                    }
                    Err(e) => win::message(&format!("백업본을 엑셀로 저장하지 못했습니다.\n{e}")),
                }
                return;
            }
            UserEvent::Refresh => {
                if let Some(w) = &admin {
                    w.changed();
                }
            }
            UserEvent::Menu(e) if e.id.0 == "ip-names" => {
                match &admin {
                    Some(w) => w.focus(),
                    None => {
                        if let Some(rt) = &rt {
                            admin = admin_window::open(
                                target,
                                &agent,
                                rt.handle().clone(),
                                admin_proxy.clone(),
                                UserEvent::Admin,
                            );
                        }
                    }
                }
                return;
            }
            UserEvent::Menu(e) => {
                let id = e.id.0.as_str();
                if id == "quit" {
                    let active = agent.overview().iter().filter(|o| !o.state.finished()).count();
                    let ask = format!(
                        "진행 중인 서비스 {active}개가 멈춥니다. 다시 켜면 이어서 합니다(그사이 마감이 지났으면 켜자마자 저장합니다).\n끝낼까요?"
                    );
                    if active == 0 || win::confirm(&ask) {
                        if let (Some(rt), Some(r)) = (rt.take(), running.take()) {
                            rt.block_on(r.stop());
                        }
                        std::process::exit(0);
                    }
                    return;
                }
                if let Some(rt) = &rt {
                    on_menu(&agent, rt, &export_proxy, id);
                }
            }
        }
        tray.set_menu(Some(Box::new(menu(&agent))));
        let _ = tray.set_tooltip(Some(tooltip(&agent)));
        keep_awake(&agent);
    })
}

/// 'Windows 시작 시 실행'을 처음 한 번 켠다(재부팅 뒤에도 서비스가 이어지게). 켠 뒤로는 사용자의 선택을 따른다.
fn autostart_once(agent: &Agent) {
    if agent.settings().autostart_set {
        return;
    }
    match win::set_autostart(true) {
        Ok(()) => {
            tracing::info!("Windows 시작 시 실행을 켰습니다(트레이 메뉴에서 끌 수 있습니다)");
            if let Err(e) = agent.mark_autostart_set() {
                tracing::warn!("{e}");
            }
        }
        Err(e) => tracing::warn!("Windows 시작 시 실행을 켤 수 없습니다: {e}"),
    }
}

fn keep_awake(agent: &Agent) {
    win::keep_awake(agent.overview().iter().any(|o| !o.state.finished()));
}

fn tooltip(agent: &Agent) -> String {
    let all = agent.overview();
    let active = all.iter().filter(|o| !o.state.finished()).count();
    let failed = all.iter().filter(|o| o.state == ServiceState::Failed).count();
    let waiting = all.iter().filter(|o| o.state == ServiceState::AwaitingHost).count();
    let mut t = format!("엑셀 취합 에이전트 · 서비스 {active}개");
    if waiting > 0 {
        t.push_str(&format!(" · 관리자 저장 대기 {waiting}개"));
    }
    if failed > 0 {
        t.push_str(&format!(" · 저장 실패 {failed}개"));
    }
    t
}

fn state_label(o: &Overview) -> &'static str {
    match o.state {
        ServiceState::Running => "서비스 중",
        ServiceState::Saving => "저장 중",
        ServiceState::Done => "저장함",
        ServiceState::Failed => "저장 실패",
        ServiceState::AwaitingHost => "관리자 저장 대기",
        ServiceState::Cancelled => "저장 없이 끝냄",
    }
}

fn short_deadline(text: &str) -> String {
    match crate::agent::parse_deadline(text) {
        Some(d) => d.format("%m/%d %H:%M").to_string(),
        None => text.to_string(),
    }
}

fn menu(agent: &Agent) -> Menu {
    let m = Menu::new();
    let info = agent.info();
    let settings = agent.settings();
    let all = agent.overview();
    let disabled = |text: String| MenuItem::new(text, false, None);
    let _ = m.append(&disabled(format!("엑셀 취합 에이전트 v{}", agent.version())));
    let url = info.urls.first().cloned().unwrap_or_default();
    let _ = m.append(&MenuItem::with_id("copy-url", format!("주소 복사 ({url})"), !url.is_empty(), None));
    if !info.excel {
        let _ = m.append(&disabled("Excel 없음 — 마감 저장은 관리자 PC에서 합니다".into()));
    }

    let active: Vec<&Overview> = all.iter().filter(|o| !o.state.finished()).collect();
    let failed = all.iter().filter(|o| o.state == ServiceState::Failed).count();
    let waiting = all.iter().filter(|o| o.state == ServiceState::AwaitingHost).count();
    let mut summary = format!("진행 중 {}개", active.len());
    if waiting > 0 {
        summary.push_str(&format!(" · 관리자 저장 대기 {waiting}개"));
    }
    if failed > 0 {
        summary.push_str(&format!(" · 저장 실패 {failed}개"));
    }
    let _ = m.append(&disabled(summary));
    if !all.is_empty() {
        let list = Submenu::new("서비스", true);
        for o in &all {
            let title = if o.title.is_empty() { "새 취합" } else { o.title.as_str() };
            let label = format!("{title} · 마감 {} · {}", short_deadline(&o.deadline), state_label(o));
            let sub = Submenu::new(label, true);
            if let Some(u) = &o.url {
                let _ = sub.append(&MenuItem::with_id(
                    format!("svc-copy:{}", o.id),
                    format!("주소 복사 ({u})"),
                    true,
                    None,
                ));
            }
            let _ = sub.append(&disabled(format!("저장할 곳: {}", o.output_path)));
            match o.state {
                ServiceState::Running => {
                    let _ = sub.append(&MenuItem::with_id(
                        format!("svc-save:{}", o.id),
                        "지금 저장하고 끝내기…",
                        true,
                        None,
                    ));
                    let _ =
                        sub.append(&MenuItem::with_id(format!("svc-cancel:{}", o.id), "저장 없이 끝내기…", true, None));
                }
                ServiceState::Failed => {
                    let _ = sub.append(&MenuItem::with_id(format!("svc-retry:{}", o.id), "다시 저장", true, None));
                    let _ =
                        sub.append(&MenuItem::with_id(format!("svc-cancel:{}", o.id), "저장 없이 끝내기…", true, None));
                }
                ServiceState::AwaitingHost => {
                    let _ = sub.append(&disabled("넘긴 호스트 앱이 접속하면 그 PC에서 저장합니다".into()));
                    if info.excel {
                        let _ = sub.append(&MenuItem::with_id(
                            format!("svc-retry:{}", o.id),
                            "이 PC에서 다시 저장",
                            true,
                            None,
                        ));
                    }
                    let _ = sub.append(&MenuItem::with_id(
                        format!("svc-dir:{}", o.id),
                        "취합 데이터 폴더 열기",
                        true,
                        None,
                    ));
                    let _ =
                        sub.append(&MenuItem::with_id(format!("svc-cancel:{}", o.id), "저장 없이 끝내기…", true, None));
                }
                ServiceState::Done if o.saved_on_host => {
                    if let Some(p) = &o.saved_path {
                        let _ = sub.append(&disabled(format!("관리자 PC에 저장함: {p}")));
                    }
                }
                ServiceState::Done if o.saved_path.is_some() => {
                    let _ =
                        sub.append(&MenuItem::with_id(format!("svc-open:{}", o.id), "저장한 파일 보기", true, None));
                }
                _ => {}
            }
            backup_items(&sub, agent, o, info.excel);
            let _ = list.append(&sub);
        }
        let _ = m.append(&list);
    }
    let _ = m.append(&PredefinedMenuItem::separator());
    let names = agent.ip_names().len();
    let _ = m.append(&MenuItem::with_id("ip-names", format!("IP 명단 관리… ({names}명)"), true, None));
    let _ = m.append(&PredefinedMenuItem::separator());
    let _ = m.append(&disabled(format!("기본 저장 폴더: {}", settings.base_dir.display())));
    let _ = m.append(&MenuItem::with_id("base-change", "기본 저장 폴더 바꾸기…", true, None));
    let _ = m.append(&MenuItem::with_id("base-open", "기본 저장 폴더 열기", true, None));
    let _ = m.append(&PredefinedMenuItem::separator());
    let _ =
        m.append(&CheckMenuItem::with_id("autostart", "Windows 시작 시 실행", true, win::autostart_enabled(), None));
    let _ = m.append(&MenuItem::with_id("logs", "로그 폴더 열기", true, None));
    let _ = m.append(&MenuItem::with_id("settings", "설정 파일 열기", true, None));
    let _ = m.append(&PredefinedMenuItem::separator());
    let _ = m.append(&MenuItem::with_id("quit", "끝내기", true, None));
    m
}

/// 서비스 메뉴의 백업 줄: 백업본을 엑셀로 저장 ▸(백업 시각마다, Excel이 있을 때)·백업 폴더 열기, 손상으로 백업에서 되살렸으면 그
/// 시각. 서비스 중이 아니고 백업도 없으면 보이지 않는다.
fn backup_items(sub: &Submenu, agent: &Agent, o: &Overview, excel: bool) {
    let backups = agent.backups(&o.id);
    if backups.is_empty() && o.state != ServiceState::Running && o.restored_from_ms.is_none() {
        return;
    }
    let disabled = |text: String| MenuItem::new(text, false, None);
    let _ = sub.append(&PredefinedMenuItem::separator());
    if let Some(ms) = o.restored_from_ms {
        let _ = sub.append(&disabled(format!("기록 파일이 손상되어 {} 백업으로 되살렸습니다", local_label(ms))));
    }
    if backups.is_empty() {
        let _ = sub.append(&disabled("백업 없음(서비스 중 1시간마다 남깁니다)".into()));
        return;
    }
    if excel {
        let list = Submenu::new("백업본을 엑셀로 저장", true);
        for b in &backups {
            let _ = list.append(&MenuItem::with_id(
                format!("bak:{}:{}", o.id, b.name),
                format!("{} 백업", b.label()),
                true,
                None,
            ));
        }
        let _ = sub.append(&list);
    } else {
        let _ = sub.append(&disabled("백업본을 엑셀로 저장 (Excel 없음)".into()));
    }
    let _ = sub.append(&MenuItem::with_id(format!("svc-bakdir:{}", o.id), "백업 폴더 열기", true, None));
}

/// 유닉스 ms → 이 PC 현지 시각 `11/22 15:00`.
fn local_label(ms: i64) -> String {
    use chrono::TimeZone;
    chrono::Local.timestamp_millis_opt(ms).single().map_or_else(String::new, |t| crate::backup::label(t.naive_local()))
}

/// 백업 하나를 엑셀로 저장한다: 저장할 곳을 묻고(기본은 마감 때 저장할 폴더 + `저장할이름_백업_시각`), 저장은 뒤에서 한다
/// (메뉴가 멈추지 않게 — 끝나면 [`UserEvent::Exported`]로 알린다).
fn export_backup(
    agent: &Arc<Agent>,
    rt: &tokio::runtime::Runtime,
    proxy: &EventLoopProxy<UserEvent>,
    svc: &str,
    name: &str,
) {
    let Some(start) = agent.backup_default_path(svc, name) else {
        win::message("그 백업을 찾을 수 없습니다(새 백업이 생겨 지워졌을 수 있습니다). 메뉴를 다시 열어 주세요.");
        return;
    };
    let ext = start.extension().map_or_else(|| "xlsx".into(), |e| e.to_string_lossy().to_string());
    let Some(out) = win::save_workbook("백업본을 엑셀로 저장", &start, &ext) else { return };
    let (agent, proxy, svc, name) = (agent.clone(), proxy.clone(), svc.to_string(), name.to_string());
    rt.spawn(async move {
        let result = agent.export_backup(&svc, &name, out).await.map_err(|e| e.message);
        let _ = proxy.send_event(UserEvent::Exported(result));
    });
}

fn on_menu(agent: &Arc<Agent>, rt: &tokio::runtime::Runtime, proxy: &EventLoopProxy<UserEvent>, id: &str) {
    // 백업 이름에는 ':'가 없고 서비스 id도 영숫자·'-'뿐이다.
    if let Some((svc, name)) = id.strip_prefix("bak:").and_then(|rest| rest.split_once(':')) {
        export_backup(agent, rt, proxy, svc, name);
        return;
    }
    match id {
        "copy-url" => {
            if let Some(u) = agent.info().urls.first() {
                win::copy_text(u);
            }
        }
        "base-change" => {
            let current = agent.settings().base_dir;
            if let Some(dir) = win::pick_folder(&current)
                && let Err(e) = agent.set_base_dir(dir)
            {
                win::message(&e);
            }
        }
        "base-open" => {
            let dir = agent.settings().base_dir;
            let _ = std::fs::create_dir_all(&dir);
            win::reveal(&dir);
        }
        "autostart" => {
            if let Err(e) = win::set_autostart(!win::autostart_enabled()) {
                win::message(&format!("시작 시 실행을 바꿀 수 없습니다: {e}"));
            }
        }
        "logs" => win::reveal(&agent.data_dir().join("logs")),
        "settings" => {
            win::open_file(agent.settings_path());
            win::message("설정 파일을 고친 뒤에는 에이전트를 다시 켜야 반영됩니다(포트·최대 기간).");
        }
        _ => {
            let Some((what, svc)) = id.split_once(':') else { return };
            let Some(o) = agent.overview().into_iter().find(|o| o.id == svc) else { return };
            let title = if o.title.is_empty() { "새 취합".to_string() } else { o.title.clone() };
            match what {
                "svc-copy" => {
                    if let Some(u) = &o.url {
                        win::copy_text(u);
                    }
                }
                "svc-open" => {
                    if let Some(p) = &o.saved_path {
                        win::reveal(std::path::Path::new(p));
                    }
                }
                "svc-save" => {
                    if win::confirm(&format!(
                        "‘{title}’을(를) 지금 엑셀로 저장하고 끝낼까요?\n참여자는 더 입력할 수 없습니다."
                    )) {
                        report(rt.block_on(agent.finish(svc, None, true)));
                    }
                }
                "svc-dir" => win::reveal(&o.dir),
                "svc-bakdir" => win::reveal(&crate::backup::dir(&o.dir)),
                "svc-cancel" => {
                    let ask = if o.state == ServiceState::AwaitingHost {
                        format!(
                            "‘{title}’을(를) 저장하지 않고 끝낼까요?\n관리자 PC의 저장을 기다리던 취합 데이터를 지웁니다(되돌릴 수 없습니다)."
                        )
                    } else {
                        format!("‘{title}’을(를) 저장하지 않고 끝낼까요?\n취합한 값은 엑셀로 저장되지 않습니다.")
                    };
                    if win::confirm(&ask) {
                        report(rt.block_on(agent.finish(svc, None, false)));
                    }
                }
                "svc-retry" => report(rt.block_on(agent.retry(svc, None))),
                _ => {}
            }
        }
    }
}

fn report<T>(r: Result<T, crate::AgentError>) {
    if let Err(e) = r {
        win::message(&e.message);
    }
}
