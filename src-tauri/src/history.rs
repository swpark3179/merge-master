//! 5 히스토리(1h): 변경 기록 보기·CSV, 셀 타임라인, 되돌리기(칸·묶음·이 값으로), 스냅샷(목록·지금 저장·비교·복원).

use std::collections::HashSet;
use std::path::PathBuf;

use collect_core::detect::{TableGuess, column_labels};
use collect_core::history::{self, BatchOrigin, ChangeEntry, HistoryFilter, HistoryPage, RevertPlan};
use collect_core::{CellRef, CellValue};
use collect_store::SnapshotMeta;
use serde::Serialize;
use tauri::State;
use ts_rs::TS;

use crate::{AppState, CmdResult};

/// 비교 보기에 보이는 최대 칸 수.
const DIFF_LIMIT: usize = 500;

#[tauri::command]
pub async fn history_query(
    state: State<'_, AppState>,
    filter: HistoryFilter,
    offset: u32,
    limit: u32,
) -> CmdResult<HistoryPage> {
    state.session()?.with(move |s| history::query(s.history(), &filter, offset as usize, limit as usize)).await
}

#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Timeline {
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

#[tauri::command]
pub async fn history_timeline(state: State<'_, AppState>, at: CellRef) -> CmdResult<Timeline> {
    state
        .session()?
        .with(move |s| {
            let cols = s.model.cols.max(1);
            let original = s
                .snapshots()
                .first()
                .and_then(|m| s.snapshot_values(m.id))
                .and_then(|v| v.get((at.row * cols + at.col) as usize).cloned())
                .unwrap_or_default();
            Timeline {
                at,
                original,
                current: s.model.cell(at).map(|c| c.v.clone()).unwrap_or_default(),
                entries: history::timeline(s.history(), at),
            }
        })
        .await
}

#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RevertResult {
    /// 되돌린 칸 수. `stale`이 있고 `force`가 아니면 0(아무것도 하지 않음).
    pub changed: u32,
    /// 기록 뒤에 다시 바뀐 칸(되돌리면 그 변경도 덮어쓴다).
    pub stale: Vec<CellRef>,
}

async fn revert(state: &AppState, pick: Pick, force: bool) -> CmdResult<RevertResult> {
    state
        .session()?
        .with(move |s| {
            let entries: Vec<&ChangeEntry> = match &pick {
                Pick::Seqs(seqs) => s.history().iter().filter(|e| seqs.contains(&e.seq)).collect(),
                Pick::Batch(batch) => history::batch_entries(s.history(), batch),
            };
            if entries.is_empty() {
                return Err("기록을 찾을 수 없습니다".to_string());
            }
            let RevertPlan { changes, stale } = history::revert_plan(s.model, &entries);
            if !stale.is_empty() && !force {
                return Ok(RevertResult { changed: 0, stale });
            }
            let n = changes.len() as u32;
            let user = s.host_name().to_string();
            s.host_set(&user, BatchOrigin::Revert, changes);
            Ok(RevertResult { changed: n, stale })
        })
        .await?
}

enum Pick {
    Seqs(HashSet<u64>),
    Batch(String),
}

/// 칸 하나(또는 몇 개)의 기록을 되돌린다.
#[tauri::command]
pub async fn history_revert(state: State<'_, AppState>, seqs: Vec<u64>, force: bool) -> CmdResult<RevertResult> {
    revert(&state, Pick::Seqs(seqs.into_iter().collect()), force).await
}

/// 묶음 전체를 되돌린다(범위 지우기·붙여넣기 '묶음 복원').
#[tauri::command]
pub async fn history_revert_batch(state: State<'_, AppState>, batch: String, force: bool) -> CmdResult<RevertResult> {
    revert(&state, Pick::Batch(batch), force).await
}

/// 셀 타임라인의 '이 값으로'.
#[tauri::command]
pub async fn history_set_value(state: State<'_, AppState>, at: CellRef, value: CellValue) -> CmdResult<()> {
    state
        .session()?
        .with(move |s| {
            let user = s.host_name().to_string();
            s.host_set(&user, BatchOrigin::Revert, vec![(at, value)]);
        })
        .await
}

/// CSV로 내보낸다(필터에 걸린 기록 전부). 쓴 줄 수를 돌려준다.
#[tauri::command]
pub async fn history_export_csv(state: State<'_, AppState>, filter: HistoryFilter, path: PathBuf) -> CmdResult<u32> {
    let offset = chrono::Local::now().offset().local_minus_utc() / 60;
    let (csv, n) = state
        .session()?
        .with(move |s| {
            let entries: Vec<ChangeEntry> = s.history().iter().filter(|e| filter.matches(e)).cloned().collect();
            let table = TableGuess { range: s.model.bounds(), header_rows: s.model.header_rows };
            let labels = column_labels(s.model, &table);
            (history::to_csv(&entries, s.model.origin, &labels, offset), entries.len() as u32)
        })
        .await?;
    std::fs::write(&path, csv).map_err(|e| format!("파일을 쓸 수 없습니다: {e}"))?;
    Ok(n)
}

// ------------------------------------------------------------ 스냅샷

#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SnapshotView {
    pub meta: SnapshotMeta,
    /// 지금과 다른 칸 수.
    pub diff: u32,
    /// 이 스냅샷 뒤에 바꾼 사람.
    pub users_after: Vec<String>,
}

#[tauri::command]
pub async fn snapshot_list(state: State<'_, AppState>) -> CmdResult<Vec<SnapshotView>> {
    state
        .session()?
        .with(|s| {
            let mut out: Vec<SnapshotView> = s
                .snapshots()
                .into_iter()
                .map(|meta| {
                    let diff = s.snapshot_values(meta.id).map_or(0, |v| history::diff(&v, s.model).len() as u32);
                    let users_after = history::users_after(s.history(), meta.seq);
                    SnapshotView { meta, diff, users_after }
                })
                .collect();
            out.reverse(); // 최근 것부터
            out
        })
        .await
}

#[tauri::command]
pub async fn snapshot_take(state: State<'_, AppState>, label: String) -> CmdResult<()> {
    state.session()?.with(move |s| s.take_snapshot(&label)).await
}

#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DiffCell {
    pub at: CellRef,
    #[ts(as = "Option<CellValue>", optional)]
    pub snapshot: CellValue,
    #[ts(as = "Option<CellValue>", optional)]
    pub current: CellValue,
}

#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SnapshotDiff {
    pub total: u32,
    /// 앞 몇 칸(행 우선).
    pub cells: Vec<DiffCell>,
    pub labels: Vec<String>,
}

#[tauri::command]
pub async fn snapshot_diff(state: State<'_, AppState>, id: u32) -> CmdResult<SnapshotDiff> {
    state
        .session()?
        .with(move |s| {
            let values = s.snapshot_values(id).ok_or("스냅샷을 찾을 수 없습니다")?;
            let cols = s.model.cols.max(1);
            let diff = history::diff(&values, s.model);
            let table = TableGuess { range: s.model.bounds(), header_rows: s.model.header_rows };
            Ok(SnapshotDiff {
                total: diff.len() as u32,
                cells: diff
                    .iter()
                    .take(DIFF_LIMIT)
                    .map(|&at| DiffCell {
                        at,
                        snapshot: values[(at.row * cols + at.col) as usize].clone(),
                        current: s.model.cell(at).map(|c| c.v.clone()).unwrap_or_default(),
                    })
                    .collect(),
                labels: column_labels(s.model, &table),
            })
        })
        .await?
}

/// 스냅샷 시점으로 복원한다(복원 직전 상태는 자동으로 스냅샷에 남는다). 바꾼 칸 수.
#[tauri::command]
pub async fn snapshot_restore(state: State<'_, AppState>, id: u32) -> CmdResult<u32> {
    state
        .session()?
        .with(move |s| {
            let user = s.host_name().to_string();
            s.restore(id, &user).map(|n| n as u32)
        })
        .await?
}
