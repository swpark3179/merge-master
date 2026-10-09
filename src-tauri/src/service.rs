//! 1c 서비스·접속자: 참여자 서버 시작/중지, 접속자 표 이벤트, 승인·거절·연결 끊기·잠금 해제, QR, 방화벽 확인.

use std::net::{IpAddr, Ipv4Addr};
use std::process::Command;
use std::time::Duration;

use collect_core::CellRange;
use collect_server::{HostSnapshot, ServerConfig, SessionHandle};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use ts_rs::TS;

use crate::{AppState, CmdResult};

/// 접속자 표 이벤트를 웹뷰에 보내는 최소 간격(커서가 움직일 때마다 보내지 않도록).
const EMIT_EVERY: Duration = Duration::from_millis(250);

#[derive(Serialize, TS)]
#[ts(export)]
pub struct ServiceUrl {
    name: String,
    ip: String,
    url: String,
}

#[derive(Serialize, TS)]
#[ts(export)]
pub struct ServiceInfo {
    port: u16,
    slug: String,
    urls: Vec<ServiceUrl>,
}

/// 접속자 표가 바뀌면 웹뷰에 알린다(세션이 살아 있는 동안 — 서비스를 멈춰도 진행률은 계속 보인다).
pub(crate) fn forward_participants(app: &AppHandle, session: &SessionHandle) {
    let mut rx = session.subscribe();
    let app = app.clone();
    let _ = app.emit("participants", rx.borrow().clone());
    tauri::async_runtime::spawn(async move {
        while rx.changed().await.is_ok() {
            let snap = rx.borrow_and_update().clone();
            let _ = app.emit("participants", snap);
            tokio::time::sleep(EMIT_EVERY).await;
        }
    });
}

/// 기록 파일에 서비스 상태를 남긴다(앱을 다시 켜면 같은 포트로 연다).
async fn remember_server(session: &SessionHandle, port: u16, slug: &str, running: bool) {
    let meta = collect_server::ServerMeta { port, slug: slug.to_string(), running };
    let json = serde_json::to_string(&meta).unwrap_or_default();
    let _ = session.with(move |s| s.set_meta(collect_server::META_SERVER, &json)).await;
}

/// 참여자 서버를 연다(이미 열려 있으면 닫고 다시).
pub(crate) async fn start_service(state: &AppState, port: u16, slug: String) -> CmdResult<ServiceInfo> {
    let session = state.session()?;
    let mut server = state.server.lock().await;
    if let Some(old) = server.take() {
        old.stop().await;
    }
    let config = ServerConfig { bind: IpAddr::V4(Ipv4Addr::UNSPECIFIED), port, slug: slug.clone(), dev_admin: false };
    let handle = collect_server::start(config, session.clone()).await.map_err(|e| e.to_string())?;
    let port = handle.local_addr.port();
    *server = Some(handle);
    remember_server(&session, port, &slug, true).await;
    let urls = collect_server::lan_addresses()
        .into_iter()
        .map(|a| ServiceUrl { url: format!("http://{}:{port}/{slug}", a.ip), ip: a.ip, name: a.name })
        .collect();
    Ok(ServiceInfo { port, slug, urls })
}

#[tauri::command]
pub async fn server_start(state: State<'_, AppState>, port: u16, slug: String) -> CmdResult<ServiceInfo> {
    start_service(&state, port, slug).await
}

/// 지금 실행 중인 서비스(화면을 다시 그릴 때).
#[tauri::command]
pub async fn server_info(state: State<'_, AppState>) -> CmdResult<Option<ServiceInfo>> {
    let server = state.server.lock().await;
    Ok(server.as_ref().map(|h| {
        let (port, slug) = (h.local_addr.port(), h.slug().to_string());
        let urls = collect_server::lan_addresses()
            .into_iter()
            .map(|a| ServiceUrl { url: format!("http://{}:{port}/{slug}", a.ip), ip: a.ip, name: a.name })
            .collect();
        ServiceInfo { port, slug, urls }
    }))
}

/// 서버만 닫는다. 세션(입력한 값·이력)은 그대로 남아 다시 시작하면 이어서 받는다.
#[tauri::command]
pub async fn server_stop(state: State<'_, AppState>) -> CmdResult<()> {
    if let Some(handle) = state.server.lock().await.take() {
        let (port, slug) = (handle.local_addr.port(), handle.slug().to_string());
        handle.stop().await;
        if let Ok(session) = state.session() {
            remember_server(&session, port, &slug, false).await;
        }
    }
    Ok(())
}

/// 시트를 확정했으면 지금의 접속자 표.
#[tauri::command]
pub fn participants_get(state: State<'_, AppState>) -> Option<HostSnapshot> {
    state.session().ok().map(|s| s.snapshot())
}

/// 명단에 없는 이름을 승인한다. `range`는 시트 주소(예: H500:H620)로, 주면 그 범위를 배정한다.
#[tauri::command]
pub async fn participant_approve(
    app: AppHandle,
    state: State<'_, AppState>,
    name: String,
    range: Option<String>,
    note: String,
) -> CmdResult<()> {
    let session = state.session()?;
    let range = match range.as_deref().map(str::trim).filter(|r| !r.is_empty()) {
        Some(text) => {
            let origin = session.with(|s| s.model.origin).await?;
            let sheet = CellRange::parse(text).map_err(|e| e.to_string())?;
            Some(sheet.relative_to(origin).ok_or_else(|| format!("{text}은(는) 불러온 표 밖입니다"))?)
        }
        None => None,
    };
    // 설정은 세션이 가진다: 1b 화면은 바뀌었다는 알림을 받고 세션에서 다시 읽는다.
    session.approve(&name, range, &note).await?;
    let _ = app.emit("config-changed", ());
    Ok(())
}

#[tauri::command]
pub async fn participant_reject(state: State<'_, AppState>, name: String) -> CmdResult<()> {
    state.session()?.reject(&name).await
}

#[tauri::command]
pub async fn participant_kick(state: State<'_, AppState>, name: String) -> CmdResult<()> {
    state.session()?.kick(&name).await
}

/// 그 사람이 잡고 있는 편집 잠금을 푼다(편집기를 열어 둔 채 자리를 비운 경우).
#[tauri::command]
pub async fn participant_unlock(state: State<'_, AppState>, name: String) -> CmdResult<()> {
    state.session()?.unlock(&name).await
}

/// 접속 주소 QR(SVG 문자열).
#[tauri::command]
pub fn qr_svg(text: String) -> CmdResult<String> {
    let code = qrcode::QrCode::new(text.as_bytes()).map_err(|e| e.to_string())?;
    Ok(code
        .render::<qrcode::render::svg::Color>()
        .min_dimensions(160, 160)
        .dark_color(qrcode::render::svg::Color("#222222"))
        .light_color(qrcode::render::svg::Color("#ffffff"))
        .build())
}

#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct FirewallStatus {
    /// 이 앱 실행 파일을 허용하는 인바운드 규칙이 있다(Windows가 처음에 물어볼 때 허용하면 생긴다).
    app_allowed: bool,
    /// 이 앱을 막는 규칙이 있다(처음 물어볼 때 '취소'를 누른 경우).
    app_blocked: bool,
    /// 포트를 허용하는 인바운드 규칙이 있다.
    port_allowed: bool,
}

fn powershell(script: &str) -> CmdResult<String> {
    let out = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .output()
        .map_err(|e| format!("PowerShell을 실행할 수 없습니다: {e}"))?;
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// 방화벽 규칙 확인(읽기만 한다. 관리자 권한이 필요 없다).
#[tauri::command]
pub async fn firewall_check(port: u16) -> CmdResult<FirewallStatus> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe = exe.to_string_lossy().replace('\'', "''");
    let script = format!(
        r#"$ErrorActionPreference='SilentlyContinue';
$in = {{ param($r) $r | Where-Object {{ $_.Enabled -eq 'True' -and $_.Direction -eq 'Inbound' }} }};
$app = & $in (Get-NetFirewallApplicationFilter -Program '{exe}' | Get-NetFirewallRule);
$port = & $in (Get-NetFirewallPortFilter -Protocol TCP | Where-Object {{ $_.LocalPort -eq '{port}' }} | Get-NetFirewallRule);
'{{0}} {{1}} {{2}}' -f @($app | Where-Object Action -eq 'Allow').Count, @($app | Where-Object Action -eq 'Block').Count, @($port | Where-Object Action -eq 'Allow').Count"#
    );
    let out = tauri::async_runtime::spawn_blocking(move || powershell(&script)).await.map_err(|e| e.to_string())??;
    let n: Vec<u32> = out.split_whitespace().filter_map(|s| s.parse().ok()).collect();
    let get = |i: usize| n.get(i).copied().unwrap_or(0) > 0;
    Ok(FirewallStatus { app_allowed: get(0), app_blocked: get(1), port_allowed: get(2) })
}

/// 포트 허용 규칙을 추가한다. Windows가 관리자 권한(UAC)을 묻고, 사용자가 허락해야 추가된다.
#[tauri::command]
pub async fn firewall_add(port: u16) -> CmdResult<()> {
    let args = format!(
        "advfirewall firewall add rule name=\"엑셀 취합 도구 (TCP {port})\" dir=in action=allow protocol=TCP localport={port} profile=domain,private"
    );
    let script = format!(
        "Start-Process -FilePath netsh -Verb RunAs -Wait -WindowStyle Hidden -ArgumentList '{}'",
        args.replace('\'', "''")
    );
    tauri::async_runtime::spawn_blocking(move || powershell(&script)).await.map_err(|e| e.to_string())??;
    Ok(())
}
