//! 에이전트: 호스트 앱이 넘긴 취합 업무를 이 PC에서 서비스하고(참여자 서버), 마감이 되면 엑셀로 저장해 끝낸다.
//! 창 없이 Windows 트레이에서 돈다(`merge-master-agent.exe`).
//!
//! - 포트 하나(기본 8090)에 참여자 서버(`/`, `/{slug}`, `/ws` …)와 관리 API(`/agent/*`, [`api`])를 함께 둔다.
//! - 데이터 폴더(`%LOCALAPPDATA%\merge-master-agent`): `agent.json`(설정), `services\<id>\`(서비스마다 기록 파일·원본·상태),
//!   `logs\`, `calc\`(Excel 계산 사본).
//! - 플랫폼과 상관없는 부분(관리 API·서비스·마감·되살리기·경로)은 Linux에서도 시험한다([`FakeEngine`]).

pub mod agent;
pub mod api;
pub mod engine;
#[cfg(windows)]
pub mod excel;
pub mod logging;
pub mod paths;
pub mod record;
pub mod service;
pub mod settings;
#[cfg(windows)]
pub mod tray;
#[cfg(windows)]
pub mod win;

use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

pub use agent::{Agent, AgentError, AgentOptions, Clock, Overview, system_clock};
use collect_server::{ServerConfig, ServerHandle};
pub use engine::{Engine, FakeEngine};
use tokio::task::JoinHandle;

pub struct StartOptions {
    pub data_dir: PathBuf,
    pub engine: Arc<dyn Engine>,
    /// 설정의 포트 대신(개발·시험).
    pub port: Option<u16>,
    pub bind: IpAddr,
    pub clock: Clock,
    pub retry_delay: Duration,
    pub max_attempts: u32,
    /// 1초마다 마감을 점검한다(시험은 끄고 [`Agent::tick`]을 직접 부른다).
    pub ticker: bool,
}

impl StartOptions {
    pub fn new(data_dir: PathBuf, engine: Arc<dyn Engine>) -> Self {
        Self {
            data_dir,
            engine,
            port: None,
            bind: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            clock: system_clock(),
            retry_delay: Duration::from_secs(60),
            max_attempts: 3,
            ticker: true,
        }
    }
}

/// 실행 중인 에이전트.
pub struct Running {
    pub agent: Arc<Agent>,
    server: ServerHandle,
    ticker: Option<JoinHandle<()>>,
}

impl Running {
    pub fn port(&self) -> u16 {
        self.server.port()
    }

    /// 공유를 닫고(참여자에게는 잠시 멈춤) 포트를 닫는다. 서비스 기록은 남아 다시 켜면 이어서 한다.
    pub async fn stop(self) {
        if let Some(t) = self.ticker {
            t.abort();
        }
        self.agent.pause_all();
        self.server.stop().await;
    }
}

/// 설정을 읽고 포트를 열고(켤 때 열어 두어야 방화벽 허용 창이 사람이 있을 때 뜬다), 서비스를 되살린다.
pub async fn start(opts: StartOptions) -> Result<Running, String> {
    let agent = Agent::new(AgentOptions {
        data_dir: opts.data_dir,
        engine: opts.engine,
        clock: opts.clock,
        name: settings::pc_name(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        retry_delay: opts.retry_delay,
        max_attempts: opts.max_attempts.max(1),
    });
    let port = opts.port.unwrap_or_else(|| agent.settings().port);
    let config = ServerConfig { bind: opts.bind, port, dev_admin: false };
    let server =
        collect_server::start_with(config, agent.hub(), api::router(agent.clone())).await.map_err(|e| e.to_string())?;
    agent.set_port(server.port());
    agent.restore().await;
    let ticker = opts.ticker.then(|| agent.spawn_ticker());
    tracing::info!("에이전트 시작: 포트 {}, 기본 저장 폴더 {}", server.port(), agent.settings().base_dir.display());
    Ok(Running { agent, server, ticker })
}
