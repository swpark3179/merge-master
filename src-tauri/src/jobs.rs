//! 여러 취합 업무를 함께 진행한다. 업무 하나 = 세션 하나(시트·설정·참여자·이력)와 그 업무의 1a 미리보기·계산 사본·
//! 저장 출처·서비스 상태. 업무에 딸린 command는 모두 `job`(업무 id)을 받는다.
//!
//! - 업무 id는 처음 만든 시각(`YYYYMMDD-HHMMSS-mmm`)이고 기록 파일 메타 `job`에 남는다(다시 확정해 새 파일이 생겨도 같다).
//! - 업무별 이벤트는 이름에 id를 붙인다(`participants:{id}`, `sheet-changes:{id}`, `comms:{id}`, `recalc-status:{id}`,
//!   `config-changed:{id}`). 사이드바 업무 목록은 전역 이벤트 `jobs`(바뀌었을 때만)로 받는다.
//! - 앱을 켜면 [`jobs_restore`]가 끝내지 않은 업무를 모두 되살린다(공개 중이던 업무는 다시 공개).

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use collect_core::SheetModel;
use collect_server::session::files::resume_plan;
use collect_server::{CommsSnapshot, HostSnapshot, META_ENDED, META_JOB, SessionHandle};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use ts_rs::TS;

use crate::service::ServiceInfo;
use crate::{AppState, CmdResult, Previews, recalc, recovery, run_excel, save};

/// 업무 목록 이벤트를 보내는 최소 간격(커서가 움직일 때마다 보내지 않도록).
const SUMMARY_EVERY: Duration = Duration::from_millis(250);
/// 새 업무의 주소 경로 기본값(겹치면 `q4-2`, `q4-3` …).
const DEFAULT_SLUG: &str = "q4";

pub(crate) const NO_JOB: &str = "없는 업무입니다(이미 끝냈을 수 있습니다)";

/// 업무 하나.
pub(crate) struct Job {
    pub id: String,
    /// 1a에서 읽었지만 아직 확정하지 않은 시트들(미리보기, 시트 이름별)과 그 통합문서.
    pub pending: tokio::sync::Mutex<Option<Previews>>,
    /// 확정한 시트·취합 설정·편집 내용을 가진 세션. 시트를 처음 확정할 때 만들고, 다시 확정하면 내용을 바꾼다.
    pub session: std::sync::Mutex<Option<SessionHandle>>,
    pub recalc: collect_excel::Recalc,
    /// 확정한 시트의 출처(저장할 원본).
    pub source: std::sync::Mutex<Option<save::Source>>,
    pub last_save: std::sync::Mutex<Option<save::LastSave>>,
    pub service: std::sync::Mutex<JobService>,
    /// 끝내는 중(진행 중인 확정이 세션을 새로 만들지 않게).
    pub ending: AtomicBool,
}

/// 업무의 참여자 공개 상태.
#[derive(Debug, Clone)]
pub(crate) struct JobService {
    /// 주소 경로(`http://IP:포트/{slug}`).
    pub slug: String,
    pub published: bool,
}

impl Job {
    pub(crate) fn new(id: String, slug: String) -> Self {
        Job {
            id,
            pending: Default::default(),
            session: Default::default(),
            recalc: collect_excel::Recalc::default(),
            source: Default::default(),
            last_save: Default::default(),
            service: std::sync::Mutex::new(JobService { slug, published: false }),
            ending: AtomicBool::new(false),
        }
    }

    pub(crate) fn session(&self) -> CmdResult<SessionHandle> {
        self.session.lock().unwrap().clone().ok_or_else(|| crate::NO_SHEET.into())
    }

    pub(crate) fn ending(&self) -> bool {
        self.ending.load(Ordering::SeqCst)
    }

    pub(crate) fn slug(&self) -> String {
        self.service.lock().unwrap().slug.clone()
    }

    pub(crate) fn published(&self) -> bool {
        self.service.lock().unwrap().published
    }
}

impl AppState {
    /// 이 PC의 업무. 에이전트 업무(서비스 id)면 '이 PC에서는 할 수 없어요'.
    pub(crate) fn job(&self, id: &str) -> CmdResult<Arc<Job>> {
        if let Some(job) = self.jobs.lock().unwrap().get(id).cloned() {
            return Ok(job);
        }
        Err(if self.remotes.get(id).is_some() { crate::remote::NOT_HERE } else { NO_JOB }.into())
    }

    pub(crate) fn all_jobs(&self) -> Vec<Arc<Job>> {
        self.jobs.lock().unwrap().values().cloned().collect()
    }

    /// 다른 업무가 쓰지 않는 주소 경로(대소문자 무시).
    pub(crate) fn slug_free(&self, slug: &str, except: Option<&str>) -> bool {
        self.jobs
            .lock()
            .unwrap()
            .values()
            .filter(|j| Some(j.id.as_str()) != except)
            .all(|j| !j.slug().eq_ignore_ascii_case(slug))
    }

    fn default_slug(&self) -> String {
        std::iter::once(DEFAULT_SLUG.to_string())
            .chain((2..).map(|n| format!("{DEFAULT_SLUG}-{n}")))
            .find(|s| self.slug_free(s, None))
            .expect("끝없는 후보")
    }

    /// 새 업무 id(만든 시각, 겹치면 숫자를 붙인다).
    fn new_job_id(&self) -> String {
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S-%3f").to_string();
        let jobs = self.jobs.lock().unwrap();
        std::iter::once(stamp.clone())
            .chain((2..).map(|n| format!("{stamp}-{n}")))
            .find(|id| !jobs.contains_key(id))
            .expect("끝없는 후보")
    }
}

/// 업무별 이벤트 이름.
pub(crate) fn event(name: &str, job: &str) -> String {
    format!("{name}:{job}")
}

/// 사이드바 업무 목록의 한 줄.
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct JobSummary {
    pub id: String,
    /// 업무명(취합 제목). 아직 시트를 확정하지 않았으면 빈 문자열.
    pub title: String,
    /// 원본 통합문서(전체 경로·이름)와 그 파일 이름.
    pub workbook_id: String,
    pub workbook: String,
    /// 불러온 시트(세션 안 순번 순서).
    pub sheets: Vec<String>,
    /// 시트를 확정해 세션이 있다.
    pub loaded: bool,
    pub slug: String,
    pub published: bool,
    pub online: u32,
    pub roster: u32,
    /// 승인 대기.
    pub pending: u32,
    pub required: u32,
    pub filled: u32,
    pub closed: bool,
    pub read_only: bool,
    #[ts(optional)]
    pub deadline: Option<String>,
    /// 참여자가 보낸 마지막 채팅 번호(안 읽은 채팅 표시, 없으면 0).
    pub last_chat: u32,
}

fn summary(job: &Job) -> JobSummary {
    let session = job.session.lock().unwrap().clone();
    let source = job.source.lock().unwrap().clone();
    let service = job.service.lock().unwrap().clone();
    let (workbook_id, sheets) = source.map(|s| (s.workbook_id, s.sheets)).unwrap_or_default();
    let workbook = Path::new(&workbook_id).file_name().map_or(workbook_id.clone(), |n| n.to_string_lossy().to_string());
    let mut out = JobSummary {
        id: job.id.clone(),
        title: String::new(),
        workbook_id,
        workbook,
        sheets,
        loaded: session.is_some(),
        slug: service.slug,
        published: service.published,
        online: 0,
        roster: 0,
        pending: 0,
        required: 0,
        filled: 0,
        closed: false,
        read_only: false,
        deadline: None,
        last_chat: 0,
    };
    if let Some(session) = session {
        let (snap, info) = (session.snapshot(), session.info());
        out.title = info.title;
        out.deadline = info.deadline;
        out.online = snap.online;
        out.roster = snap.roster;
        out.pending = snap.pending.len() as u32;
        out.required = snap.required;
        out.filled = snap.filled;
        out.closed = snap.closed;
        out.read_only = snap.read_only;
        out.last_chat = session.comms().chat.iter().filter(|m| !m.host).map(|m| m.id).max().unwrap_or(0);
    }
    out
}

fn summaries(state: &AppState) -> Vec<JobSummary> {
    state.all_jobs().iter().map(|j| summary(j)).collect()
}

/// 업무 목록이 바뀌었으면 웹뷰에 알린다(`jobs`).
pub(crate) fn emit_jobs(app: &AppHandle) {
    let state = app.state::<AppState>();
    let now = summaries(&state);
    let mut last = state.last_jobs.lock().unwrap();
    if *last != now {
        *last = now.clone();
        drop(last);
        let _ = app.emit("jobs", now);
    }
}

/// 세션의 접속자 표·공개 정보·소통이 바뀌면 업무 목록을 다시 보낸다. 세션이 끝나면 함께 끝난다.
fn forward_summary(app: &AppHandle, session: &SessionHandle) {
    let (mut snap, mut info, mut comms) = (session.subscribe(), session.subscribe_info(), session.subscribe_comms());
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            let alive = tokio::select! {
                r = snap.changed() => r.is_ok(),
                r = info.changed() => r.is_ok(),
                r = comms.changed() => r.is_ok(),
            };
            if !alive {
                break;
            }
            tokio::time::sleep(SUMMARY_EVERY).await;
            snap.mark_unchanged();
            info.mark_unchanged();
            comms.mark_unchanged();
            emit_jobs(&app);
        }
    });
}

/// 새로 만든(또는 되살린) 세션을 업무에 붙인다: IP 명단, 화면 이벤트, 기록 파일의 업무 id.
pub(crate) async fn attach(app: &AppHandle, state: &AppState, job: &Job, session: &SessionHandle) {
    crate::people::attach(state, session);
    crate::service::forward_participants(app, &job.id, session);
    crate::monitor::forward_events(app, &job.id, session);
    forward_summary(app, session);
    let id = job.id.clone();
    let _ = session.with(move |s| s.set_meta(META_JOB, &id)).await;
}

fn create(app: &AppHandle, state: &AppState) -> JobSummary {
    let job = Arc::new(Job::new(state.new_job_id(), state.default_slug()));
    let out = summary(&job);
    state.jobs.lock().unwrap().insert(job.id.clone(), job);
    emit_jobs(app);
    out
}

#[tauri::command]
pub fn jobs_list(state: State<'_, AppState>) -> Vec<JobSummary> {
    summaries(&state)
}

/// 새 취합 업무(시트는 1a에서 불러와 확정한다).
#[tauri::command]
pub fn job_create(app: AppHandle, state: State<'_, AppState>) -> JobSummary {
    create(&app, &state)
}

/// 업무를 고를 때 화면이 다시 그릴 것.
#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct JobState {
    pub id: String,
    /// 확정한 시트들(세션 안 순번 순서). 비어 있으면 아직 불러오지 않았다.
    pub models: Vec<SheetModel>,
    /// 주소 경로(공개하지 않았어도 정해 둔 값).
    pub slug: String,
    #[ts(optional)]
    pub service: Option<ServiceInfo>,
    #[ts(optional)]
    pub snapshot: Option<HostSnapshot>,
    #[ts(optional)]
    pub comms: Option<CommsSnapshot>,
    pub recalc: collect_server::RecalcStatus,
}

/// 업무를 고를 때 화면이 다시 그릴 것. 에이전트 업무면 관리자 연결로 받는다(공유 정보는 에이전트 상태에 있다).
#[tauri::command]
pub async fn job_state(state: State<'_, AppState>, job: String) -> CmdResult<serde_json::Value> {
    if !state.jobs.lock().unwrap().contains_key(&job)
        && let Some(remote) = state.remotes.get(&job)
    {
        let s = remote.state().await?;
        return Ok(serde_json::json!({
            "id": job,
            "models": s.models,
            "slug": s.status.slug,
            "snapshot": s.snapshot,
            "comms": s.comms,
            "recalc": s.recalc,
        }));
    }
    let job = state.job(&job)?;
    let session = job.session.lock().unwrap().clone();
    let models = match &session {
        Some(s) => s.with(|s| s.sheets.iter().map(|x| x.model.clone()).collect()).await?,
        None => Vec::new(),
    };
    let out = JobState {
        id: job.id.clone(),
        models,
        slug: job.slug(),
        service: crate::service::info_of(&state, &job).await,
        snapshot: session.as_ref().map(SessionHandle::snapshot),
        comms: session.as_ref().map(SessionHandle::comms),
        recalc: job.recalc.status(),
    };
    serde_json::to_value(out).map_err(|e| e.to_string())
}

/// 업무를 끝낸다: 공개를 멈추고(참여자에게 '끝났습니다'), 계산 사본을 닫고, 기록 파일에 '끝냄'을 남겨 앱을 다시 켜도
/// 되살리지 않는다(파일은 남는다). 남은 업무 목록을 돌려준다.
#[tauri::command]
pub async fn job_end(app: AppHandle, state: State<'_, AppState>, job: String) -> CmdResult<Vec<JobSummary>> {
    let job = state.job(&job)?;
    end_local(&app, &state, &job, Vec::new()).await?;
    Ok(summaries(&state))
}

/// 이 PC에서 업무를 끝낸다([`job_end`], 에이전트로 넘긴 뒤). `meta`는 기록 파일에 함께 남길 값(넘긴 곳 등).
pub(crate) async fn end_local(
    app: &AppHandle,
    state: &AppState,
    job: &Job,
    meta: Vec<(&'static str, String)>,
) -> CmdResult<()> {
    job.ending.store(true, Ordering::SeqCst);
    crate::service::unpublish(app, state, job, true).await?;
    let session = job.session.lock().unwrap().take();
    job.recalc.stop(&state.excel, &job.id);
    if let Some(session) = session {
        session.set_recalc(None);
        session
            .with(move |s| {
                for (k, v) in &meta {
                    s.set_meta(k, v);
                }
                s.set_meta(META_ENDED, "1");
            })
            .await?;
    }
    state.jobs.lock().unwrap().remove(&job.id);
    emit_jobs(app);
    Ok(())
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RestoredJob {
    pub id: String,
    pub title: String,
    /// 다시 공개했다.
    pub published: bool,
    #[ts(optional)]
    pub error: Option<String>,
}

/// 앱을 켤 때 되살린 업무.
#[derive(Debug, Clone, Default, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RestoreReport {
    pub jobs: Vec<RestoredJob>,
    /// 공개 중이던 업무를 다시 공개하지 못한 까닭(포트를 다른 프로그램이 씀 등). 취합은 이어서 할 수 있다.
    #[ts(optional)]
    pub service_error: Option<String>,
}

/// 앱을 켤 때 한 번: 끝내지 않은 업무를 모두 되살린다. 업무가 하나도 없으면 빈 업무를 하나 만든다.
#[tauri::command]
pub async fn jobs_restore(app: AppHandle, state: State<'_, AppState>) -> CmdResult<RestoreReport> {
    Ok(state.restored.get_or_init(|| restore_all(&app, &state)).await.clone())
}

async fn restore_all(app: &AppHandle, state: &AppState) -> RestoreReport {
    let mut report = RestoreReport::default();
    let files = recovery::sessions_dir(app).map(|d| recovery::store_files(&d)).unwrap_or_default();
    let metas: Vec<_> = files.iter().map(|(_, f)| f.clone()).collect();
    let mut started = Vec::new();
    let mut publish = Vec::new();
    for pick in resume_plan(&metas) {
        let path = &files[pick.index].0;
        let rec = match collect_server::recover(path) {
            Ok(rec) => rec,
            Err(e) => {
                tracing::warn!("기록 파일을 되살릴 수 없습니다({}): {e}", path.display());
                report.jobs.push(RestoredJob { id: pick.job, title: String::new(), published: false, error: Some(e) });
                continue;
            }
        };
        // 주소 경로는 남긴 값(다른 업무와 겹치면 새로).
        let remembered = rec.server.clone();
        let slug = remembered
            .as_ref()
            .map(|s| s.slug.clone())
            .filter(|s| collect_server::validate_slug(s).is_ok() && state.slug_free(s, None))
            .unwrap_or_else(|| state.default_slug());
        let job = Arc::new(Job::new(pick.job.clone(), slug));
        let session = SessionHandle::spawn(rec.init);
        *job.session.lock().unwrap() = Some(session.clone());
        *job.source.lock().unwrap() =
            Some(save::Source { workbook_id: rec.workbook_id.clone(), sheets: rec.sheets.clone() });
        state.jobs.lock().unwrap().insert(job.id.clone(), job.clone());
        attach(app, state, &job, &session).await; // 옛 파일에도 업무 id를 적어 둔다
        report.jobs.push(RestoredJob {
            id: job.id.clone(),
            title: session.info().title,
            published: false,
            error: None,
        });
        if let Some(meta) = remembered.filter(|s| s.running) {
            publish.push((job.clone(), meta.port));
        }
        started.push((job, session, rec.workbook_id, rec.sheets));
    }

    // 앞 앱이 남긴 숨은 Excel(계산 사본·원본)을 한 번에 정리한다. Excel이 없어도 취합은 이어서 한다.
    if !started.is_empty() {
        let sources: Vec<String> = started.iter().map(|(_, _, wb, _)| wb.clone()).collect();
        match run_excel(&state.excel, move |s| s.adopt_orphans_many(&sources)).await {
            Ok(r) if r.closed_calc > 0 || r.adopted => tracing::info!("앞 앱이 남긴 Excel 정리: {r:?}"),
            Ok(_) => {}
            Err(e) => tracing::warn!("Excel 정리 실패: {e}"),
        }
    }
    for (job, session, workbook_id, sheets) in &started {
        let source = collect_excel::CalcSource::Workbook(workbook_id.clone());
        let notify = recalc::notify(app, &job.id);
        job.recalc.start(state.excel.clone(), &job.id, session.clone(), source, sheets, notify);
    }

    // 공개 중이던 업무를 다시 공개한다. 포트는 모든 업무가 같이 쓰므로 가장 최근 업무의 포트로.
    if let Some(&(_, port)) = publish.first() {
        *state.port.lock().unwrap() = port;
        for (job, _) in &publish {
            match crate::service::publish(app, state, job, port, job.slug()).await {
                Ok(_) => {
                    if let Some(r) = report.jobs.iter_mut().find(|r| r.id == job.id) {
                        r.published = true;
                    }
                }
                Err(e) => {
                    tracing::warn!("업무 {}를 다시 공개하지 못했습니다: {e}", job.id);
                    report.service_error.get_or_insert(e);
                }
            }
        }
    }
    if state.jobs.lock().unwrap().is_empty() {
        create(app, state);
    }
    emit_jobs(app);
    report
}
