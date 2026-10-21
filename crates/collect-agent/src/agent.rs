//! 에이전트: 넘겨받은 서비스들을 한 포트(참여자 서버 + 관리 API)로 서비스하고, 마감이 되면 엑셀로 저장해 끝낸다.
//!
//! - 받기([`Agent::create`]): 묶음을 서비스 폴더(`services\<id>\`)에 풀고, 기록 파일로 세션을 되살려 주소 경로로 공개한다.
//!   마감 동작은 '읽기 전용으로 전환'으로 바꿔 둔다(저장이 늦어도 마감 순간 입력은 바로 막힌다). 서비스 중에는 Excel을
//!   열지 않는다(수식 칸은 넘겨받은 값 그대로 — 저장할 때 Excel이 계산한다).
//! - 1초마다([`Agent::tick`]): 마감이 된 서비스는 취합 완료 데이터(`result.mmsave`)부터 남기고, 이 PC에 Excel이 있으면
//!   별도 저장 프로세스에서 엑셀로 저장하고 끝낸다. 실패하면 1분 뒤 다시(모두 3번), 그래도 안 되거나 Excel이 없으면
//!   '관리자 저장 대기'로 두고 공유를 닫는다 — 넘긴 호스트 앱이 데이터를 받아 그 PC에서 저장하면 데이터를 지우고 끝낸다
//!   ([`Agent::host_saved`]).
//! - 다시 켜면([`Agent::restore`]): 서비스 중이던 것은 다시 공개하고, 꺼진 사이 마감이 지났으면 바로 저장한다.
//! - 참여자: 명단 밖에서도 입력한 이름 그대로 들어온다(자동 승인 — 관리자가 승인하지 못할 수 있다).
//! - IP 명단: 에이전트 PC의 명단(`ip-names.json`, 트레이 'IP 명단 관리')이 이긴다. 서비스마다 넘겨받은 명단은 에이전트
//!   명단에 없는 IP만 그 서비스에서 쓴다(에이전트 명단에 섞지 않는다).
//! - 관리자 연결([`crate::host_link`]): 넘긴 호스트 앱이 설정·모니터링·소통·히스토리·참여자 승인을 이 PC 업무처럼 한다
//!   ([`Agent::host_request`]). 마감은 비울 수 없고, 마감 동작은 '읽기 전용'(에이전트가 저장한다)으로 둔다.

use std::collections::{BTreeMap, HashSet};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use agent_protocol::link::LinkState;
use agent_protocol::save_data::SaveData;
use agent_protocol::{
    AgentInfo, Approve, Created, FolderList, HostSaved, MIN_LEAD_MINUTES, PROTOCOL, Package, ServiceState,
    ServiceStatus, StatusItem, StatusQuery, check_deadline, check_name, extension,
};
use chrono::{NaiveDateTime, TimeZone};
use collect_core::config::DeadlineAction;
use collect_core::edit::ConfigEdit;
use collect_server::host::{self, HostRequest};
use collect_server::{
    Gone, Hub, IpNames, META_ENDED, RecalcStatus, SaveParts, SessionHandle, lan_addresses, recover, validate_slug,
};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::engine::{Engine, SaveJob};
use crate::paths;
use crate::record::{Record, STORE_FILE};
use crate::service::{Service, View};
use crate::settings::{Settings, write_synced};

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
    pub saved_on_host: bool,
    /// 서비스 폴더(취합 데이터·원본·기록).
    pub dir: PathBuf,
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
    /// 에이전트 PC의 IP 명단(`ip-names.json`).
    ip_names: RwLock<IpNames>,
    ip_names_path: PathBuf,
    /// 에이전트를 끝내는 중(관리자 연결을 닫는다).
    stopping: watch::Sender<bool>,
}

/// 에이전트 PC의 IP 명단 파일 이름(데이터 폴더).
pub const IP_NAMES_FILE: &str = "ip-names.json";

impl Agent {
    pub fn new(opts: AgentOptions) -> Arc<Agent> {
        let settings_path = opts.data_dir.join("agent.json");
        let settings = Settings::load(&settings_path);
        if let Err(e) = std::fs::create_dir_all(&settings.base_dir) {
            tracing::warn!("기본 저장 폴더({})를 만들 수 없습니다: {e}", settings.base_dir.display());
        }
        let port = settings.port;
        let ip_names_path = opts.data_dir.join(IP_NAMES_FILE);
        let ip_names = match std::fs::read_to_string(&ip_names_path) {
            Ok(text) => IpNames::parse_json(&text).unwrap_or_else(|e| {
                tracing::warn!("IP 명단({})을 읽을 수 없습니다: {e}", ip_names_path.display());
                IpNames::default()
            }),
            Err(_) => IpNames::default(),
        };
        Arc::new(Agent {
            opts,
            settings_path,
            settings: RwLock::new(settings),
            hub: Hub::new(),
            services: Mutex::new(BTreeMap::new()),
            creating: tokio::sync::Mutex::new(()),
            port: AtomicU16::new(port),
            changes: watch::channel(0).0,
            ip_names: RwLock::new(ip_names),
            ip_names_path,
            stopping: watch::channel(false).0,
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

    /// 에이전트를 끝낼 때 `true`가 된다.
    pub fn stopping(&self) -> watch::Receiver<bool> {
        self.stopping.subscribe()
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

    /// 지금 받아 둔 서비스들(끝난 것 포함).
    pub fn services_now(&self) -> Vec<Arc<Service>> {
        self.all()
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
        f(&View {
            urls: &urls,
            now_ms: self.now_ms(),
            deadline_ms: &deadline_ms,
            excel: self.opts.engine.excel_available(),
        })
    }

    pub(crate) fn owned(&self, id: &str, token: Option<&str>) -> Result<Arc<Service>, AgentError> {
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
                    saved_on_host: r.saved_on_host,
                    dir: s.dir.clone(),
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
            result_file: false,
            excel_started_ms: None,
            saved_on_host: false,
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

    /// 서비스 폴더의 기록 파일로 세션을 되살려 서비스로 만든다(받을 때). 공개한다.
    async fn open(self: &Arc<Self>, dir: &Path, record: Record) -> Result<Arc<Service>, AgentError> {
        let store = dir.join(STORE_FILE);
        let rec = tokio::task::spawn_blocking(move || recover(&store))
            .await
            .map_err(|e| AgentError::internal(e.to_string()))?
            .map_err(|e| AgentError::bad(format!("기록 파일을 읽을 수 없습니다: {e}")))?;
        if rec.init.sheets.len() != record.manifest.sheets.len() {
            return Err(AgentError::bad("기록 파일의 시트 수가 넘길 내용과 다릅니다"));
        }
        let recalc = formula_status(rec.init.sheets.iter().map(|(m, _)| m));
        let session = SessionHandle::spawn(rec.init);
        let service = Service::new(dir.to_path_buf(), record, Some(session.clone()));
        service.set_recalc(recalc);
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
        Ok(service)
    }

    /// 세션에 넘겨받은 호스트 이름·IP 명단(에이전트 명단이 이긴다)을 걸고 자동 승인을 켠다(공개하기 전에).
    fn attach(&self, service: &Service, session: &SessionHandle) {
        let m = service.record().manifest;
        session.set_host_name(m.host_name.clone());
        session.set_ip_names(self.effective_ip_names(service));
        session.set_auto_approve(true);
    }

    // ------------------------------------------------------------------ IP 명단

    /// 에이전트 PC의 IP 명단.
    pub fn ip_names(&self) -> IpNames {
        self.ip_names.read().unwrap().clone()
    }

    pub fn ip_names_path(&self) -> &Path {
        &self.ip_names_path
    }

    /// 에이전트 명단을 바꾼다(파일에 남기고 서비스 중인 세션마다 다시 건다 — 기다리던 사람은 바로 들어온다).
    pub fn set_ip_names(&self, names: IpNames) -> Result<(), String> {
        crate::settings::write_atomic(&self.ip_names_path, names.to_json().as_bytes())
            .map_err(|e| format!("IP 명단을 남길 수 없습니다: {e}"))?;
        *self.ip_names.write().unwrap() = names;
        self.refresh_ip_names();
        self.changed();
        Ok(())
    }

    /// 한 사람을 더하거나 이름을 바꾼다.
    pub fn put_ip_name(&self, ip: &str, name: &str) -> Result<(), String> {
        let mut names = self.ip_names();
        names.put(ip, name)?;
        self.set_ip_names(names)
    }

    /// 한 사람을 뺀다.
    pub fn remove_ip_name(&self, ip: &str) -> Result<(), String> {
        let ip: IpAddr = ip.trim().parse().map_err(|_| format!("IP 주소가 잘못되었습니다: {ip}"))?;
        let mut names = self.ip_names();
        names.remove(ip);
        self.set_ip_names(names)
    }

    /// 그 서비스가 넘겨받은 명단에서 한 사람을 뺀다(그 서비스에서만 쓰던 것).
    pub fn remove_received(&self, id: &str, ip: &str) -> Result<(), AgentError> {
        let s = self.service(id).ok_or_else(AgentError::missing)?;
        let target: IpAddr =
            ip.trim().parse().map_err(|_| AgentError::bad(format!("IP 주소가 잘못되었습니다: {ip}")))?;
        s.update(|r| {
            r.manifest
                .ip_names
                .retain(|k, _| k.parse::<IpAddr>().map_or(true, |a| a.to_canonical() != target.to_canonical()))
        });
        self.refresh_ip_names();
        self.changed();
        Ok(())
    }

    /// 그 서비스가 넘겨받은 명단을 바꾼다(넘긴 호스트가 명단을 고쳤다).
    pub fn set_received(&self, id: &str, names: BTreeMap<String, String>) -> Result<(), AgentError> {
        let s = self.service(id).ok_or_else(AgentError::missing)?;
        s.update(|r| r.manifest.ip_names = names);
        self.refresh_ip_names();
        self.changed();
        Ok(())
    }

    /// 그 서비스에서 쓰는 명단: 에이전트 명단 + 넘겨받은 명단 중 에이전트 명단에 없는 IP.
    pub fn effective_ip_names(&self, service: &Service) -> IpNames {
        let mut names = self.ip_names();
        names.merge_missing(&ip_names_of(&service.record().manifest.ip_names));
        names
    }

    /// 서비스 중인 세션마다 명단을 다시 걸고, 업무 목록('내 담당')에는 모든 명단을 합쳐 쓴다(에이전트 명단이 이긴다).
    fn refresh_ip_names(&self) {
        let mut all = self.ip_names();
        for s in self.all().iter().filter(|s| !s.state().finished()) {
            if let Some(session) = s.session() {
                session.set_ip_names(self.effective_ip_names(s));
            }
            all.merge_missing(&ip_names_of(&s.record().manifest.ip_names));
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

    pub(crate) fn live_session(&self, s: &Service) -> Result<SessionHandle, AgentError> {
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

    // ------------------------------------------------------------------ 관리자 연결

    /// 관리자 연결의 요청 하나([`crate::host_link`]). 보기는 세션이 남아 있으면 언제든, 바꾸기는 서비스 중일 때만(저장을
    /// 시작하면 막는다 — 저장하는 파일에 빠진 값이 생기지 않게). 에이전트 업무의 규칙: 마감은 비울 수 없고 바꾸면 받을 때와
    /// 같은 범위(지금부터 1분 뒤 ~ 최대 서비스 기간)여야 하며 저장할 때도 그 시각을 쓴다. 마감 동작은 늘 '읽기 전용'
    /// (마감 순간 입력을 막고 에이전트가 이어서 저장한다).
    pub async fn host_request(&self, s: &Service, req: HostRequest) -> Result<serde_json::Value, String> {
        if req.is_read() {
            let session = s.session().ok_or("이 서비스는 끝났습니다")?;
            return host::execute(&session, req).await;
        }
        let _edit = s.edits.read().await;
        let session = self.live_session(s).map_err(|e| e.message)?;
        let mut req = req;
        let mut moved = None;
        if let HostRequest::ConfigApply { edit, .. } = &mut req {
            match edit {
                ConfigEdit::SetMeta { deadline, .. } => {
                    let text = deadline
                        .as_deref()
                        .map(str::trim)
                        .filter(|t| !t.is_empty())
                        .ok_or("에이전트 PC에서 서비스하는 업무는 마감 일시를 비울 수 없습니다")?
                        .to_string();
                    if text != s.record().manifest.deadline {
                        check_deadline(&text, self.now(), MIN_LEAD_MINUTES, self.settings().max_days)?;
                        moved = Some(text.clone());
                    }
                    *deadline = Some(text);
                }
                ConfigEdit::SetPermissions { permissions } => permissions.deadline_action = DeadlineAction::ReadOnly,
                _ => {}
            }
        }
        let out = host::execute(&session, req).await?;
        if let Some(deadline) = moved {
            tracing::info!(id = %s.id, "마감을 바꿨습니다: {deadline}");
            s.update(|r| r.manifest.deadline = deadline);
            self.changed();
        }
        Ok(out)
    }

    /// 관리자 연결이 처음 그릴 것.
    pub async fn link_state(&self, s: &Service) -> Result<LinkState, String> {
        let session = s.session().ok_or("이 서비스는 끝났습니다")?;
        let models = host::execute(&session, HostRequest::Models).await?;
        let value = |v: serde_json::Result<serde_json::Value>| v.map_err(|e| e.to_string());
        Ok(LinkState {
            models,
            snapshot: value(serde_json::to_value(session.snapshot()))?,
            comms: value(serde_json::to_value(session.comms()))?,
            recalc: s.recalc_status(),
            status: self.view(|v| s.status(v)),
        })
    }

    // ------------------------------------------------------------------ 끝내기·저장

    /// 끝낸다. `save`면 엑셀로 저장하고 끝내고(저장은 뒤에서 — 상태는 '저장 중'), 아니면 저장 없이 끝낸다(남긴 취합 데이터도
    /// 지운다). `token`이 `None`이면 에이전트 PC(트레이)에서 한 것.
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
            match s.state() {
                ServiceState::Saving => return self.status_of(id, token), // 이미 저장 중
                ServiceState::Failed | ServiceState::AwaitingHost => return self.retry(id, token).await,
                _ => {}
            }
            self.begin_save(&s, true);
            tokio::spawn(self.clone().run_save(s.clone()));
        } else {
            let _g = s.saving.lock().await;
            if !s.state().finished() {
                self.close(&s, Gone::Ended).await;
                remove_result(&s);
                s.update(|r| {
                    r.state = ServiceState::Cancelled;
                    r.ended_by_host = true;
                    r.next_try_ms = None;
                    r.result_file = false;
                    r.finished_at_ms = Some(local_ms((self.opts.clock)()));
                });
                tracing::info!(id = %s.id, "저장 없이 끝냈습니다");
                self.after_finish(&s);
            }
        }
        self.status_of(id, token)
    }

    /// '저장 실패'·'관리자 저장 대기'를 이 PC에서 다시 저장한다.
    pub async fn retry(self: &Arc<Self>, id: &str, token: Option<&str>) -> Result<ServiceStatus, AgentError> {
        let s = self.owned(id, token)?;
        let r = s.record();
        match r.state {
            ServiceState::Failed if s.session().is_none() && !r.result_file => {
                return Err(AgentError::conflict("이 서비스의 기록을 되살릴 수 없어 다시 저장할 수 없습니다"));
            }
            ServiceState::AwaitingHost if !self.opts.engine.excel_available() => {
                return Err(AgentError::conflict(
                    "이 에이전트 PC에는 Excel이 없어 저장할 수 없습니다. 넘긴 호스트 앱(관리자 PC)에서 저장해 주세요",
                ));
            }
            ServiceState::Failed | ServiceState::AwaitingHost => {}
            _ => return Err(AgentError::conflict("저장에 실패한 서비스만 다시 저장할 수 있습니다")),
        }
        self.begin_save(&s, r.ended_by_host);
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

    /// 저장한다(이미 '저장 중'으로 바꿔 둔 서비스). 먼저 취합 완료 데이터를 서비스 폴더에 남기고(한 번만 — 다시 해 볼 때는
    /// 남긴 것을 쓴다), 이 PC에 Excel이 있으면 별도 저장 프로세스에서 엑셀로 저장한다. 성공하면 데이터를 지우고 공유를
    /// 닫고 끝낸다. 실패하면 다시 해 볼 시각을 정하고, 다 해 봤거나 Excel이 없으면 '관리자 저장 대기'로 둔다(데이터도
    /// 남기지 못했으면 '저장 실패').
    async fn run_save(self: Arc<Self>, s: Arc<Service>) {
        let _g = s.saving.lock().await;
        if s.state() != ServiceState::Saving {
            return;
        }
        let why = if s.record().ended_by_host { Gone::Ended } else { Gone::Deadline };
        if !s.record().result_file
            && let Err(e) = self.freeze(&s).await
        {
            tracing::error!(id = %s.id, "취합 데이터를 남길 수 없습니다: {e}");
            self.attempt_failed(&s, why, format!("취합 데이터를 남길 수 없습니다: {e}")).await;
            self.changed();
            return;
        }
        if !self.opts.engine.excel_available() {
            tracing::info!(id = %s.id, "이 PC에 Excel이 없어 관리자 PC의 저장을 기다립니다");
            self.await_host(&s, why).await;
            self.changed();
            return;
        }
        let r = s.update(|r| r.excel_started_ms = Some(local_ms((self.opts.clock)())));
        self.changed();
        let result = async {
            if let Some(dir) = r.output_path.parent() {
                std::fs::create_dir_all(dir).map_err(|e| format!("저장할 폴더를 만들 수 없습니다: {e}"))?;
            }
            let out = paths::unique_file(&r.output_path);
            paths::check_length(&out)?;
            self.opts.engine.save(SaveJob { data: s.result_path(), source: s.source_path(), out }).await
        }
        .await;
        match result {
            Ok(path) => {
                let progress = s.progress();
                self.close(&s, why).await;
                remove_result(&s);
                s.update(|r| {
                    r.state = ServiceState::Done;
                    r.attempts += 1;
                    r.last_error = None;
                    r.result_file = false;
                    r.excel_started_ms = None;
                    r.saved_on_host = false;
                    r.saved_path = Some(path.clone());
                    r.saved_at_ms = Some(local_ms((self.opts.clock)()));
                    r.finished_at_ms = r.saved_at_ms;
                    r.last = Some(progress);
                });
                tracing::info!(id = %s.id, "저장하고 끝냈습니다: {path}");
                self.after_finish(&s);
            }
            // 에이전트를 끝내 저장을 그만뒀다: 시도로 세지 않는다(다시 켜면 바로 다시 한다).
            Err(e) if *self.stopping.borrow() => {
                tracing::info!(id = %s.id, "에이전트를 끝내 저장을 멈춥니다: {e}");
                s.update(|r| r.excel_started_ms = None);
            }
            Err(e) => {
                tracing::warn!(id = %s.id, "저장 실패: {e}");
                self.attempt_failed(&s, why, e).await;
            }
        }
        self.changed();
    }

    /// 저장할 내용을 떠(입력을 막고) 취합 완료 데이터 파일로 남긴다(디스크까지 쓴 뒤 기록에 남긴다).
    async fn freeze(&self, s: &Service) -> Result<(), String> {
        let session = s.session().ok_or("이 서비스의 기록을 되살릴 수 없습니다")?;
        // 저장하는 동안 입력을 막는다(참여자는 보기만). 관리자 연결의 쓰기가 끝나기를 기다린 뒤 저장할 내용을 뜬다(그 뒤의
        // 쓰기는 '서비스 중이 아님'으로 거절된다).
        let parts = {
            let _edits = s.edits.write().await;
            let _ = session.with(|s| s.set_closed(true)).await;
            session.save_parts().await
        }?;
        let data = self.save_data_of(s, parts, false);
        let path = s.result_path();
        tokio::task::spawn_blocking(move || write_synced(&path, &data.encode()))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        s.update(|r| r.result_file = true);
        tracing::info!(id = %s.id, "취합 데이터를 남겼습니다");
        Ok(())
    }

    fn save_data_of(&self, s: &Service, parts: SaveParts, live: bool) -> SaveData {
        let r = s.record();
        let file_name = r.output_path.file_name().map(|n| n.to_string_lossy().to_string());
        SaveData {
            id: s.id.clone(),
            title: r.manifest.title.clone(),
            workbook_name: r.manifest.workbook_name.clone(),
            file_name: file_name.unwrap_or_else(|| r.manifest.output.file_name.clone()),
            sheets: r.manifest.sheets.clone(),
            export: r.manifest.export,
            utc_offset_minutes: chrono::Local::now().offset().local_minus_utc() / 60,
            frozen_at_ms: self.now_ms(),
            live,
            parts,
        }
    }

    /// 저장 시도 하나가 실패했다: 남은 시도가 있으면 다시 해 볼 시각을 정하고, 없으면 취합 데이터를 남겼으면 '관리자 저장
    /// 대기', 못 남겼으면 '저장 실패'로 두고 공유를 닫는다(세션은 남겨 다시 저장할 수 있다).
    async fn attempt_failed(&self, s: &Service, why: Gone, error: String) {
        let retry_at = local_ms((self.opts.clock)()) + self.opts.retry_delay.as_millis() as i64;
        let mut wait_host = false;
        let r = s.update(|r| {
            r.attempts += 1;
            r.last_error = Some(error);
            r.excel_started_ms = None;
            if r.retries_left > 0 {
                r.retries_left -= 1;
                r.next_try_ms = Some(retry_at);
            } else {
                r.next_try_ms = None;
                if r.result_file {
                    wait_host = true;
                } else {
                    r.state = ServiceState::Failed;
                }
            }
        });
        if wait_host {
            self.await_host(s, why).await;
        } else if r.state == ServiceState::Failed {
            // 공유는 닫는다(마감이 지났다). 세션은 남겨 다시 저장할 수 있다.
            let progress = s.progress();
            if let Some(session) = s.session() {
                self.unpublish(&session, why);
            }
            s.update(|r| r.last = Some(progress));
        }
    }

    /// '관리자 저장 대기'로 둔다: 공유를 닫고 세션을 놓는다(취합 데이터 파일에 모두 들어 있다).
    async fn await_host(&self, s: &Service, why: Gone) {
        let progress = s.progress();
        self.close(s, why).await;
        s.update(|r| {
            r.state = ServiceState::AwaitingHost;
            r.next_try_ms = None;
            r.excel_started_ms = None;
            r.last = Some(progress);
        });
        s.drop_session();
        self.refresh_ip_names();
    }

    /// 관리자 PC(넘긴 호스트 앱)에서 취합 데이터를 저장했다: 데이터를 지우고 끝낸다. 같은 알림을 다시 받으면(응답을 놓친
    /// 호스트) 그대로 돌려준다.
    pub async fn host_saved(
        &self,
        id: &str,
        token: Option<&str>,
        saved: HostSaved,
    ) -> Result<ServiceStatus, AgentError> {
        let s = self.owned(id, token)?;
        let busy = || AgentError::conflict("에이전트 PC에서 저장하는 중입니다. 잠시 뒤 다시 해 주세요");
        let _g = s.saving.try_lock().map_err(|_| busy())?;
        let r = s.record();
        match r.state {
            ServiceState::Done if r.saved_on_host => return self.status_of(id, token),
            ServiceState::AwaitingHost | ServiceState::Failed if r.result_file => {}
            ServiceState::Saving => return Err(busy()),
            _ => return Err(AgentError::conflict("관리자 PC의 저장을 기다리는 서비스가 아닙니다")),
        }
        if saved.saved_path.trim().is_empty() {
            return Err(AgentError::bad("저장한 파일 경로가 없습니다"));
        }
        let why = if r.ended_by_host { Gone::Ended } else { Gone::Deadline };
        let progress = s.progress();
        self.close(&s, why).await;
        remove_result(&s);
        let path = saved.saved_path.clone();
        s.update(|r| {
            r.state = ServiceState::Done;
            r.result_file = false;
            r.saved_on_host = true;
            r.last_error = None;
            r.next_try_ms = None;
            r.saved_path = Some(saved.saved_path);
            r.saved_at_ms = Some(saved.saved_at_ms);
            r.finished_at_ms = Some(local_ms((self.opts.clock)()));
            r.last = Some(progress);
        });
        tracing::info!(id = %s.id, "관리자 PC에서 저장했습니다: {path}");
        self.after_finish(&s);
        self.status_of(id, token)
    }

    /// 취합 데이터([`SaveData`] 파일 그대로): 마감 때 남긴 것, 없고 서비스 중이면 지금 상태(서비스는 그대로 이어진다).
    pub async fn save_data(&self, id: &str, token: Option<&str>) -> Result<Vec<u8>, AgentError> {
        let s = self.owned(id, token)?;
        if s.record().result_file {
            return tokio::fs::read(s.result_path())
                .await
                .map_err(|e| AgentError::internal(format!("취합 데이터를 읽을 수 없습니다: {e}")));
        }
        let session = self.live_session(&s).map_err(|_| AgentError::conflict("받을 취합 데이터가 없습니다"))?;
        let parts = session.save_parts().await.map_err(AgentError::internal)?;
        let data = self.save_data_of(&s, parts, true);
        tokio::task::spawn_blocking(move || data.encode()).await.map_err(|e| AgentError::internal(e.to_string()))
    }

    /// 넘겨받은 원본 통합문서.
    pub async fn source(&self, id: &str, token: Option<&str>) -> Result<Vec<u8>, AgentError> {
        let s = self.owned(id, token)?;
        tokio::fs::read(s.source_path())
            .await
            .map_err(|e| AgentError::internal(format!("원본을 읽을 수 없습니다: {e}")))
    }

    /// 공유를 닫는다. 기록 파일에 '끝냄'을 남긴다.
    async fn close(&self, s: &Service, why: Gone) {
        if let Some(session) = s.session() {
            self.unpublish(&session, why);
            let _ = session.with(|x| x.set_meta(META_ENDED, "1")).await;
        }
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
    /// 저장하게 둔다. 취합 데이터를 남긴 서비스(관리자 저장 대기·저장 중)는 세션 없이 둔다. 오래된(14일) 끝난 서비스 폴더는
    /// 지운다.
    pub async fn restore(self: &Arc<Self>) {
        // 앞 판(서비스 중 수식 계산)이 남긴 계산 사본.
        let _ = std::fs::remove_dir_all(self.opts.data_dir.join("calc"));
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
            if record.result_file && matches!(record.state, ServiceState::Saving | ServiceState::AwaitingHost) {
                let s = Service::new(dir, record, None);
                if s.state() == ServiceState::Saving {
                    if s.record().excel_started_ms.is_some() {
                        tracing::warn!(id = %s.id, "엑셀로 저장하는 도중 에이전트가 꺼졌습니다");
                        let why = if s.record().ended_by_host { Gone::Ended } else { Gone::Deadline };
                        self.attempt_failed(&s, why, "엑셀로 저장하는 도중 에이전트가 꺼졌습니다".into()).await;
                    } else {
                        s.update(|r| r.next_try_ms = Some(now_ms));
                    }
                }
                self.services.lock().unwrap().insert(s.id.clone(), s);
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
                    service.update(|r| {
                        r.next_try_ms = Some(now_ms);
                        r.excel_started_ms = None;
                    });
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
        let recalc = formula_status(rec.init.sheets.iter().map(|(m, _)| m));
        let session = SessionHandle::spawn(rec.init);
        let passed = parse_deadline(&record.manifest.deadline).is_none_or(|d| self.now() >= d);
        let running = record.state == ServiceState::Running && !passed;
        let service = Service::new(dir.to_path_buf(), record, Some(session.clone()));
        service.set_recalc(recalc);
        self.attach(&service, &session);
        if running {
            self.hub.publish(&service.record().slug, session.clone())?;
            tracing::info!(id = %service.id, "서비스를 이어서 합니다");
        } else {
            let _ = session.with(|s| s.set_closed(true)).await;
        }
        Ok(service)
    }

    /// 에이전트를 끝낼 때: 공유를 닫고(참여자에게는 잠시 멈춘 것으로 — 다시 켜면 이어서 한다), 하고 있는 저장을 그만둔다
    /// (다시 켜면 다시 한다).
    pub async fn pause_all(&self) {
        self.stopping.send_replace(true);
        for (slug, _) in self.hub.sessions() {
            self.hub.unpublish_as(&slug, Gone::Paused);
        }
        self.opts.engine.shutdown().await;
    }
}

/// 남긴 취합 데이터 파일을 지운다(없으면 그만).
fn remove_result(s: &Service) {
    if let Err(e) = std::fs::remove_file(s.result_path())
        && e.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(id = %s.id, "취합 데이터를 지울 수 없습니다: {e}");
    }
}

/// 수식 칸이 있으면 '서비스 중에는 계산하지 않음'을 알린다(에이전트는 Excel을 열지 않는다 — 저장할 때 Excel이 계산한다).
fn formula_status<'a>(mut models: impl Iterator<Item = &'a collect_core::SheetModel>) -> RecalcStatus {
    if models.any(|m| m.cells.iter().any(|c| c.f.is_some())) {
        RecalcStatus::Unavailable {
            message: "에이전트 PC에서는 수식을 다시 계산하지 않습니다(저장할 때 Excel이 계산합니다)".into(),
        }
    } else {
        RecalcStatus::None
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
