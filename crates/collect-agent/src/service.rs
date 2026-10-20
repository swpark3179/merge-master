//! 넘겨받은 서비스 하나: 서비스 폴더(기록 파일·원본·기록), 세션, 수식 계산 상태.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use agent_protocol::{Created, PendingPerson, ServiceState, ServiceStatus};
use collect_server::{RecalcStatus, SessionHandle};

use crate::record::{Progress, Record, STORE_FILE};

pub struct Service {
    pub id: String,
    pub dir: PathBuf,
    record: Mutex<Record>,
    /// 서비스 중·저장 전이면 있다(끝나면 놓는다).
    session: Mutex<Option<SessionHandle>>,
    pub(crate) recalc: Arc<Mutex<RecalcStatus>>,
    /// 저장은 한 번에 하나.
    pub(crate) saving: tokio::sync::Mutex<()>,
    /// 관리자 연결의 쓰기(읽기 잠금)와 저장할 내용 뜨기(쓰기 잠금)가 겹치지 않게.
    pub(crate) edits: tokio::sync::RwLock<()>,
}

/// 상태를 만들 때 에이전트가 주는 것.
pub(crate) struct View<'a> {
    pub urls: &'a dyn Fn(&str) -> Vec<String>,
    pub now_ms: i64,
    pub deadline_ms: &'a dyn Fn(&str) -> i64,
}

impl Service {
    pub(crate) fn new(dir: PathBuf, record: Record, session: Option<SessionHandle>) -> Arc<Service> {
        Arc::new(Service {
            id: record.manifest.id.clone(),
            dir,
            record: Mutex::new(record),
            session: Mutex::new(session),
            recalc: Arc::new(Mutex::new(RecalcStatus::None)),
            saving: tokio::sync::Mutex::new(()),
            edits: tokio::sync::RwLock::new(()),
        })
    }

    pub fn record(&self) -> Record {
        self.record.lock().unwrap().clone()
    }

    pub fn state(&self) -> ServiceState {
        self.record.lock().unwrap().state
    }

    /// 기록을 고치고 파일에 남긴다(남기지 못해도 메모리의 상태는 바꾼다 — 로그만).
    pub(crate) fn update(&self, f: impl FnOnce(&mut Record)) -> Record {
        let mut r = self.record.lock().unwrap();
        f(&mut r);
        if let Err(e) = r.save(&self.dir) {
            tracing::error!(id = %self.id, "{e}");
        }
        r.clone()
    }

    pub fn session(&self) -> Option<SessionHandle> {
        self.session.lock().unwrap().clone()
    }

    pub(crate) fn drop_session(&self) {
        self.session.lock().unwrap().take();
    }

    /// 관리 키가 맞는가(걸린 시간으로 앞 글자가 맞는지 알 수 없게 끝까지 비교한다).
    pub(crate) fn token_ok(&self, token: &str) -> bool {
        let r = self.record.lock().unwrap();
        let (a, b) = (r.manifest.token.as_bytes(), token.as_bytes());
        a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
    }

    /// 수식 계산 상태(서비스 중일 때만).
    pub fn recalc_status(&self) -> RecalcStatus {
        if self.state() == ServiceState::Running { self.recalc.lock().unwrap().clone() } else { RecalcStatus::None }
    }

    pub fn source_path(&self) -> PathBuf {
        self.dir.join(&self.record.lock().unwrap().source_file)
    }

    pub fn store_path(&self) -> PathBuf {
        self.dir.join(STORE_FILE)
    }

    /// 지금 진행률(세션이 없으면 끝났을 때 남긴 것).
    pub(crate) fn progress(&self) -> Progress {
        match self.session() {
            Some(s) => {
                let snap = s.snapshot();
                Progress { roster: snap.roster, required: snap.required, filled: snap.filled }
            }
            None => self.record().last.unwrap_or_default(),
        }
    }

    pub(crate) fn created(&self, view: &View<'_>) -> Created {
        let r = self.record();
        Created {
            id: self.id.clone(),
            slug: r.slug.clone(),
            urls: (view.urls)(&r.slug),
            list_urls: (view.urls)(""),
            output_path: r.output_path.display().to_string(),
            deadline: r.manifest.deadline.clone(),
        }
    }

    pub(crate) fn status(&self, view: &View<'_>) -> ServiceStatus {
        let r = self.record();
        let session = self.session();
        let snap = session.as_ref().map(SessionHandle::snapshot);
        let last = r.last.unwrap_or_default();
        let running = r.state == ServiceState::Running;
        let (urls, list_urls) =
            if running { ((view.urls)(&r.slug), (view.urls)("")) } else { (Vec::new(), Vec::new()) };
        ServiceStatus {
            id: self.id.clone(),
            title: r.manifest.title.clone(),
            slug: r.slug.clone(),
            state: r.state,
            urls,
            list_urls,
            deadline: r.manifest.deadline.clone(),
            deadline_at_ms: (view.deadline_ms)(&r.manifest.deadline),
            now_ms: view.now_ms,
            online: snap.as_ref().map_or(0, |s| s.online),
            roster: snap.as_ref().map_or(last.roster, |s| s.roster),
            pending: snap
                .as_ref()
                .map(|s| {
                    s.pending
                        .iter()
                        .map(|p| PendingPerson { name: p.name.clone(), ip: p.ip.clone(), reason: p.reason })
                        .collect()
                })
                .unwrap_or_default(),
            required: snap.as_ref().map_or(last.required, |s| s.required),
            filled: snap.as_ref().map_or(last.filled, |s| s.filled),
            read_only: snap.as_ref().is_none_or(|s| s.read_only),
            recalc: self.recalc_status(),
            output_path: r.output_path.display().to_string(),
            saved_path: r.saved_path.clone(),
            saved_at_ms: r.saved_at_ms,
            attempts: r.attempts,
            last_error: r.last_error.clone(),
            last_chat_ms: session.as_ref().and_then(|s| s.comms().chat.last().map(|c| c.at_ms)),
            created_at_ms: r.created_at_ms,
        }
    }
}
