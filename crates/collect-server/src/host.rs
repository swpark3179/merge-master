//! 호스트(취합 관리자)가 업무에 하는 일. 호스트 앱의 업무 command(이 PC의 세션)와 에이전트의 관리자 연결(다른 PC의
//! 세션 — 호스트 앱이 원격으로 보낸다)이 같이 쓴다: 요청([`HostRequest`]) 하나를 세션에서 하고 결과를 JSON으로
//! 돌려준다([`execute`]).
//!
//! 여기는 세션만 다룬다. 앱 화면 이벤트·파일 쓰기(CSV는 글자로 돌려준다)·에이전트의 마감 규칙은 부르는 쪽이 한다.

use std::collections::HashSet;
use std::time::Duration;

use collect_core::a1::parse_sheet_range;
use collect_core::edit::ConfigEdit;
use collect_core::format::{FormatEdit, apply_format};
use collect_core::history::{
    self, BatchOrigin, ChangeEntry, ChangeKind, HistoryFilter, HistoryPage, RevertPlan, SheetLabels,
};
use collect_core::structure::StructureEdit;
use collect_core::view::ConfigView;
use collect_core::{CellRange, CellRef, CellValue, SheetCell, SheetModel};
use collect_store::SnapshotMeta;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::protocol::{ChatReaction, ChatTo};
use crate::{HighlightAudience, NoticeTarget, SessionHandle, Shared};

/// '최근 삭제된 셀' 필터가 보는 시간.
const RECENT_DELETE: Duration = Duration::from_secs(60 * 60);
/// 비교 보기에 보이는 최대 칸 수.
const DIFF_LIMIT: usize = 500;

/// 호스트가 칸에 넣을 값(모델 좌표). TS: `apps/host/src/api.ts`의 `CellWrite`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CellWrite {
    pub row: u32,
    pub col: u32,
    #[serde(default)]
    pub value: CellValue,
}

/// 호스트 요청 하나(업무 command 하나). 좌표는 그 시트의 모델 좌표.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "req", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum HostRequest {
    /// 1b 설정 보기. `viewer`면 그 참여자 시점.
    ConfigView {
        sheet: usize,
        viewer: Option<String>,
    },
    ConfigApply {
        sheet: usize,
        edit: ConfigEdit,
        viewer: Option<String>,
    },
    SheetFormat {
        sheet: usize,
        edit: FormatEdit,
        viewer: Option<String>,
    },
    SheetStructure {
        sheet: usize,
        edit: StructureEdit,
        viewer: Option<String>,
    },
    SheetSetCells {
        sheet: usize,
        cells: Vec<CellWrite>,
    },
    MonitorGet {
        sheet: usize,
    },
    MonitorProgress {
        sheet: usize,
    },
    MonitorActivity {
        sheet: usize,
        limit: Option<u32>,
    },
    CommsGet,
    NoticeSend {
        text: String,
        important: bool,
        target: NoticeTarget,
    },
    HighlightAdd {
        sheet: usize,
        range: CellRange,
        note: String,
        audience: HighlightAudience,
    },
    HighlightClear {
        id: u32,
    },
    ChatSend {
        to: ChatTo,
        text: String,
    },
    ChatReact {
        id: u32,
        kind: ChatReaction,
    },
    HistoryQuery {
        filter: HistoryFilter,
        offset: u32,
        limit: u32,
    },
    HistoryTimeline {
        sheet: u32,
        at: CellRef,
    },
    HistoryRevert {
        seqs: Vec<u64>,
        force: bool,
    },
    HistoryRevertBatch {
        batch: String,
        force: bool,
    },
    HistorySetValue {
        sheet: u32,
        at: CellRef,
        value: CellValue,
    },
    /// CSV 글자(파일은 부르는 쪽이 쓴다). `offset_minutes`는 호스트 PC 시간대(UTC와의 차이).
    HistoryCsv {
        filter: HistoryFilter,
        offset_minutes: i32,
    },
    SnapshotList,
    SnapshotTake {
        label: String,
    },
    SnapshotDiff {
        id: u32,
    },
    SnapshotRestore {
        id: u32,
    },
    ParticipantsGet,
    /// `range`는 `sheet` 시트의 주소(`H500:H620`, `인력계획!E5:E9`처럼 시트 이름을 붙여도 된다).
    ParticipantApprove {
        name: String,
        sheet: u32,
        range: Option<String>,
        note: String,
    },
    ParticipantReject {
        name: String,
    },
    ParticipantKick {
        name: String,
    },
    ParticipantUnlock {
        name: String,
    },
    /// 취합 마감(참여자 읽기 전용) / 다시 열기.
    CollectClose {
        closed: bool,
    },
    /// 확정한 시트들(세션 안 순번 순서).
    Models,
}

impl HostRequest {
    /// 세션을 바꾸지 않는 요청(보기만 한다).
    pub fn is_read(&self) -> bool {
        matches!(
            self,
            HostRequest::ConfigView { .. }
                | HostRequest::MonitorGet { .. }
                | HostRequest::MonitorProgress { .. }
                | HostRequest::MonitorActivity { .. }
                | HostRequest::CommsGet
                | HostRequest::HistoryQuery { .. }
                | HostRequest::HistoryTimeline { .. }
                | HostRequest::HistoryCsv { .. }
                | HostRequest::SnapshotList
                | HostRequest::SnapshotDiff { .. }
                | HostRequest::ParticipantsGet
                | HostRequest::Models
        )
    }
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct FormatResult {
    pub model: SheetModel,
    pub view: ConfigView,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct MonitorData {
    /// 지금 시트(참여자 입력이 반영된 값).
    pub model: SheetModel,
    /// 호스트 시점 설정 보기(셀 상태·명단·행 담당자·진행률·열 이름).
    pub view: ConfigView,
}

#[derive(Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Activity {
    /// 모든 시트의 최근 묶음(묶음마다 시트).
    pub page: HistoryPage,
    /// 그 시트에서 최근 1시간 안에 지워져 지금도 빈 칸이 있는 행('최근 삭제된 셀' 필터).
    pub deleted_rows: Vec<u32>,
    /// 그 시트에서 행 담당자가 없는 행 중 사람마다(명단 인덱스) 그 사람이 넣은 값이 남아 있는 행('담당자별' 필터).
    pub open_rows_by: Vec<Vec<u32>>,
}

#[derive(Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Timeline {
    pub sheet: u32,
    pub at: CellRef,
    /// 불러온 원본 값.
    #[ts(as = "Option<CellValue>", optional)]
    pub original: CellValue,
    /// 지금 값.
    #[ts(as = "Option<CellValue>", optional)]
    pub current: CellValue,
    /// 오래된 것부터.
    pub entries: Vec<ChangeEntry>,
}

#[derive(Debug, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RevertResult {
    /// 되돌린 칸 수. `stale`이 있고 `force`가 아니면 0(아무것도 하지 않음).
    pub changed: u32,
    /// 기록 뒤에 다시 바뀐 칸(되돌리면 그 변경도 덮어쓴다).
    pub stale: Vec<SheetCell>,
}

#[derive(Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SnapshotView {
    pub meta: SnapshotMeta,
    /// 지금과 다른 칸 수(모든 시트).
    pub diff: u32,
    /// 이 스냅샷 뒤에 바꾼 사람.
    pub users_after: Vec<String>,
}

#[derive(Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DiffCell {
    pub sheet: u32,
    pub at: CellRef,
    #[ts(as = "Option<CellValue>", optional)]
    pub snapshot: CellValue,
    #[ts(as = "Option<CellValue>", optional)]
    pub current: CellValue,
}

#[derive(Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SnapshotDiff {
    pub total: u32,
    /// 앞 몇 칸(시트 순서, 행 우선).
    pub cells: Vec<DiffCell>,
    /// 시트마다 이름·위치·열 이름(주소·항목 표시용).
    pub sheets: Vec<DiffSheet>,
}

#[derive(Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DiffSheet {
    pub name: String,
    pub origin: CellRef,
    pub labels: Vec<String>,
}

/// CSV로 내보낼 글자와 줄 수([`HostRequest::HistoryCsv`]).
#[derive(Debug, Serialize, Deserialize)]
pub struct CsvExport {
    pub csv: String,
    pub count: u32,
}

fn json<T: Serialize>(v: T) -> Result<serde_json::Value, String> {
    serde_json::to_value(v).map_err(|e| e.to_string())
}

/// 미리보던 사람이 명단에서 빠졌으면(범위 배정을 지움 등) 호스트 시점으로 돌아간다. 작업 안에서 바꾼 설정을 바로 본다.
fn view_or_host(s: &Shared<'_>, sheet: usize, viewer: Option<&str>) -> Result<ConfigView, String> {
    s.view(sheet, viewer).or_else(|_| s.view(sheet, None))
}

/// 스냅샷과 지금 시트들이 다른 칸(시트 순번, 칸).
fn diff_all(values: &[Vec<CellValue>], s: &Shared<'_>) -> Vec<SheetCell> {
    values
        .iter()
        .zip(s.sheets.iter())
        .enumerate()
        .flat_map(|(i, (v, slot))| {
            history::diff(v, &slot.model).into_iter().map(move |at| SheetCell::new(i as u32, at))
        })
        .collect()
}

enum Pick {
    Seqs(HashSet<u64>),
    Batch(String),
}

async fn revert(session: &SessionHandle, pick: Pick, force: bool) -> Result<RevertResult, String> {
    session
        .with(move |s| {
            let entries: Vec<&ChangeEntry> = match &pick {
                Pick::Seqs(seqs) => s.history().iter().filter(|e| seqs.contains(&e.seq)).collect(),
                Pick::Batch(batch) => history::batch_entries(s.history(), batch),
            };
            if entries.is_empty() {
                return Err("기록을 찾을 수 없습니다".to_string());
            }
            // 시트마다 되돌릴 값(한 묶음은 한 시트지만, 칸 여럿을 고르면 여러 시트일 수 있다).
            let mut plans: Vec<(usize, RevertPlan)> = Vec::new();
            for (i, slot) in s.sheets.iter().enumerate() {
                let mine: Vec<&ChangeEntry> = entries.iter().copied().filter(|e| e.sheet as usize == i).collect();
                if !mine.is_empty() {
                    plans.push((i, history::revert_plan(&slot.model, &mine)));
                }
            }
            let stale: Vec<SheetCell> =
                plans.iter().flat_map(|(i, p)| p.stale.iter().map(|at| SheetCell::new(*i as u32, *at))).collect();
            if !stale.is_empty() && !force {
                return Ok(RevertResult { changed: 0, stale });
            }
            let user = s.host_name().to_string();
            let mut n = 0;
            for (i, RevertPlan { changes, .. }) in plans {
                n += changes.len() as u32;
                s.host_set(&user, BatchOrigin::Revert, i, changes);
            }
            Ok(RevertResult { changed: n, stale })
        })
        .await?
}

/// 요청 하나를 그 세션에서 하고 결과를 JSON으로 돌려준다(돌려줄 것이 없으면 `null`).
pub async fn execute(session: &SessionHandle, req: HostRequest) -> Result<serde_json::Value, String> {
    match req {
        HostRequest::ConfigView { sheet, viewer } => {
            json(session.with(move |s| s.view(sheet, viewer.as_deref())).await??)
        }
        HostRequest::ConfigApply { sheet, edit, viewer } => json(
            session
                .with(move |s| {
                    s.apply(sheet, edit)?;
                    view_or_host(s, sheet, viewer.as_deref())
                })
                .await??,
        ),
        // 서식 도구 줄·크기 조절. 병합이 바뀌면 입력 가능 칸도 바뀌므로 설정 보기를 함께 돌려준다. 열 너비·행 높이는
        // 권한이 그대로라 참여자에게 새 시트를 다시 보내지 않고 바뀐 크기만 알린다.
        HostRequest::SheetFormat { sheet, edit, viewer } => json(
            session
                .with(move |s| {
                    if let Some((axis, from, count, size)) = edit.as_resize() {
                        s.resize(sheet, axis, from, count, size)?;
                    } else {
                        let slot = s.sheets.get_mut(sheet).ok_or("없는 시트입니다")?;
                        apply_format(&mut slot.model, edit).map_err(|e| e.to_string())?;
                        s.changed();
                    }
                    let model = s.sheets[sheet].model.clone();
                    let view = view_or_host(s, sheet, viewer.as_deref())?;
                    Ok::<_, String>(FormatResult { model, view })
                })
                .await??,
        ),
        // 표 구조 바꾸기는 작업이 끝난 뒤 반영되므로 다시 읽는다.
        HostRequest::SheetStructure { sheet, edit, viewer } => {
            session.with(move |s| s.structure(sheet, edit)).await??;
            json(
                session
                    .with(move |s| {
                        let model = s.sheets.get(sheet).ok_or("없는 시트입니다")?.model.clone();
                        let view = view_or_host(s, sheet, viewer.as_deref())?;
                        Ok::<_, String>(FormatResult { model, view })
                    })
                    .await??,
            )
        }
        // 머리글(열 이름)이나 행 배정 기준 열(명단)을 바꿨으면 권한을 다시 계산한다 — 같은 작업 안에서는 다시 계산이 값
        // 반영보다 먼저 돌아서 따로 보낸다.
        HostRequest::SheetSetCells { sheet, cells } => {
            let structural = session
                .with(move |s| {
                    let slot = s.sheets.get(sheet).ok_or("없는 시트입니다")?;
                    let header_rows = slot.model.header_rows;
                    let key = slot.config.assignment.key_col;
                    let structural = cells.iter().any(|c| c.row < header_rows || Some(c.col) == key);
                    let user = s.host_name().to_string();
                    let cells = cells.into_iter().map(|c| (CellRef::new(c.row, c.col), c.value)).collect();
                    s.host_set(&user, BatchOrigin::Edit, sheet, cells);
                    Ok::<_, String>(structural)
                })
                .await??;
            if structural {
                session.with(|s| s.changed()).await?;
            }
            Ok(serde_json::Value::Null)
        }
        HostRequest::MonitorGet { sheet } => json(
            session
                .with(move |s| {
                    let view = s.view(sheet, None)?;
                    Ok::<_, String>(MonitorData { model: s.sheets[sheet].model.clone(), view })
                })
                .await??,
        ),
        HostRequest::MonitorProgress { sheet } => json(
            session.with(move |s| s.progress(sheet).cloned().ok_or_else(|| "없는 시트입니다".to_string())).await??,
        ),
        HostRequest::MonitorActivity { sheet, limit } => {
            let limit = limit.unwrap_or(30) as usize;
            let since = chrono::Utc::now().timestamp_millis() - RECENT_DELETE.as_millis() as i64;
            json(
                session
                    .with(move |s| {
                        let page = history::query(s.history(), &HistoryFilter::default(), 0, limit);
                        let mut rows: Vec<u32> = s
                            .history()
                            .iter()
                            .filter(|e| {
                                e.kind == ChangeKind::Delete
                                    && e.sheet as usize == sheet
                                    && e.at_ms >= since
                                    && s.sheets
                                        .get(sheet)
                                        .and_then(|x| x.model.cell(e.at()))
                                        .is_some_and(|c| c.v.is_empty())
                            })
                            .map(|e| e.row)
                            .collect();
                        rows.sort_unstable();
                        rows.dedup();
                        Activity { page, deleted_rows: rows, open_rows_by: s.open_rows_by(sheet) }
                    })
                    .await?,
            )
        }
        HostRequest::CommsGet => json(session.comms()),
        HostRequest::NoticeSend { text, important, target } => {
            if text.trim().is_empty() {
                return Err("공지 내용을 적어 주세요".into());
            }
            session.with(move |s| s.send_notice(&text, important, target)).await?;
            Ok(serde_json::Value::Null)
        }
        HostRequest::HighlightAdd { sheet, range, note, audience } => {
            session.with(move |s| s.highlight(sheet, range, &note, audience)).await?;
            Ok(serde_json::Value::Null)
        }
        HostRequest::HighlightClear { id } => {
            session.with(move |s| s.clear_highlight(id)).await?;
            Ok(serde_json::Value::Null)
        }
        HostRequest::ChatSend { to, text } => {
            if !text.trim().is_empty() {
                session.with(move |s| s.chat(to, &text)).await?;
            }
            Ok(serde_json::Value::Null)
        }
        // 참여자 채팅에 리액션을 달거나 뗀다(같은 종류를 다시 누르면 뗀다).
        HostRequest::ChatReact { id, kind } => {
            session.with(move |s| s.chat_react(id, kind)).await?;
            Ok(serde_json::Value::Null)
        }
        HostRequest::HistoryQuery { filter, offset, limit } => {
            json(session.with(move |s| history::query(s.history(), &filter, offset as usize, limit as usize)).await?)
        }
        HostRequest::HistoryTimeline { sheet, at } => json(
            session
                .with(move |s| {
                    let slot = s.sheets.get(sheet as usize).ok_or("없는 시트입니다")?;
                    let cols = slot.model.cols.max(1);
                    let current = slot.model.cell(at).map(|c| c.v.clone()).unwrap_or_default();
                    let original = s
                        .snapshots()
                        .first()
                        .and_then(|m| s.snapshot_values(m.id))
                        .and_then(|v| v.get(sheet as usize)?.get((at.row * cols + at.col) as usize).cloned())
                        .unwrap_or_default();
                    Ok::<_, String>(Timeline {
                        sheet,
                        at,
                        original,
                        current,
                        entries: history::timeline(s.history(), sheet, at),
                    })
                })
                .await??,
        ),
        HostRequest::HistoryRevert { seqs, force } => {
            json(revert(session, Pick::Seqs(seqs.into_iter().collect()), force).await?)
        }
        HostRequest::HistoryRevertBatch { batch, force } => json(revert(session, Pick::Batch(batch), force).await?),
        // 셀 타임라인의 '이 값으로'.
        HostRequest::HistorySetValue { sheet, at, value } => {
            session
                .with(move |s| {
                    let user = s.host_name().to_string();
                    s.host_set(&user, BatchOrigin::Revert, sheet as usize, vec![(at, value)]);
                })
                .await?;
            Ok(serde_json::Value::Null)
        }
        // 필터에 걸린 기록 전부. 시트가 둘 이상이면 '시트' 열이 있다.
        HostRequest::HistoryCsv { filter, offset_minutes } => json(
            session
                .with(move |s| {
                    let entries: Vec<ChangeEntry> = s.history().iter().filter(|e| filter.matches(e)).cloned().collect();
                    let sheets: Vec<SheetLabels> = s.sheets.iter().map(|x| SheetLabels::of(&x.model)).collect();
                    CsvExport { csv: history::to_csv(&entries, &sheets, offset_minutes), count: entries.len() as u32 }
                })
                .await?,
        ),
        HostRequest::SnapshotList => json(
            session
                .with(|s| {
                    let mut out: Vec<SnapshotView> = s
                        .snapshots()
                        .into_iter()
                        .map(|meta| {
                            let diff = s.snapshot_values(meta.id).map_or(0, |v| diff_all(&v, s).len() as u32);
                            let users_after = history::users_after(s.history(), meta.seq);
                            SnapshotView { meta, diff, users_after }
                        })
                        .collect();
                    out.reverse(); // 최근 것부터
                    out
                })
                .await?,
        ),
        HostRequest::SnapshotTake { label } => {
            session.with(move |s| s.take_snapshot(&label)).await?;
            Ok(serde_json::Value::Null)
        }
        HostRequest::SnapshotDiff { id } => json(
            session
                .with(move |s| {
                    let values = s.snapshot_values(id).ok_or("스냅샷을 찾을 수 없습니다")?;
                    let diff = diff_all(&values, s);
                    let cell = |c: &SheetCell| {
                        let slot = &s.sheets[c.sheet as usize];
                        let cols = slot.model.cols.max(1);
                        DiffCell {
                            sheet: c.sheet,
                            at: c.at(),
                            snapshot: values[c.sheet as usize][(c.row * cols + c.col) as usize].clone(),
                            current: slot.model.cell(c.at()).map(|x| x.v.clone()).unwrap_or_default(),
                        }
                    };
                    Ok::<_, String>(SnapshotDiff {
                        total: diff.len() as u32,
                        cells: diff.iter().take(DIFF_LIMIT).map(cell).collect(),
                        sheets: s
                            .sheets
                            .iter()
                            .map(|x| {
                                let l = SheetLabels::of(&x.model);
                                DiffSheet { name: l.name, origin: l.origin, labels: l.labels }
                            })
                            .collect(),
                    })
                })
                .await??,
        ),
        // 스냅샷 시점으로 복원한다(복원 직전 상태는 자동으로 스냅샷에 남는다). 바꾼 칸 수.
        HostRequest::SnapshotRestore { id } => json(
            session
                .with(move |s| {
                    let user = s.host_name().to_string();
                    s.restore(id, &user).map(|n| n as u32)
                })
                .await??,
        ),
        HostRequest::ParticipantsGet => json(session.snapshot()),
        HostRequest::ParticipantApprove { name, sheet, range, note } => {
            let range = match range.as_deref().map(str::trim).filter(|r| !r.is_empty()) {
                Some(text) => {
                    let sheets: Vec<(String, CellRef)> = session
                        .with(|s| s.sheets.iter().map(|x| (x.model.sheet.clone(), x.model.origin)).collect())
                        .await?;
                    let names: Vec<&str> = sheets.iter().map(|(n, _)| n.as_str()).collect();
                    let (i, range) = parse_sheet_range(text, &names, sheet as usize).map_err(|e| e.to_string())?;
                    let origin = sheets.get(i).map(|(_, o)| *o).ok_or("없는 시트입니다")?;
                    let range = range.relative_to(origin).ok_or_else(|| format!("{text}은(는) 불러온 표 밖입니다"))?;
                    Some((i as u32, range))
                }
                None => None,
            };
            session.approve(&name, range, &note).await?;
            Ok(serde_json::Value::Null)
        }
        HostRequest::ParticipantReject { name } => session.reject(&name).await.map(|_| serde_json::Value::Null),
        HostRequest::ParticipantKick { name } => session.kick(&name).await.map(|_| serde_json::Value::Null),
        // 그 사람이 잡고 있는 편집 잠금을 푼다(편집기를 열어 둔 채 자리를 비운 경우).
        HostRequest::ParticipantUnlock { name } => session.unlock(&name).await.map(|_| serde_json::Value::Null),
        HostRequest::CollectClose { closed } => {
            session.with(move |s| s.set_closed(closed)).await?;
            Ok(serde_json::Value::Null)
        }
        HostRequest::Models => {
            json(session.with(|s| s.sheets.iter().map(|x| x.model.clone()).collect::<Vec<SheetModel>>()).await?)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};

    use collect_core::config::{CollectConfig, suggested_key_col};
    use serde_json::json;
    use tokio::sync::mpsc;

    use super::*;
    use crate::SessionInit;
    use crate::protocol::{ClientMsg, PROTOCOL_VERSION};
    use crate::session::Command;

    fn session() -> SessionHandle {
        let model: SheetModel = serde_json::from_slice(include_bytes!("../../../fixtures/budget.sheet.json")).unwrap();
        let mut config = CollectConfig::suggest(&model);
        config.assignment.key_col = suggested_key_col(&model); // C 담당자
        config.requester = "박지훈".into();
        SessionHandle::spawn(SessionInit::new(model, config))
    }

    async fn run(session: &SessionHandle, req: serde_json::Value) -> serde_json::Value {
        let req: HostRequest = serde_json::from_value(req).unwrap();
        execute(session, req).await.unwrap()
    }

    #[test]
    fn requests_travel_as_tagged_camel_case_json() {
        let req = HostRequest::ParticipantApprove {
            name: "새사람".into(),
            sheet: 0,
            range: Some("F5:F9".into()),
            note: "검토".into(),
        };
        let text = serde_json::to_value(&req).unwrap();
        assert_eq!(
            text,
            json!({ "req": "participantApprove", "name": "새사람", "sheet": 0, "range": "F5:F9", "note": "검토" })
        );
        assert_eq!(serde_json::from_value::<HostRequest>(text).unwrap(), req);
        let csv: HostRequest =
            serde_json::from_value(json!({ "req": "historyCsv", "filter": {}, "offsetMinutes": 540 })).unwrap();
        assert_eq!(csv, HostRequest::HistoryCsv { filter: HistoryFilter::default(), offset_minutes: 540 });
        let cells: HostRequest = serde_json::from_value(json!({
            "req": "sheetSetCells", "sheet": 0, "cells": [{ "row": 4, "col": 5, "value": 3 }]
        }))
        .unwrap();
        assert_eq!(
            cells,
            HostRequest::SheetSetCells {
                sheet: 0,
                cells: vec![CellWrite { row: 4, col: 5, value: CellValue::Number(3.0) }]
            }
        );
        assert!(serde_json::from_value::<HostRequest>(json!({ "req": "deleteEverything" })).is_err());
        assert!(HostRequest::CommsGet.is_read() && !HostRequest::CollectClose { closed: true }.is_read());
    }

    #[tokio::test]
    async fn host_requests_drive_the_session() {
        let s = session();
        let at = CellRef::new(4, 5);
        run(&s, json!({ "req": "sheetSetCells", "sheet": 0, "cells": [{ "row": 4, "col": 5, "value": 4321 }] })).await;
        let data = run(&s, json!({ "req": "monitorGet", "sheet": 0 })).await;
        let model: SheetModel = serde_json::from_value(data["model"].clone()).unwrap();
        assert_eq!(model.cell(at).unwrap().v, CellValue::Number(4321.0));

        let page = run(&s, json!({ "req": "historyQuery", "filter": {}, "offset": 0, "limit": 10 })).await;
        assert_eq!(page["totalEntries"], 1);
        let csv: CsvExport =
            serde_json::from_value(run(&s, json!({ "req": "historyCsv", "filter": {}, "offsetMinutes": 540 })).await)
                .unwrap();
        assert_eq!(csv.count, 1);
        assert!(csv.csv.contains("4321"), "{}", csv.csv);

        // '이 값으로' → 되돌리기(묶음 하나).
        run(&s, json!({ "req": "historySetValue", "sheet": 0, "at": { "row": 4, "col": 5 }, "value": 7 })).await;
        let timeline = run(&s, json!({ "req": "historyTimeline", "sheet": 0, "at": { "row": 4, "col": 5 } })).await;
        assert_eq!(timeline["entries"].as_array().unwrap().len(), 2);
        let seq = timeline["entries"][1]["seq"].clone();
        let reverted = run(&s, json!({ "req": "historyRevert", "seqs": [seq], "force": false })).await;
        assert_eq!(reverted["changed"], 1);
        let current = s.with(move |x| x.sheets[0].model.cell(at).unwrap().v.clone()).await.unwrap();
        assert_eq!(current, CellValue::Number(4321.0));

        run(&s, json!({ "req": "snapshotTake", "label": "점검" })).await;
        let list = run(&s, json!({ "req": "snapshotList" })).await;
        assert_eq!(list[0]["meta"]["label"], "점검");

        run(&s, json!({ "req": "collectClose", "closed": true })).await;
        assert!(s.with(|x| x.closed()).await.unwrap());
        assert!(s.snapshot().closed);
        let models = run(&s, json!({ "req": "models" })).await;
        assert_eq!(models.as_array().unwrap().len(), 1);
        assert!(execute(&s, HostRequest::MonitorGet { sheet: 3 }).await.is_err());
    }

    #[tokio::test]
    async fn read_requests_leave_the_session_alone() {
        let s = session();
        run(&s, json!({ "req": "sheetSetCells", "sheet": 0, "cells": [{ "row": 4, "col": 5, "value": 1 }] })).await;
        run(&s, json!({ "req": "snapshotTake", "label": "하나" })).await;
        let id = run(&s, json!({ "req": "snapshotList" })).await[0]["meta"]["id"].clone();
        let before = s.with(|x| (x.history().len(), x.snapshots().len(), x.closed())).await.unwrap();
        let reads = [
            json!({ "req": "configView", "sheet": 0 }),
            json!({ "req": "monitorGet", "sheet": 0 }),
            json!({ "req": "monitorProgress", "sheet": 0 }),
            json!({ "req": "monitorActivity", "sheet": 0 }),
            json!({ "req": "commsGet" }),
            json!({ "req": "historyQuery", "filter": {}, "offset": 0, "limit": 5 }),
            json!({ "req": "historyTimeline", "sheet": 0, "at": { "row": 4, "col": 5 } }),
            json!({ "req": "historyCsv", "filter": {}, "offsetMinutes": 0 }),
            json!({ "req": "snapshotList" }),
            json!({ "req": "snapshotDiff", "id": id }),
            json!({ "req": "participantsGet" }),
            json!({ "req": "models" }),
        ];
        for req in reads {
            let parsed: HostRequest = serde_json::from_value(req.clone()).unwrap();
            assert!(parsed.is_read(), "{req}");
            execute(&s, parsed).await.unwrap();
        }
        let after = s.with(|x| (x.history().len(), x.snapshots().len(), x.closed())).await.unwrap();
        assert_eq!(before, after);
    }

    #[tokio::test]
    async fn approve_with_a_range_assigns_it() {
        let s = session();
        let (outbox, mut rx) = mpsc::unbounded_channel();
        s.send(Command::Connect { conn: 1, ip: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 7)), outbox });
        let msg = ClientMsg::Join { name: "새사람".into(), token: None, protocol: PROTOCOL_VERSION };
        s.send(Command::Client { conn: 1, msg });
        s.with(|_| ()).await.unwrap(); // 접속 표가 들어온 요청을 반영한 뒤 읽는다
        let pending = run(&s, json!({ "req": "participantsGet" })).await["pending"].clone();
        assert_eq!(pending[0]["name"], "새사람");

        let bad =
            json!({ "req": "participantApprove", "name": "새사람", "sheet": 0, "range": "없는시트!F5", "note": "" });
        assert!(execute(&s, serde_json::from_value(bad).unwrap()).await.is_err(), "없는 시트");
        assert!(!s.snapshot().pending.is_empty(), "실패한 승인은 대기를 그대로 둔다");
        run(&s, json!({ "req": "participantApprove", "name": "새사람", "sheet": 0, "range": "F5:F9", "note": "검토" }))
            .await;
        let ranges = s.with(|x| x.sheets[0].config.assignment.ranges.clone()).await.unwrap();
        let mine: Vec<_> = ranges.iter().filter(|r| r.person == "새사람").collect();
        assert_eq!(mine.len(), 1);
        assert_eq!(mine[0].range, CellRange { row: 4, col: 5, rows: 5, cols: 1 });
        assert_eq!(mine[0].note, "검토");
        let snap = s.snapshot();
        assert!(snap.pending.is_empty());
        assert!(snap.participants.iter().any(|p| p.name == "새사람"));
        assert!(rx.try_recv().is_ok(), "참여자에게 보낸 것이 있어야 함");

        run(&s, json!({ "req": "participantKick", "name": "새사람" })).await;
        assert!(execute(&s, HostRequest::ParticipantKick { name: "없는사람".into() }).await.is_err());
    }
}
