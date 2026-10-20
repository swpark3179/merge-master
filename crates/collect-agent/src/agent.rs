//! 에이전트: 넘겨받은 서비스들을 한 포트(참여자 서버 + 관리 API)로 서비스하고, 마감이 되면 엑셀로 저장해 끝낸다.
//!
//! - 받기([`Agent::create`]): 묶음을 서비스 폴더(`services\<id>\`)에 풀고, 기록 파일로 세션을 되살려 주소 경로로 공개한다.
//!   마감 동작은 '읽기 전용으로 전환'으로 바꿔 둔다(저장이 늦어도 마감 순간 입력은 바로 막힌다).
//! - 1초마다([`Agent::tick`]): 마감이 된 서비스는 저장하고 끝낸다. 실패하면 1분 뒤 다시(모두 3번), 그래도 안 되면
//!   '저장 실패'로 두고 공유를 닫는다(데이터는 남아 다시 저장할 수 있다).
//! - 다시 켜면([`Agent::restore`]): 서비스 중이던 것은 다시 공개하고, 꺼진 사이 마감이 지났으면 바로 저장한다.

use std::collections::{BTreeMap, HashSet};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use agent_protocol::{
    AgentInfo, Approve, Created, FolderList, MIN_LEAD_MINUTES, PROTOCOL, Package, ServiceState, ServiceStatus,
    StatusItem, StatusQuery, check_deadline, check_name, extension,
};
use chrono::{NaiveDateTime, TimeZone};
use collect_core::config::DeadlineAction;
use collect_core::edit::ConfigEdit;
use collect_server::{Gone, Hub, IpNames, META_ENDED, SessionHandle, lan_addresses, recover, validate_slug};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::engine::{Engine, Notify, SaveJob};
use crate::paths;
use crate::record::{Record, STORE_FILE};
use crate::service::{Service, View};
use crate::settings::Settings;

/// 지금 시각(에이전트 PC의 현지 시각). 시험은 바꿔 끼운다.
pub type Clock = Arc<dyn Fn() -> NaiveDateTime + Send + Sync>;

pub fn system_clock() -> Clock {
    Arc::new(|| chrono::Local::now().naive_local())
}

/// 끝낸 서비스 폴더를 남겨 두는 날 수(저장한 엑셀은 건드리지 않는다).
pub const KEEP_FINISHED_DAYS: i64 = 14;

pub struct AgentOptions {
    pub data_dir: PathBuf,
    pub engine: Arc<dyn Engine>,
    pub clock: Clock,
    /// 이 PC 이름.
    pub name: String,
    pub version: String,
    /// 저장에 실패하면 다시 해 볼 간격과 모두 몇 번 해 볼지.
    pub retry_delay: Duration,
    pub max_attempts: u32,
}

/// 관리 API 오류(상태 코드 + 한국어 문구).
#[derive(Debug, Clone, PartialEq)]
pub struct AgentError {
    pub status: u16,
    pub message: String,
}

impl AgentError {
    pub fn bad(message: impl Into<String>) -> Self {
        Self { status: 400, message: message.into() }
    }
    pub fn forbidden() -> Self {
        Self {
            status: 403,
            message: "이 서비스의 관리 키가 맞지 않습니다(넘긴 호스트 앱에서만 다룰 수 있습니다)".into(),
        }
    }
    pub fn missing() -> Self {
        Self { status: 404, message: "에이전트에 없는 서비스입니다".into() }
    }
    pub fn conflict(message: impl Into<String>) -> Self {
        Self { status: 409, message: message.into() }
    }
    pub fn internal(message: impl Into<String>) -> Self {
        Self { status: 500, message: message.into() }
    }
}

impl From<String> for AgentError {
    fn from(message: String) -> Self {
        Self::bad(message)
    }
}

/// 트레이 메뉴에 보일 서비스 한 줄.
#[derive(Debug, Clone, PartialEq)]
pub struct Overview {
    pub id: String,
    pub title: String,
    pub state: ServiceState,
    pub deadline: String,
    pub url: Option<String>,
    pub output_path: String,
    pub saved_path: Option<String>,
}

pub struct Agent {
    opts: AgentOptions,
    settings_path: PathBuf,
    settings: RwLock<Settings>,
    hub: Hub,
    services: Mutex<BTreeMap<String, Arc<Service>>>,
    /// 받기는 한 번에 하나(주소 경로 정하기가 겹치지 않게).
    creating: tokio::sync::Mutex<()>,
    port: AtomicU16,
    changes: watch::Sender<u64>,
}

impl Agent {
    pub fn new(opts: AgentOptions) -> Arc<Agent> {
        let settings_path = opts.data_dir.join("agent.json");
        let settings = Settings::load(&settings_path);
        if let Err(e) = std::fs::create_dir_all(&settings.base_dir) {
            tracing::warn!("기본 저장 폴더({})를 만들 수 없습니다: {e}", settings.base_dir.display());
        }
        let port = settings.port;
        Arc::new(Agent {
            opts,
            settings_path,
            settings: RwLock::new(settings),
            hub: Hub::new(),
            services: Mutex::new(BTreeMap::new()),
            creating: tokio::sync::Mutex::new(()),
            port: AtomicU16::new(port),
            changes: watch::channel(0).0,
        })
    }

    pub fn hub(&self) -> Hub {
        self.hub.clone()
    }

    pub fn settings(&self) -> Settings {
        self.settings.read().unwrap().clone()
    }

    pub fn settings_path(&self) -> &Path {
        &self.settings_path
    }

    pub fn data_dir(&self) -> &Path {
        &self.opts.data_dir
    }

    pub fn services_dir(&self) -> PathBuf {
        self.opts.data_dir.join("services")
    }

    pub fn version(&self) -> &str {
        &self.opts.version
    }

    pub fn port(&self) -> u16 {
        self.port.load(Ordering::SeqCst)
    }

    pub(crate) fn set_port(&self, port: u16) {
        self.port.store(port, Ordering::SeqCst);
    }

    /// 기본 저장 폴더를 바꾼다(이미 받은 서비스의 저장 위치는 그대로).
    pub fn set_base_dir(&self, dir: PathBuf) -> Result<(), String> {
        std::fs::create_dir_all(&dir).map_err(|e| format!("폴더를 만들 수 없습니다: {e}"))?;
        let mut s = self.settings.write().unwrap();
        s.base_dir = dir;
        s.save(&self.settings_path).map_err(|e| format!("설정을 남길 수 없습니다: {e}"))?;
        drop(s);
        self.changed();
        Ok(())
    }

    /// 서비스 목록·상태가 바뀔 때마다 늘어난다(트레이 메뉴를 다시 그린다).
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.changes.subscribe()
    }

    fn changed(&self) {
        self.changes.send_modify(|n| *n += 1);
    }

    fn now(&self) -> NaiveDateTime {
        (self.opts.clock)()
    }

    fn now_ms(&self) -> i64 {
        local_ms(self.now())
    }

    fn service(&self, id: &str) -> Option<Arc<Service>> {
        self.services.lock().unwrap().get(id).cloned()
    }

    fn all(&self) -> Vec<Arc<Service>> {
        self.services.lock().unwrap().values().cloned().collect()
    }

    /// 서비스 주소들(`slug`가 비면 업무 목록 주소).
    fn urls(&self, slug: &str) -> Vec<String> {
        let port = self.port();
        let mut ips: Vec<String> = lan_addresses().into_iter().map(|a| a.ip).collect();
        if ips.is_empty() {
            ips.push("127.0.0.1".into());
        }
        ips.into_iter().map(|ip| format!("http://{ip}:{port}/{slug}")).collect()
    }

    fn view<R>(&self, f: impl FnOnce(&View<'_>) -> R) -> R {
        let urls = |slug: &str| self.urls(slug);
        let deadline_ms = |text: &str| parse_deadline(text).map_or(0, local_ms);
        f(&View { urls: &urls, now_ms: self.now_ms(), deadline_ms: &deadline_ms })
    }

    fn owned(&self, id: &str, token: Option<&str>) -> Result<Arc<Service>, AgentError> {
        let service = self.service(id).ok_or_else(AgentError::missing)?;
        match token {
            Some(t) if !service.token_ok(t) => Err(AgentError::forbidden()),
            _ => Ok(service),
        }
    }

    // ------------------------------------------------------------------ 정보·폴더

    pub fn info(&self) -> AgentInfo {
        let s = self.settings();
        let base_ok = s.base_dir.is_dir() && paths::writable(&s.base_dir).is_ok();
        AgentInfo {
            name: self.opts.name.clone(),
            version: self.opts.version.clone(),
            protocol: PROTOCOL,
            base_dir: s.base_dir.display().to_string(),
            base_ok,
            port: self.port(),
            urls: self.urls(""),
            now_ms: self.now_ms(),
            utc_offset_minutes: chrono::Local::now().offset().local_minus_utc() / 60,
            excel: self.opts.engine.excel_available(),
            services: self.all().iter().filter(|s| !s.state().finished()).count() as u32,
            max_days: s.max_days,
        }
    }

    pub fn folders(&self, rel: &str) -> Result<FolderList, AgentError> {
        let segs = paths::split(rel)?;
        Ok(paths::list(&self.settings().base_dir, &segs)?)
    }

    pub fn overview(&self) -> Vec<Overview> {
        self.all()
            .iter()
            .map(|s| {
                let r = s.record();
                Overview {
                    id: s.id.clone(),
                    title: r.manifest.title.clone(),
                    state: r.state,
                    deadline: r.manifest.deadline.clone(),
                    url: (r.state == ServiceState::Running).then(|| self.urls(&r.slug).remove(0)),
                    output_path: r.output_path.display().to_string(),
                    saved_path: r.saved_path.clone(),
                }
            })
            .collect()
    }

    // ------------------------------------------------------------------ 받기

    /// 넘겨받는다. 같은 id·관리 키로 다시 보내면 이미 받은 서비스를 돌려준다(보낸 쪽이 응답을 놓친 경우).
    pub async fn create(self: &Arc<Self>, pkg: Package) -> Result<Created, AgentError> {
        let _one = self.creating.lock().await;
        let m = &pkg.manifest;
        if m.id.is_empty() || m.id.len() > 64 || !m.id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return Err(AgentError::bad("서비스 id가 올바르지 않습니다"));
        }
        if m.token.is_empty() || m.token.len() > 128 {
            return Err(AgentError::bad("관리 키가 올바르지 않습니다"));
        }
        if let Some(existing) = self.service(&m.id) {
            return if existing.token_ok(&m.token) {
                Ok(self.view(|v| existing.created(v)))
            } else {
                Err(AgentError::conflict("같은 id의 서비스가 이미 있습니다"))
            };
        }
        let settings = self.settings();
        check_deadline(&m.deadline, self.now(), MIN_LEAD_MINUTES, settings.max_days)?;
        if m.sheets.is_empty() {
            return Err(AgentError::bad("넘길 시트가 없습니다"));
        }
        let ext = paths::workbook_ext(&m.workbook_name)
            .ok_or_else(|| AgentError::bad(format!("‘{}’ 형식은 엑셀로 저장할 수 없습니다", m.workbook_name)))?;
        check_name(&m.output.file_name)?;
        if extension(&m.output.file_name) != ext {
            return Err(AgentError::bad(format!("저장할 파일은 원본과 같은 형식(.{ext})이어야 합니다")));
        }
        let dir = paths::join(&settings.base_dir, &m.output.dir)?;
        let output_path = dir.join(&m.output.file_name);
        paths::check_length(&output_path)?;
        paths::writable(&dir)?;
        if pkg.store.is_empty() || pkg.workbook.is_empty() {
            return Err(AgentError::bad("넘길 묶음에 기록 파일이나 원본이 없습니다"));
        }
        if !self.opts.engine.excel_available() {
            return Err(AgentError::bad("이 에이전트 PC에는 Excel이 없어 마감 때 저장할 수 없습니다"));
        }
        let slug = self.allocate_slug(&m.slug)?;

        // 서비스 폴더: 다 쓴 뒤 이름을 바꿔 넣는다(도중에 꺼지면 `.part`는 다음에 켤 때 지운다).
        let services = self.services_dir();
        let svc_dir = services.join(&m.id);
        let part = services.join(format!("{}.part", m.id));
        let source_file = format!("source.{ext}");
        let record = Record {
            manifest: m.clone(),
            slug: slug.clone(),
            state: ServiceState::Running,
            output_path,
            source_file: source_file.clone(),
            created_at_ms: self.now_ms(),
            attempts: 0,
            retries_left: 0,
            next_try_ms: None,
            ended_by_host: false,
            last_error: None,
            saved_path: None,
            saved_at_ms: None,
            finished_at_ms: None,
            last: None,
        };
        let write = || -> Result<(), String> {
            let _ = std::fs::remove_dir_all(&part);
            let _ = std::fs::remove_dir_all(&svc_dir); // 앞서 받다 만 것
            std::fs::create_dir_all(&part).map_err(|e| format!("서비스 폴더를 만들 수 없습니다: {e}"))?;
            std::fs::write(part.join(STORE_FILE), &pkg.store)
                .map_err(|e| format!("기록 파일을 쓸 수 없습니다: {e}"))?;
            std::fs::write(part.join(&source_file), &pkg.workbook)
                .map_err(|e| format!("원본을 쓸 수 없습니다: {e}"))?;
            record.save(&part)?;
            std::fs::rename(&part, &svc_dir).map_err(|e| format!("서비스 폴더를 만들 수 없습니다: {e}"))
        };
        write().map_err(AgentError::internal)?;

        match self.open(&svc_dir, record).await {
            Ok(service) => {
                tracing::info!(id = %service.id, slug, from = %m.host_pc, "서비스를 받았습니다: {}", m.title);
                let created = self.view(|v| service.created(v));
                self.services.lock().unwrap().insert(service.id.clone(), service);
                self.refresh_ip_names();
                self.changed();
                Ok(created)
            }
            Err(e) => {
                let _ = std::fs::remove_dir_all(&svc_dir);
                Err(e)
            }
        }
    }

    /// 바라는 주소 경로가 비어 있으면 그대로, 아니면 `-2`, `-3`…
    fn allocate_slug(&self, wanted: &str) -> Result<String, AgentError> {
        let wanted = if validate_slug(wanted).is_ok() { wanted.to_string() } else { "q".to_string() };
        let busy: HashSet<String> = self
            .all()
            .iter()
            .filter(|s| !s.state().finished())
            .map(|s| s.record().slug.to_lowercase())
            .chain(self.hub.sessions().into_iter().map(|(slug, _)| slug.to_lowercase()))
            .collect();
        let free = |s: &str| validate_slug(s).is_ok() && !busy.contains(&s.to_lowercase());
        if free(&wanted) {
            return Ok(wanted);
        }
        (2..1000)
            .map(|n| {
                let tail = format!("-{n}");
                let head: String = wanted.chars().take(collect_server::hub::MAX_SLUG_CHARS - tail.len()).collect();
                format!("{head}{tail}")
            })
            .find(|s| free(s))
            .ok_or_else(|| AgentError::conflict("쓸 수 있는 주소 경로가 없습니다"))
    }

    /// 서비스 폴더의 기록 파일로 세션을 되살려 서비스로 만든다(받을 때). 공개하고 계산 사본을 연다.
    async fn open(self: &Arc<Self>, dir: &Path, record: Record) -> Result<Arc<Service>, AgentError> {
        let store = dir.join(STORE_FILE);
        let rec = tokio::task::spawn_blocking(move || recover(&store))
            .await
            .map_err(|e| AgentError::internal(e.to_string()))?
            .map_err(|e| AgentError::bad(format!("기록 파일을 읽을 수 없습니다: {e}")))?;
        if rec.init.sheets.len() != record.manifest.sheets.len() {
            return Err(AgentError::bad("기록 파일의 시트 수가 넘길 내용과 다릅니다"));
        }
        let models = rec.init.sheets.iter().map(|(m, _)| m.clone()).collect();
        let session = SessionHandle::spawn(rec.init);
        let service = Service::new(dir.to_path_buf(), record, Some(session.clone()));
        let source = service.source_path().display().to_string();
        let deadline = service.record().manifest.deadline.clone();
        session
            .with(move |s| -> Result<(), String> {
                let c = s.sheets[0].config.clone();
                let meta = ConfigEdit::SetMeta {
                    title: c.title,
                    description: c.description,
                    requester: c.requester,
                    deadline: Some(deadline),
                };
                s.apply(0, meta)?;
                // 마감 순간 바로 입력을 막는다(저장은 에이전트가 이어서 한다).
                let mut permissions = s.sheets[0].config.permissions.clone();
                permissions.deadline_action = DeadlineAction::ReadOnly;
                s.apply(0, ConfigEdit::SetPermissions { permissions })?;
                s.set_closed(false);
                s.set_meta("workbook", &source);
                Ok(())
            })
            .await
            .map_err(AgentError::internal)??;
        self.attach(&service, &session);
        self.hub.publish(&service.record().slug, session.clone()).map_err(AgentError::conflict)?;
        self.start_recalc(&service, &session, models);
        Ok(service)
    }

    /// 세션에 넘겨받은 호스트 이름·IP 명단을 건다.
    fn attach(&self, service: &Service, session: &SessionHandle) {
        let m = service.record().manifest;
        session.set_host_name(m.host_name.clone());
        session.set_ip_names(ip_names_of(&m.ip_names));
    }

    fn start_recalc(&self, service: &Arc<Service>, session: &SessionHandle, models: Vec<collect_core::SheetModel>) {
        let (slot, changes) = (service.recalc.clone(), self.changes.clone());
        let notify: Notify = Arc::new(move |status| {
            *slot.lock().unwrap() = status.clone();
            changes.send_modify(|n| *n += 1);
        });
        let sheets = service.record().manifest.sheets;
        self.opts.engine.start_recalc(&service.id, session.clone(), models, service.source_path(), sheets, notify);
    }

    /// 모든 서비스의 IP 명단을 합쳐 업무 목록('내 담당')에 쓴다(입장은 서비스마다 자기 명단으로).
    fn refresh_ip_names(&self) {
        let mut all = IpNames::default();
        for s in self.all().iter().filter(|s| !s.state().finished()) {
            for (ip, name) in &s.record().manifest.ip_names {
                let _ = all.put(ip, name);
            }
        }
        self.hub.set_ip_names(all);
    }

    // ------------------------------------------------------------------ 상태·승인

    pub fn status(&self, queries: &[StatusQuery]) -> Vec<StatusItem> {
        queries
            .iter()
            .map(|q| match self.service(&q.id) {
                None => StatusItem { id: q.id.clone(), status: None, missing: true, forbidden: false },
                Some(s) if !s.token_ok(&q.token) => {
                    StatusItem { id: q.id.clone(), status: None, missing: false, forbidden: true }
                }
                Some(s) => StatusItem {
                    id: q.id.clone(),
                    status: Some(self.view(|v| s.status(v))),
                    missing: false,
                    forbidden: false,
                },
            })
            .collect()
    }

    pub fn status_of(&self, id: &str, token: Option<&str>) -> Result<ServiceStatus, AgentError> {
        let s = self.owned(id, token)?;
        Ok(self.view(|v| s.status(v)))
    }

    fn live_session(&self, s: &Service) -> Result<SessionHandle, AgentError> {
        match (s.state(), s.session()) {
            (ServiceState::Running, Some(session)) => Ok(session),
            _ => Err(AgentError::conflict("서비스 중이 아닙니다")),
        }
    }

    pub async fn approve(&self, id: &str, token: Option<&str>, a: Approve) -> Result<ServiceStatus, AgentError> {
        let s = self.owned(id, token)?;
        self.live_session(&s)?.approve(&a.name, None, &a.note).await?;
        self.changed();
        self.status_of(id, token)
    }

    pub async fn reject(&self, id: &str, token: Option<&str>, name: &str) -> Result<ServiceStatus, AgentError> {
        let s = self.owned(id, token)?;
        self.live_session(&s)?.reject(name).await?;
        self.changed();
        self.status_of(id, token)
    }

    // ------------------------------------------------------------------ 끝내기·저장

    /// 끝낸다. `save`면 엑셀로 저장하고 끝내고(저장은 뒤에서 — 상태는 '저장 중'), 아니면 저장 없이 끝낸다.
    /// `token`이 `None`이면 에이전트 PC(트레이)에서 한 것.
    pub async fn finish(
        self: &Arc<Self>,
        id: &str,
        token: Option<&str>,
        save: bool,
    ) -> Result<ServiceStatus, AgentError> {
        let s = self.owned(id, token)?;
        if s.state().finished() {
            return self.status_of(id, token);
        }
        if save {
            if s.state() == ServiceState::Saving {
                return self.status_of(id, token); // 이미 저장 중
            }
            self.begin_save(&s, true);
            tokio::spawn(self.clone().run_save(s.clone()));
        } else {
            let _g = s.saving.lock().await;
            if !s.state().finished() {
                self.close(&s, Gone::Ended).await;
                s.update(|r| {
                    r.state = ServiceState::Cancelled;
                    r.ended_by_host = true;
                    r.next_try_ms = None;
                    r.finished_at_ms = Some(local_ms((self.opts.clock)()));
                });
                tracing::info!(id = %s.id, "저장 없이 끝냈습니다");
                self.after_finish(&s);
            }
        }
        self.status_of(id, token)
    }

    /// '저장 실패'를 다시 저장한다.
    pub async fn retry(self: &Arc<Self>, id: &str, token: Option<&str>) -> Result<ServiceStatus, AgentError> {
        let s = self.owned(id, token)?;
        if s.state() != ServiceState::Failed {
            return Err(AgentError::conflict("저장에 실패한 서비스만 다시 저장할 수 있습니다"));
        }
        if s.session().is_none() {
            return Err(AgentError::conflict("이 서비스의 기록을 되살릴 수 없어 다시 저장할 수 없습니다"));
        }
        let by_host = s.record().ended_by_host;
        self.begin_save(&s, by_host);
        tokio::spawn(self.clone().run_save(s.clone()));
        self.status_of(id, token)
    }

    /// 저장을 시작한다(상태를 '저장 중'으로 — 다음 점검이 다시 시작하지 않게).
    fn begin_save(&self, s: &Service, by_host: bool) {
        let retries = self.opts.max_attempts.saturating_sub(1);
        s.update(|r| {
            r.state = ServiceState::Saving;
            r.ended_by_host = by_host;
            r.retries_left = retries;
            r.next_try_ms = None;
        });
        self.changed();
    }

    /// 저장한다(이미 '저장 중'으로 바꿔 둔 서비스). 성공하면 공유를 닫고 끝내고, 실패하면 다시 해 볼 시각을 정하거나
    /// '저장 실패'로 둔다.
    async fn run_save(self: Arc<Self>, s: Arc<Service>) {
        let _g = s.saving.lock().await;
        if s.state() != ServiceState::Saving {
            return;
        }
        let Some(session) = s.session() else {
            s.update(|r| {
                r.state = ServiceState::Failed;
                r.last_error = Some("이 서비스의 기록을 되살릴 수 없습니다".into());
            });
            self.changed();
            return;
        };
        // 저장하는 동안 입력을 막는다(참여자는 보기만).
        let _ = session.with(|s| s.set_closed(true)).await;
        let r = s.record();
        let result = async {
            let parts = session.save_parts().await?;
            let offset = chrono::Local::now().offset().local_minus_utc() / 60;
            let plans = parts.plan(&r.manifest.sheets, r.manifest.export, offset, &[]);
            if let Some(dir) = r.output_path.parent() {
                std::fs::create_dir_all(dir).map_err(|e| format!("저장할 폴더를 만들 수 없습니다: {e}"))?;
            }
            let out = paths::unique_file(&r.output_path);
            paths::check_length(&out)?;
            self.opts.engine.save(SaveJob { source: s.source_path(), plans, out }).await
        }
        .await;
        let why = if r.ended_by_host { Gone::Ended } else { Gone::Deadline };
        match result {
            Ok(path) => {
                let progress = s.progress();
                self.close(&s, why).await;
                s.update(|r| {
                    r.state = ServiceState::Done;
                    r.attempts += 1;
                    r.last_error = None;
                    r.saved_path = Some(path.clone());
                    r.saved_at_ms = Some(local_ms((self.opts.clock)()));
                    r.finished_at_ms = r.saved_at_ms;
                    r.last = Some(progress);
                });
                tracing::info!(id = %s.id, "저장하고 끝냈습니다: {path}");
                self.after_finish(&s);
            }
            Err(e) => {
                tracing::warn!(id = %s.id, "저장 실패: {e}");
                let retry_at = local_ms((self.opts.clock)()) + self.opts.retry_delay.as_millis() as i64;
                let r = s.update(|r| {
                    r.attempts += 1;
                    r.last_error = Some(e.clone());
                    if r.retries_left > 0 {
                        r.retries_left -= 1;
                        r.next_try_ms = Some(retry_at);
                    } else {
                        r.state = ServiceState::Failed;
                        r.next_try_ms = None;
                    }
                });
                if r.state == ServiceState::Failed {
                    // 공유는 닫는다(마감이 지났다). 세션은 남겨 다시 저장할 수 있다.
                    let progress = s.progress();
                    self.unpublish(&session, why);
                    s.update(|r| r.last = Some(progress));
                }
            }
        }
        self.changed();
    }

    /// 공유를 닫고 계산 사본을 닫는다. 기록 파일에 '끝냄'을 남긴다.
    async fn close(&self, s: &Service, why: Gone) {
        if let Some(session) = s.session() {
            self.unpublish(&session, why);
            session.set_recalc(None);
            let _ = session.with(|x| x.set_meta(META_ENDED, "1")).await;
        }
        self.opts.engine.stop_recalc(&s.id);
    }

    fn unpublish(&self, session: &SessionHandle, why: Gone) {
        if let Some(slug) = self.hub.slug_of(session) {
            self.hub.unpublish_as(&slug, why);
        }
    }

    fn after_finish(&self, s: &Service) {
        s.drop_session();
        self.refresh_ip_names();
        self.changed();
    }

    /// 1초마다: 마감이 된 서비스는 저장을 시작하고, 다시 해 볼 시각이 된 저장을 다시 한다. 시작한 저장 작업을 돌려준다.
    pub fn tick(self: &Arc<Self>) -> Vec<JoinHandle<()>> {
        let now = self.now();
        let now_ms = local_ms(now);
        let mut started = Vec::new();
        for s in self.all() {
            let r = s.record();
            let due = match r.state {
                ServiceState::Running => parse_deadline(&r.manifest.deadline).is_some_and(|d| now >= d),
                ServiceState::Saving => r.next_try_ms.is_some_and(|t| now_ms >= t),
                _ => false,
            };
            if !due {
                continue;
            }
            if r.state == ServiceState::Running {
                tracing::info!(id = %s.id, "마감 시각이 되어 저장합니다");
                self.begin_save(&s, false);
            } else {
                s.update(|r| r.next_try_ms = None);
            }
            started.push(tokio::spawn(self.clone().run_save(s)));
        }
        started
    }

    /// 1초마다 [`Agent::tick`].
    pub fn spawn_ticker(self: &Arc<Self>) -> JoinHandle<()> {
        let agent = Arc::downgrade(self);
        tokio::spawn(async move {
            let mut every = tokio::time::interval(Duration::from_secs(1));
            loop {
                every.tick().await;
                let Some(agent) = agent.upgrade() else { break };
                agent.tick();
            }
        })
    }

    // ------------------------------------------------------------------ 다시 켜기

    /// 서비스 폴더에서 되살린다. 서비스 중이던 것은 다시 공개하고, 꺼진 사이 마감이 지났거나 저장하다 꺼졌으면 바로
    /// 저장하게 둔다. 오래된(14일) 끝난 서비스 폴더는 지운다.
    pub async fn restore(self: &Arc<Self>) {
        self.opts.engine.cleanup().await;
        let Ok(entries) = std::fs::read_dir(self.services_dir()) else { return };
        let now = self.now();
        let now_ms = local_ms(now);
        for e in entries.flatten() {
            let dir = e.path();
            if !dir.is_dir() {
                continue;
            }
            if dir.extension().is_some_and(|x| x == "part") {
                let _ = std::fs::remove_dir_all(&dir);
                continue;
            }
            let record = match Record::load(&dir) {
                Ok(r) => r,
                Err(err) => {
                    tracing::warn!("{}: {err}", dir.display());
                    continue;
                }
            };
            if record.state.finished() {
                let old = record.finished_at_ms.is_some_and(|t| now_ms - t > KEEP_FINISHED_DAYS * 86_400_000);
                if old {
                    let _ = std::fs::remove_dir_all(&dir);
                } else {
                    let s = Service::new(dir, record, None);
                    self.services.lock().unwrap().insert(s.id.clone(), s);
                }
                continue;
            }
            let service = match self.reopen(&dir, record.clone()).await {
                Ok(s) => s,
                Err(err) => {
                    tracing::error!("{}: 되살릴 수 없습니다: {err}", dir.display());
                    let s = Service::new(dir, record, None);
                    s.update(|r| {
                        r.state = ServiceState::Failed;
                        r.next_try_ms = None;
                        r.last_error = Some(format!("에이전트를 다시 켰지만 기록을 되살릴 수 없습니다: {err}"));
                    });
                    self.services.lock().unwrap().insert(s.id.clone(), s);
                    continue;
                }
            };
            let r = service.record();
            let passed = parse_deadline(&r.manifest.deadline).is_none_or(|d| now >= d);
            match r.state {
                ServiceState::Running if passed => {
                    tracing::info!(id = %service.id, "꺼진 사이 마감이 지나 바로 저장합니다");
                    self.begin_save(&service, false);
                    service.update(|r| r.next_try_ms = Some(now_ms));
                }
                ServiceState::Saving => {
                    service.update(|r| r.next_try_ms = Some(now_ms));
                }
                _ => {}
            }
            self.services.lock().unwrap().insert(service.id.clone(), service);
        }
        self.refresh_ip_names();
        self.changed();
    }

    /// 다시 켤 때 서비스 하나: 세션을 되살리고, 서비스 중이고 마감 전이면 다시 공개한다.
    async fn reopen(self: &Arc<Self>, dir: &Path, record: Record) -> Result<Arc<Service>, String> {
        let store = dir.join(STORE_FILE);
        let rec = tokio::task::spawn_blocking(move || recover(&store)).await.map_err(|e| e.to_string())??;
        let models = rec.init.sheets.iter().map(|(m, _)| m.clone()).collect();
        let session = SessionHandle::spawn(rec.init);
        let passed = parse_deadline(&record.manifest.deadline).is_none_or(|d| self.now() >= d);
        let running = record.state == ServiceState::Running && !passed;
        let service = Service::new(dir.to_path_buf(), record, Some(session.clone()));
        self.attach(&service, &session);
        if running {
            self.hub.publish(&service.record().slug, session.clone())?;
            self.start_recalc(&service, &session, models);
            tracing::info!(id = %service.id, "서비스를 이어서 합니다");
        } else {
            let _ = session.with(|s| s.set_closed(true)).await;
        }
        Ok(service)
    }

    /// 에이전트를 끝낼 때: 공유를 닫고(참여자에게는 잠시 멈춘 것으로 — 다시 켜면 이어서 한다), 계산 사본을 닫는다.
    pub async fn pause_all(&self) {
        for (slug, _) in self.hub.sessions() {
            self.hub.unpublish_as(&slug, Gone::Paused);
        }
        for s in self.all() {
            if let Some(session) = s.session() {
                session.set_recalc(None);
            }
        }
        self.opts.engine.shutdown().await;
    }
}

fn ip_names_of(map: &BTreeMap<String, String>) -> IpNames {
    let mut names = IpNames::default();
    for (ip, name) in map {
        if ip.parse::<IpAddr>().is_ok() {
            let _ = names.put(ip, name);
        }
    }
    names
}

/// 마감 글자 → 시각(`YYYY-MM-DDTHH:MM`, 초까지 있어도 된다).
pub fn parse_deadline(text: &str) -> Option<NaiveDateTime> {
    let t = text.trim();
    NaiveDateTime::parse_from_str(t, "%Y-%m-%dT%H:%M")
        .or_else(|_| NaiveDateTime::parse_from_str(t, "%Y-%m-%dT%H:%M:%S"))
        .ok()
}

/// 이 PC 현지 시각 → 유닉스 ms.
pub fn local_ms(t: NaiveDateTime) -> i64 {
    chrono::Local
        .from_local_datetime(&t)
        .earliest()
        .map_or_else(|| t.and_utc().timestamp_millis(), |x| x.timestamp_millis())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deadline_text() {
        assert!(parse_deadline("2026-10-30T18:00").is_some());
        assert!(parse_deadline("2026-10-30T18:00:30").is_some());
        assert!(parse_deadline("2026-10-30").is_none());
        let t = parse_deadline("2026-10-30T18:00").unwrap();
        assert!(local_ms(t) > 0);
    }

    #[test]
    fn ip_names_skip_bad_entries() {
        let map = BTreeMap::from([("10.0.0.5".to_string(), "이서연".to_string()), ("x".into(), "y".into())]);
        assert_eq!(ip_names_of(&map).len(), 1);
    }
}
