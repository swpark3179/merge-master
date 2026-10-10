//! Tauri·Excel 없이 참여자 서버만 띄우는 개발용 바이너리.
//!
//! ```text
//! cargo run -p collect-server --bin collect-dev-server -- --fixture fixtures/budget.sheet.json
//!     [--port 8080] [--bind 0.0.0.0] [--slug q4] [--title 제목] [--requester 박지훈]
//!     [--required F,H] [--others-hidden] [--add-rows] [--ip-names people.json] [--dev-admin]
//! ```
//! 설정은 `CollectConfig::suggest`로 만든 초안에 위 옵션을 더한다. `--dev-admin`은 `/__dev/*`(승인·거절·연결 끊기·잠금 해제·설정·수식 결과)를 켠다.
//! `--ip-names`는 IP별 이름 JSON(`{"192.168.0.11": "이서연"}`): 그 IP에서 열면 이름 없이 바로 들어온다. 없으면 모두 이름 + 승인
//! (`--dev-admin`이면 `POST /__dev/approve`).

use std::net::IpAddr;

use anyhow::Context;
use collect_core::SheetModel;
use collect_core::a1::col_from_letters;
use collect_core::config::{CollectConfig, OthersRows, suggested_key_col};
use collect_server::{IpNames, ServerConfig, SessionHandle, SessionInit, lan_addresses, start};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into())).init();

    let mut fixture = None;
    let mut server = ServerConfig { bind: IpAddr::from([0, 0, 0, 0]), port: 8080, slug: "q4".into(), dev_admin: false };
    let mut title = "2026년 부서별 예산 신청".to_string();
    let mut requester = "박지훈".to_string();
    let mut required: Vec<String> = Vec::new();
    let mut others_hidden = false;
    let mut add_rows = false;
    let mut ip_names = None;
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--dev-admin" => server.dev_admin = true,
            "--others-hidden" => others_hidden = true,
            "--add-rows" => add_rows = true,
            _ => {
                let value = args.next().with_context(|| format!("{flag} 값이 없습니다"))?;
                match flag.as_str() {
                    "--fixture" => fixture = Some(value),
                    "--port" => server.port = value.parse()?,
                    "--bind" => server.bind = value.parse()?,
                    "--slug" => server.slug = value,
                    "--title" => title = value,
                    "--requester" => requester = value,
                    "--required" => required = value.split(',').map(|s| s.trim().to_string()).collect(),
                    "--ip-names" => ip_names = Some(value),
                    other => anyhow::bail!("알 수 없는 옵션 {other}"),
                }
            }
        }
    }

    let path = fixture.context("--fixture <시트 JSON>이 필요합니다")?;
    let model: SheetModel = serde_json::from_slice(&std::fs::read(&path).with_context(|| path.clone())?)?;
    let mut config = CollectConfig::suggest(&model);
    // 개발 서버는 담당자 열(후보 1순위)로 행을 배정해 둔다(호스트 앱은 1b에서 고른다).
    config.assignment.key_col = suggested_key_col(&model);
    config.title = title;
    config.requester = requester;
    config.description = "부서별 담당 비목의 신청 금액과 산출 근거를 입력해 주세요.".into();
    if others_hidden {
        config.permissions.others_rows = OthersRows::Hidden;
    }
    config.permissions.add_rows = add_rows;
    for letters in &required {
        let c = col_from_letters(letters).with_context(|| format!("열 이름이 잘못되었습니다: {letters}"))?;
        let rule = config.columns.get_mut(c as usize).with_context(|| format!("없는 열: {letters}"))?;
        rule.required = true;
    }

    let session = SessionHandle::spawn(SessionInit::new(model, config));
    if let Some(path) = ip_names {
        let text = std::fs::read_to_string(&path).with_context(|| path.clone())?;
        let names = IpNames::parse_json(&text).map_err(anyhow::Error::msg)?;
        println!("  IP 명단 {}명 ({path})", names.len());
        session.set_ip_names(names);
    }
    let handle = start(server.clone(), session).await?;
    for lan in lan_addresses() {
        println!("  {} → http://{}:{}/{}", lan.name, lan.ip, handle.local_addr.port(), server.slug);
    }
    println!("  로컬 → http://127.0.0.1:{}/{}   (Ctrl+C로 종료)", handle.local_addr.port(), server.slug);
    tokio::signal::ctrl_c().await?;
    handle.stop().await;
    Ok(())
}
