//! 5 히스토리(1h): 변경 기록 보기·CSV, 셀 타임라인, 되돌리기(칸·묶음·이 값으로), 스냅샷(목록·지금 저장·비교·복원).

use std::collections::HashSet;
use std::path::PathBuf;

use collect_core::history::{self, BatchOrigin, ChangeEntry, HistoryFilter, HistoryPage, RevertPlan, SheetLabels};
use collect_core::{CellRef, CellValue, SheetCell};
use collect_server::Shared;
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

#[tauri::command]
pub async fn history_timeline(state: State<'_, AppState>, sheet: u32, at: CellRef) -> CmdResult<Timeline> {
    state
        .session()?
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
            Ok(Timeline { sheet, at, original, current, entries: history::timeline(s.history(), sheet, at) })
        })
        .await?
}

#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RevertResult {
    /// 되돌린 칸 수. `stale`이 있고 `force`가 아니면 0(아무것도 하지 않음).
    pub changed: u32,
    /// 기록 뒤에 다시 바뀐 칸(되돌리면 그 변경도 덮어쓴다).
    pub stale: Vec<SheetCell>,
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
pub async fn history_set_value(state: State<'_, AppState>, sheet: u32, at: CellRef, value: CellValue) -> CmdResult<()> {
    state
        .session()?
        .with(move |s| {
            let user = s.host_name().to_string();
            s.host_set(&user, BatchOrigin::Revert, sheet as usize, vec![(at, value)]);
        })
        .await
}

/// CSV로 내보낸다(필터에 걸린 기록 전부). 쓴 줄 수를 돌려준다. 시트가 둘 이상이면 '시트' 열이 있다.
#[tauri::command]
pub async fn history_export_csv(state: State<'_, AppState>, filter: HistoryFilter, path: PathBuf) -> CmdResult<u32> {
    let offset = chrono::Local::now().offset().local_minus_utc() / 60;
    let (csv, n) = state
        .session()?
        .with(move |s| {
            let entries: Vec<ChangeEntry> = s.history().iter().filter(|e| filter.matches(e)).cloned().collect();
            let sheets: Vec<SheetLabels> = s.sheets.iter().map(|x| SheetLabels::of(&x.model)).collect();
            (history::to_csv(&entries, &sheets, offset), entries.len() as u32)
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
    /// 지금과 다른 칸 수(모든 시트).
    pub diff: u32,
    /// 이 스냅샷 뒤에 바꾼 사람.
    pub users_after: Vec<String>,
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

#[tauri::command]
pub async fn snapshot_list(state: State<'_, AppState>) -> CmdResult<Vec<SnapshotView>> {
    state
        .session()?
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
    pub sheet: u32,
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
    /// 앞 몇 칸(시트 순서, 행 우선).
    pub cells: Vec<DiffCell>,
    /// 시트마다 이름·위치·열 이름(주소·항목 표시용).
    pub sheets: Vec<DiffSheet>,
}

#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DiffSheet {
    pub name: String,
    pub origin: CellRef,
    pub labels: Vec<String>,
}

#[tauri::command]
pub async fn snapshot_diff(state: State<'_, AppState>, id: u32) -> CmdResult<SnapshotDiff> {
    state
        .session()?
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
            Ok(SnapshotDiff {
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
