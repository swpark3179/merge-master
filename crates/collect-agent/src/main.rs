//! `merge-master-agent.exe`: 창 없이 트레이에서 돈다. `--headless`면 트레이 없이(Ctrl+C로 끝).
//!
//! 옵션: `--data-dir <폴더>`(기본 `%LOCALAPPDATA%\merge-master-agent`), `--port <포트>`(설정 대신), `--headless`,
//! `--fake-excel`(Excel 없이 시험 — 저장 대신 요약 JSON을 쓴다).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::sync::Arc;

use collect_agent::{Engine, FakeEngine, StartOptions, logging, settings};

struct Args {
    data_dir: Option<PathBuf>,
    port: Option<u16>,
    headless: bool,
    fake_excel: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args { data_dir: None, port: None, headless: false, fake_excel: false };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--data-dir" => a.data_dir = Some(it.next().ok_or("--data-dir 다음에 폴더를 주세요")?.into()),
            "--port" => {
                let v = it.next().ok_or("--port 다음에 포트를 주세요")?;
                a.port = Some(v.parse().map_err(|_| format!("포트가 올바르지 않습니다: {v}"))?);
            }
            "--headless" => a.headless = true,
            "--fake-excel" => a.fake_excel = true,
            other => return Err(format!("알 수 없는 옵션: {other}")),
        }
    }
    Ok(a)
}

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => fail(&e),
    };
    let data_dir = args.data_dir.clone().unwrap_or_else(settings::default_data_dir);
    let headless = args.headless || !cfg!(windows);
    logging::init(&data_dir.join("logs"), headless);

    #[cfg(windows)]
    let _single = match collect_agent_win::single_instance(&data_dir) {
        Some(guard) => guard,
        None => fail("에이전트가 이미 실행 중입니다(작업 표시줄 오른쪽 트레이를 확인해 주세요)"),
    };

    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("tokio 런타임");
    let engine: Arc<dyn Engine> = if args.fake_excel {
        tracing::warn!("--fake-excel: Excel 없이 시험합니다(엑셀 대신 요약 JSON을 씁니다)");
        Arc::new(FakeEngine::default())
    } else {
        match platform_engine(&data_dir) {
            Ok(e) => e,
            Err(e) => fail(&e),
        }
    };
    let mut opts = StartOptions::new(data_dir, engine);
    opts.port = args.port;
    let running = match rt.block_on(collect_agent::start(opts)) {
        Ok(r) => r,
        Err(e) => fail(&format!("에이전트를 시작할 수 없습니다: {e}")),
    };

    #[cfg(windows)]
    if !headless {
        collect_agent::tray::run(rt, running);
    }
    rt.block_on(async {
        let _ = tokio::signal::ctrl_c().await;
        tracing::info!("에이전트를 끝냅니다");
        running.stop().await;
    });
}

#[cfg(windows)]
use collect_agent::win as collect_agent_win;

#[cfg(windows)]
fn platform_engine(data_dir: &std::path::Path) -> Result<Arc<dyn Engine>, String> {
    Ok(Arc::new(collect_agent::excel::ExcelEngine::new(data_dir.join("calc"))?))
}

#[cfg(not(windows))]
fn platform_engine(_data_dir: &std::path::Path) -> Result<Arc<dyn Engine>, String> {
    Err("Excel은 Windows에서만 쓸 수 있습니다(시험은 --fake-excel)".into())
}

/// 알리고 끝낸다(트레이 앱은 콘솔이 없어 메시지 창으로).
fn fail(message: &str) -> ! {
    tracing::error!("{message}");
    eprintln!("{message}");
    #[cfg(windows)]
    collect_agent::win::message(message);
    std::process::exit(1)
}
