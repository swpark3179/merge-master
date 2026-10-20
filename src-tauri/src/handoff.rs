//! 업무를 에이전트 PC로 넘긴다(1c '에이전트에서 공유 시작'). 공유를 시작하기 전에만 넘긴다.
//!
//! 1. 넘기기 전 확인: 원본의 표 위치가 바뀌었는지(바뀌었으면 '그래도 넘기기'), 그사이 Excel에서도 고친 칸 수
//!    (마감 저장 때 취합 값으로 쓴다).
//! 2. 마감(필수)과 '마감 시각이 되면 읽기 전용'을 설정에 쓰고, 원본의 지금 상태(`SaveCopyAs`, 저장하지 않은 Excel 변경도
//!    든다)와 기록 파일 사본(`VACUUM INTO`)을 떠서 한 묶음으로 보낸다.
//! 3. 에이전트가 받으면 이 PC의 업무를 끝내고(기록 파일은 남는다) '에이전트에서 진행 중'에 둔다. 응답을 놓쳤으면 에이전트에
//!    물어 받았는지 확인한다([`crate::agent`]).

use std::collections::BTreeMap;
use std::path::Path;

use agent_protocol::client::{AgentClient, ClientError};
use agent_protocol::{
    HOST_LEAD_MINUTES, HandoffManifest, OutputTarget, Package, StatusQuery, check_deadline, check_name, extension,
};
use collect_core::config::DeadlineAction;
use collect_core::edit::ConfigEdit;
use collect_core::export::ExportOptions;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};
use ts_rs::TS;

use crate::agent::{AgentRef, AgentServiceView};
use crate::jobs::Job;
use crate::save::{AfterSave, SaveOptions, SaveTarget};
use crate::{AppState, CmdResult, run_excel};

#[derive(Debug, Clone, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HandoffOptions {
    /// 에이전트 주소(연결 확인한 것).
    pub addr: String,
    /// 마감(`YYYY-MM-DDTHH:MM`, 에이전트 PC 시각).
    pub deadline: String,
    /// 마감 때 저장할 폴더(에이전트 기본 저장 폴더 아래 경로 조각)와 파일 이름.
    pub dir: Vec<String>,
    pub file_name: String,
    pub export: ExportOptions,
    /// 바라는 주소 경로(겹치면 에이전트가 바꾼다).
    pub slug: String,
    /// 원본의 표 위치가 바뀐 것 같아도 넘긴다.
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HandoffCheck {
    /// 원본 파일 이름(확장자 포함)과 그 이름·확장자(저장 파일 이름 기본값: `이름_취합_마감일.확장자`).
    pub workbook: String,
    pub stem: String,
    pub ext: String,
    /// 그사이 Excel에서도 고친 칸(마감 저장 때 취합 값으로 쓴다).
    pub conflicts: u32,
    /// 원본의 표 위치가 바뀐 것 같다(행·열을 넣거나 지움).
    pub suspicious: bool,
    /// 지금 바뀐 칸 수.
    pub writes: u32,
}

/// 원본 파일 이름(새 문서면 `.xlsx`를 붙인다)과 확장자.
fn workbook_name(name: &str) -> (String, String) {
    let ext = extension(name);
    if ["xlsx", "xlsm", "xlsb", "xls"].contains(&ext.as_str()) {
        (name.to_string(), ext)
    } else {
        (format!("{name}.xlsx"), "xlsx".into())
    }
}

fn not_shared(job: &Job) -> CmdResult<()> {
    if job.published() {
        return Err("이 PC에서 공유 중에는 넘길 수 없어요. 공유를 멈춘 뒤 넘겨 주세요".into());
    }
    Ok(())
}

fn check_options() -> SaveOptions {
    SaveOptions {
        target: SaveTarget::Original,
        export: ExportOptions { mark: None, styles: true, history: true, cancelled: true },
        after: AfterSave::Keep,
    }
}

/// 넘기기 전 확인(원본을 다시 읽어 비교한다 — 원본이 닫혔으면 다시 연다).
#[tauri::command]
pub async fn agent_handoff_check(state: State<'_, AppState>, job: String) -> CmdResult<HandoffCheck> {
    let job = state.job(&job)?;
    not_shared(&job)?;
    let (_, check, _) = crate::save::check(&state, &job, &check_options()).await?;
    let (workbook, ext) = workbook_name(&check.source.name);
    let stem = Path::new(&workbook).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    Ok(HandoffCheck {
        workbook,
        stem,
        ext,
        conflicts: check.sheets.iter().map(|s| s.report.conflicts.len() as u32).sum(),
        suspicious: check.suspicious,
        writes: check.writes,
    })
}

/// 넘긴다. 에이전트가 받으면 이 PC의 업무를 끝내고 '에이전트에서 진행 중' 한 줄을 돌려준다.
#[tauri::command]
pub async fn agent_handoff(
    app: AppHandle,
    state: State<'_, AppState>,
    job: String,
    options: HandoffOptions,
) -> CmdResult<AgentServiceView> {
    let job = state.job(&job)?;
    not_shared(&job)?;
    let session = job.session()?;
    let src = crate::save::source(&job)?;
    let client = AgentClient::new(&options.addr)?;
    let info = client.info().await?;
    if !info.excel {
        return Err("에이전트 PC에 Excel이 없어 마감 때 저장할 수 없습니다".into());
    }
    // 마감은 에이전트 PC 시각으로 잰다.
    let agent_now = chrono::DateTime::from_timestamp_millis(info.now_ms + i64::from(info.utc_offset_minutes) * 60_000)
        .map(|t| t.naive_utc())
        .ok_or("에이전트 시각을 읽을 수 없습니다")?;
    check_deadline(&options.deadline, agent_now, HOST_LEAD_MINUTES, info.max_days)?;
    options.dir.iter().try_for_each(|d| check_name(d))?;
    check_name(&options.file_name)?;

    let (_, check, _) = crate::save::check(&state, &job, &check_options()).await?;
    if check.suspicious && !options.force {
        return Err(
            "원본의 표 위치가 바뀐 것 같습니다(행·열을 넣거나 지움). 확인한 뒤 '그래도 넘기기'를 골라 주세요".into()
        );
    }
    let (workbook, ext) = workbook_name(&check.source.name);
    if extension(&options.file_name) != ext {
        return Err(format!("저장할 파일은 원본과 같은 형식(.{ext})이어야 합니다"));
    }

    // 마감과 '마감 시각이 되면 읽기 전용'을 설정에 쓴다(넘기는 기록 파일에 든다).
    let deadline = options.deadline.clone();
    let (title, host_name) = session
        .with(move |s| -> Result<(String, String), String> {
            let c = s.sheets[0].config.clone();
            let title = c.title.clone();
            s.apply(
                0,
                ConfigEdit::SetMeta {
                    title: c.title,
                    description: c.description,
                    requester: c.requester,
                    deadline: Some(deadline),
                },
            )?;
            let mut permissions = s.sheets[0].config.permissions.clone();
            permissions.deadline_action = DeadlineAction::ReadOnly;
            s.apply(0, ConfigEdit::SetPermissions { permissions })?;
            Ok((title, s.host_name().to_string()))
        })
        .await??;
    let title = if title.trim().is_empty() {
        Path::new(&workbook).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()
    } else {
        title
    };

    let id = uuid::Uuid::new_v4().to_string();
    let token = uuid::Uuid::new_v4().simple().to_string();
    let _busy = state.agents.busy(&id);
    let dir = app.path().app_local_data_dir().map_err(|e| e.to_string())?.join("handoff").join(&id);
    std::fs::create_dir_all(&dir).map_err(|e| format!("임시 폴더를 만들 수 없습니다: {e}"))?;
    let cleanup = Cleanup(dir.clone());
    let source_copy = dir.join(format!("source.{ext}"));
    let (wb, path) = (src.workbook_id.clone(), source_copy.clone());
    run_excel(&state.excel, move |s| s.save_copy_as(&wb, &path)).await?;
    let store_copy = dir.join("session.sqlite");
    session.backup_store(store_copy.clone()).await?;
    let read = |p: &Path| std::fs::read(p).map_err(|e| format!("{}을(를) 읽을 수 없습니다: {e}", p.display()));
    let ip_names: BTreeMap<String, String> =
        state.ip_names.lock().unwrap().entries().into_iter().map(|e| (e.ip, e.name)).collect();
    let manifest = HandoffManifest {
        id: id.clone(),
        token: token.clone(),
        job_id: job.id.clone(),
        host_pc: std::env::var("COMPUTERNAME").unwrap_or_default(),
        host_version: env!("CARGO_PKG_VERSION").to_string(),
        host_name: Some(host_name),
        title: title.clone(),
        slug: options.slug.trim().to_string(),
        deadline: options.deadline.clone(),
        output: OutputTarget { dir: options.dir.clone(), file_name: options.file_name.clone() },
        export: options.export,
        workbook_name: workbook.clone(),
        sheets: src.sheets.clone(),
        ip_names,
    };
    let pkg = Package { manifest, store: read(&store_copy)?, workbook: read(&source_copy)? };
    drop(cleanup);

    let addr = client.base_url();
    state.agents.add(AgentRef {
        addr: addr.clone(),
        id: id.clone(),
        token: token.clone(),
        job_id: job.id.clone(),
        title,
        workbook,
        agent_name: info.name.clone(),
        created_at_ms: chrono::Utc::now().timestamp_millis(),
        pending: true,
        local_ended: false,
    });
    let received = match client.create(&pkg).await {
        Ok(_) => true,
        Err(ClientError::Api { message, .. }) => {
            state.agents.remove(&id);
            return Err(message);
        }
        // 응답을 놓쳤다: 받았는지 물어본다.
        Err(ClientError::Unreachable(e)) => {
            let q = [StatusQuery { id: id.clone(), token: token.clone() }];
            match client.status(&q).await {
                Ok(items) if items.first().is_some_and(|i| i.status.is_some()) => true,
                Ok(_) => {
                    state.agents.remove(&id);
                    return Err(format!("에이전트에 넘기지 못했습니다: {e}"));
                }
                Err(_) => {
                    return Err(format!(
                        "에이전트의 응답을 받지 못했습니다({e}). 넘겨졌는지 잠시 뒤 '에이전트에서 진행 중'에서 확인해 주세요"
                    ));
                }
            }
        }
    };
    debug_assert!(received);
    state.agents.set(&id, |r| r.pending = false);
    let r = state.agents.find(&id)?;
    crate::agent::end_local_job(&app, &state, &r).await;
    if let Ok(items) = client.status(&[StatusQuery { id: id.clone(), token }]).await
        && let Some(status) = items.into_iter().next().and_then(|i| i.status)
    {
        state.agents.record_status(&id, status);
    }
    // 관리자 연결을 바로 연다(넘긴 업무를 이어서 이 PC 업무처럼 다룬다).
    crate::remote::sync(&app);
    crate::agent::emit(&app);
    tracing::info!(id, addr, "업무를 에이전트로 넘겼습니다");
    state.agents.view(&id, &state.remotes).ok_or_else(|| "넘긴 서비스를 찾을 수 없습니다".into())
}

/// 임시 폴더를 지운다(넘기기가 어떻게 끝나든).
struct Cleanup(std::path::PathBuf);

impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workbook_names_keep_or_add_the_extension() {
        assert_eq!(workbook_name("budget.xlsm"), ("budget.xlsm".into(), "xlsm".into()));
        assert_eq!(workbook_name("통합 문서1"), ("통합 문서1.xlsx".into(), "xlsx".into()));
    }
}
