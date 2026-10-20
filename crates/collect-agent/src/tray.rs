//! 트레이 아이콘과 메뉴(Windows). 메뉴는 서비스 목록·상태가 바뀔 때마다 다시 만든다.
//!
//! - 버전·주소(복사), 서비스 n개 / 저장 실패 n개, 진행 중인 서비스 ▸(주소 복사·지금 저장하고 끝내기·저장 없이 끝내기)
//! - 기본 저장 폴더 표시·바꾸기·열기, Windows 시작 시 실행, 로그 폴더 열기, 설정 파일 열기, 끝내기
//! - 서비스가 있는 동안 PC가 절전에 들어가지 않게 한다.

use std::sync::Arc;

use agent_protocol::ServiceState;
use tao::event::Event;
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

use crate::agent::{Agent, Overview};
use crate::{Running, win};

enum UserEvent {
    Menu(MenuEvent),
    Refresh,
}

/// 트레이를 띄우고 끝날 때까지 돈다(끝내기를 고르면 공유를 닫고 프로세스를 끝낸다).
pub fn run(rt: tokio::runtime::Runtime, running: Running) -> ! {
    let agent = running.agent.clone();
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
    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        let Event::UserEvent(event) = event else { return };
        match event {
            UserEvent::Refresh => {}
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
                    on_menu(&agent, rt, id);
                }
            }
        }
        tray.set_menu(Some(Box::new(menu(&agent))));
        let _ = tray.set_tooltip(Some(tooltip(&agent)));
        keep_awake(&agent);
    })
}

fn keep_awake(agent: &Agent) {
    win::keep_awake(agent.overview().iter().any(|o| !o.state.finished()));
}

fn tooltip(agent: &Agent) -> String {
    let all = agent.overview();
    let active = all.iter().filter(|o| !o.state.finished()).count();
    let failed = all.iter().filter(|o| o.state == ServiceState::Failed).count();
    let mut t = format!("엑셀 취합 에이전트 · 서비스 {active}개");
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
        let _ = m.append(&disabled("⚠ Excel이 없어 서비스를 받지 않습니다".into()));
    }

    let active: Vec<&Overview> = all.iter().filter(|o| !o.state.finished()).collect();
    let failed = all.iter().filter(|o| o.state == ServiceState::Failed).count();
    let summary = if failed > 0 {
        format!("진행 중 {}개 · 저장 실패 {failed}개", active.len())
    } else {
        format!("진행 중 {}개", active.len())
    };
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
                ServiceState::Done => {
                    if o.saved_path.is_some() {
                        let _ = sub.append(&MenuItem::with_id(
                            format!("svc-open:{}", o.id),
                            "저장한 파일 보기",
                            true,
                            None,
                        ));
                    }
                }
                _ => {}
            }
            let _ = list.append(&sub);
        }
        let _ = m.append(&list);
    }
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

fn on_menu(agent: &Arc<Agent>, rt: &tokio::runtime::Runtime, id: &str) {
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
                "svc-cancel" => {
                    if win::confirm(&format!(
                        "‘{title}’을(를) 저장하지 않고 끝낼까요?\n취합한 값은 엑셀로 저장되지 않습니다."
                    )) {
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
