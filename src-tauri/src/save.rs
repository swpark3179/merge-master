//! 6 저장·마감: 요약, 원본과 비교(그사이 Excel에서 고친 칸), 엑셀로 저장, 저장 뒤 마감.
//!
//! 세션에서 기준 시트·지금 시트·이력을 한 번에 떠서 저장 계획을 만들고(`collect_core::export`), Excel 스레드에서 쓴다.
//! 원본에 반영하면 쓴 값이 새 기준이 되어, 다시 저장할 때는 그 뒤에 바뀐 칸만 쓴다.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use collect_core::a1::sheet_prefix;
use collect_core::config::CollectConfig;
use collect_core::export::{self, CheckReport, ExportOptions, ExportPlan, SaveSummary, SummaryInput};
use collect_core::history::{BatchOrigin, ChangeEntry, SheetLabels};
use collect_core::{CellRef, CellValue, SheetCell, SheetModel};
use excel_com::{SaveReport, SourceStatus};
use serde::{Deserialize, Serialize};
use tauri::State;
use ts_rs::TS;

use crate::{AppState, CmdResult, run_excel};

/// 'Excel 값 유지'로 고친 칸의 사용자 이름(이력에 남는다).
const EXCEL_USER: &str = "Excel(원본)";

#[derive(Debug, Clone, Deserialize, Serialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum SaveTarget {
    /// 원본 통합문서에 쓰고 저장한다.
    Original,
    /// 원본을 복사해 새 파일에 쓴다(원본은 그대로).
    Copy { path: String },
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum AfterSave {
    /// 서비스를 그대로 둔다.
    Keep,
    /// 참여자 입력을 막는다(취합 마감).
    ReadOnly,
    /// 서비스를 멈춘다.
    Stop,
}

#[derive(Debug, Clone, Deserialize, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SaveOptions {
    pub target: SaveTarget,
    pub export: ExportOptions,
    pub after: AfterSave,
}

/// 저장할 원본(1a에서 확정한 통합문서·시트들 — 세션 안 시트 순번 순서).
#[derive(Debug, Clone)]
pub(crate) struct Source {
    pub workbook_id: String,
    pub sheets: Vec<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LastSave {
    #[ts(type = "number")]
    pub at_ms: i64,
    pub path: String,
    pub original: bool,
    /// 저장할 때까지 쌓인 변경 기록 수(그 뒤 변경 = 지금 기록 수 − 이것).
    pub history_entries: u32,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SaveInfo {
    pub summary: SaveSummary,
    pub workbook: String,
    /// 저장할 시트(세션 안 순번 순서).
    pub sheets: Vec<String>,
    /// 원본이 지금 Excel에 열려 있으면 그 상태(닫혀 있으면 저장할 때 다시 연다).
    #[ts(optional)]
    pub source: Option<SourceStatus>,
    /// 원본 파일 경로(저장한 적 없는 새 문서면 없음).
    #[ts(optional)]
    pub source_path: Option<String>,
    /// 다른 이름으로 저장할 때 기본 경로: `원본명_취합_YYYYMMDD.xlsx`.
    pub default_copy: String,
    pub closed: bool,
    pub running: bool,
    #[ts(optional)]
    pub last_save: Option<LastSave>,
}

/// 시트 하나의 저장 전 확인.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SheetCheck {
    pub name: String,
    pub report: CheckReport,
    /// 값을 쓸 칸 수.
    pub writes: u32,
    pub styled: u32,
    /// 불러온 범위의 시트 위치(주소 표시용).
    pub origin: CellRef,
    /// 열 이름(충돌 표에 '신청 금액'처럼 보인다).
    pub labels: Vec<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SaveCheck {
    pub source: SourceStatus,
    /// 시트마다(세션 안 순번 순서). 충돌 칸은 그 시트의 모델 좌표.
    pub sheets: Vec<SheetCheck>,
    /// 값을 쓸 칸 수(모든 시트).
    pub writes: u32,
    pub styled: u32,
    /// 어느 시트든 표 위치가 밀린 것 같다('그래도 저장'을 확인해야 한다).
    pub suspicious: bool,
    /// 다른 이름으로 저장할 파일이 이미 있다.
    pub target_exists: bool,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SaveResult {
    pub report: SaveReport,
    pub original: bool,
    /// 'Excel 값 유지'로 취합 값을 바꾼 칸 수.
    pub kept: u32,
    pub closed: bool,
    pub stopped: bool,
}

/// 세션에서 한 번에 뜬 저장 재료(시트마다 + 모든 기록).
struct Snapshot {
    sheets: Vec<SheetParts>,
    history: Vec<ChangeEntry>,
}

struct SheetParts {
    baseline: SheetModel,
    current: SheetModel,
    config: CollectConfig,
    labels: Vec<String>,
}

fn source(state: &AppState) -> CmdResult<Source> {
    state.source.lock().unwrap().clone().ok_or_else(|| crate::NO_SHEET.to_string())
}

async fn snapshot(state: &AppState) -> CmdResult<Snapshot> {
    state
        .session()?
        .with(|s| Snapshot {
            sheets: s
                .sheets
                .iter()
                .map(|x| SheetParts {
                    baseline: x.baseline.clone(),
                    current: x.model.clone(),
                    config: x.config.clone(),
                    labels: SheetLabels::of(&x.model).labels,
                })
                .collect(),
            history: s.history().to_vec(),
        })
        .await
}

fn utc_offset_minutes() -> i32 {
    chrono::Local::now().offset().local_minus_utc() / 60
}

/// `budget.xlsx` → `budget_취합_20261009.xlsx` (원본 폴더, 새 문서면 문서 폴더).
fn default_copy_path(name: &str, folder: &str) -> String {
    let path = Path::new(name);
    let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| name.to_string());
    let ext = path.extension().map(|e| e.to_string_lossy().to_string()).unwrap_or_else(|| "xlsx".into());
    let dir = if folder.is_empty() {
        std::env::var_os("USERPROFILE").map(|h| PathBuf::from(h).join("Documents")).unwrap_or_default()
    } else {
        PathBuf::from(folder)
    };
    let today = chrono::Local::now().format("%Y%m%d");
    dir.join(format!("{stem}_취합_{today}.{ext}")).to_string_lossy().to_string()
}

#[tauri::command]
pub async fn save_info(state: State<'_, AppState>) -> CmdResult<SaveInfo> {
    let src = source(&state)?;
    let snap = snapshot(&state).await?;
    let closed = state.session()?.with(|s| s.closed()).await?;
    let parts: Vec<SummaryInput<'_>> = snap
        .sheets
        .iter()
        .map(|x| SummaryInput { baseline: &x.baseline, current: &x.current, config: &x.config })
        .collect();
    let summary = export::summary_all(&parts, &snap.history);
    let workbook = snap.sheets[0].current.workbook.clone();
    let id = src.workbook_id.clone();
    // 열려 있는지만 본다(닫혀 있어도 다시 열지 않는다).
    let status = run_excel(&state.excel, move |s| s.peek(&id)).await.unwrap_or(None);
    let source_path = Path::new(&src.workbook_id).is_absolute().then(|| src.workbook_id.clone());
    let (name, folder) = match (&status, &source_path) {
        (Some(st), _) => (st.name.clone(), st.folder.clone()),
        (None, Some(p)) => {
            let p = Path::new(p);
            let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            (name, p.parent().map(|d| d.to_string_lossy().to_string()).unwrap_or_default())
        }
        (None, None) => (workbook.clone(), String::new()),
    };
    Ok(SaveInfo {
        summary,
        workbook,
        sheets: src.sheets.clone(),
        source: status,
        source_path,
        default_copy: default_copy_path(&name, &folder),
        closed,
        running: state.server.lock().await.is_some(),
        last_save: state.last_save.lock().unwrap().clone(),
    })
}

/// 지금 Excel에 있는 값과 비교한다(원본이 닫혔으면 다시 연다). 시트마다 Excel 값(행 우선)도 돌려준다.
async fn check(state: &AppState, opts: &SaveOptions) -> CmdResult<(Snapshot, SaveCheck, Vec<Vec<CellValue>>)> {
    let src = source(state)?;
    let snap = snapshot(state).await?;
    if src.sheets.len() != snap.sheets.len() {
        return Err("불러온 시트와 세션의 시트가 맞지 않습니다. 다시 불러와 주세요".into());
    }
    let ranges: Vec<(String, CellRef, u32, u32)> = src
        .sheets
        .iter()
        .zip(&snap.sheets)
        .map(|(name, x)| (name.clone(), x.baseline.origin, x.baseline.rows, x.baseline.cols))
        .collect();
    let id = src.workbook_id.clone();
    let (status, excel) = run_excel(&state.excel, move |s| {
        let status = s.locate(&id)?;
        let values = ranges
            .iter()
            .map(|(sheet, origin, rows, cols)| s.read_range_values(&id, sheet, *origin, *rows, *cols))
            .collect::<excel_com::Result<Vec<_>>>()?;
        Ok((status, values))
    })
    .await?;
    let sheets: Vec<SheetCheck> = snap
        .sheets
        .iter()
        .zip(&src.sheets)
        .zip(&excel)
        .map(|((x, name), values)| {
            let cells = export::changed_cells(&x.baseline, &x.current);
            let styled = if opts.export.styles {
                export::plan(&x.baseline, &x.current, &[], &[], opts.export, 0, &HashSet::new())
                    .styles
                    .iter()
                    .map(|b| b.range.len() as u32)
                    .sum()
            } else {
                0
            };
            SheetCheck {
                name: name.clone(),
                report: export::compare(&x.baseline, &x.current, values, &cells),
                writes: cells.len() as u32,
                styled,
                origin: x.baseline.origin,
                labels: x.labels.clone(),
            }
        })
        .collect();
    let target_exists = match &opts.target {
        SaveTarget::Copy { path } => Path::new(path).exists(),
        SaveTarget::Original => false,
    };
    let check = SaveCheck {
        source: status,
        writes: sheets.iter().map(|x| x.writes).sum(),
        styled: sheets.iter().map(|x| x.styled).sum(),
        suspicious: sheets.iter().any(|x| x.report.suspicious),
        sheets,
        target_exists,
    };
    Ok((snap, check, excel))
}

/// 저장 전 확인: 쓸 칸, 그사이 Excel에서 고친 칸(충돌), 표 위치가 밀렸는지, 저장하지 않은 Excel 변경.
#[tauri::command]
pub async fn save_check(state: State<'_, AppState>, options: SaveOptions) -> CmdResult<SaveCheck> {
    Ok(check(&state, &options).await?.1)
}

/// 저장한다. 충돌 칸은 모두 `keep_excel`(Excel 값 유지)이나 `use_collected`(취합 값으로 덮어쓰기) 중 하나여야 한다.
/// 어느 시트든 표 위치가 바뀐 것 같으면 `force` 없이는 저장하지 않는다. 모든 시트를 한 번에 쓰고 한 번 저장한다.
#[tauri::command]
pub async fn save_run(
    state: State<'_, AppState>,
    options: SaveOptions,
    keep_excel: Vec<SheetCell>,
    use_collected: Vec<SheetCell>,
    force: bool,
) -> CmdResult<SaveResult> {
    let src = source(&state)?;
    let (snap, checked, excel) = check(&state, &options).await?;
    let decided: HashSet<SheetCell> = keep_excel.iter().chain(&use_collected).copied().collect();
    let many = checked.sheets.len() > 1;
    for (i, sheet) in checked.sheets.iter().enumerate() {
        if let Some(c) = sheet.report.conflicts.iter().find(|c| !decided.contains(&SheetCell::new(i as u32, c.at))) {
            let o = sheet.origin;
            let addr = CellRef::new(c.at.row + o.row, c.at.col + o.col);
            let prefix = if many { sheet_prefix(&sheet.name) } else { String::new() };
            return Err(format!(
                "{prefix}{addr}을(를) Excel에서도 고쳤습니다. 어느 값을 쓸지 고른 뒤 다시 저장해 주세요"
            ));
        }
    }
    if checked.suspicious && !force {
        return Err(
            "원본의 표 위치가 바뀐 것 같습니다(행·열을 넣거나 지움). 확인한 뒤 '그래도 저장'을 눌러 주세요".into()
        );
    }
    // 시트마다 'Excel 값 유지'로 쓰지 않을 칸.
    let keep: Vec<HashSet<CellRef>> = checked
        .sheets
        .iter()
        .enumerate()
        .map(|(i, sheet)| {
            let conflicts: HashSet<CellRef> = sheet.report.conflicts.iter().map(|c| c.at).collect();
            keep_excel.iter().filter(|c| c.sheet == i as u32 && conflicts.contains(&c.at())).map(|c| c.at()).collect()
        })
        .collect();
    let offset = utc_offset_minutes();
    let mut plans: Vec<(String, ExportPlan)> = snap
        .sheets
        .iter()
        .zip(&src.sheets)
        .zip(&keep)
        .map(|((x, name), keep)| {
            let opts = ExportOptions { history: false, ..options.export };
            (name.clone(), export::plan(&x.baseline, &x.current, &[], &[], opts, offset, keep))
        })
        .collect();
    // '변경 이력' 시트는 하나(시트가 둘 이상이면 '시트' 열).
    if options.export.history && !snap.history.is_empty() {
        let labels: Vec<SheetLabels> = snap
            .sheets
            .iter()
            .map(|x| SheetLabels { name: x.current.sheet.clone(), origin: x.current.origin, labels: x.labels.clone() })
            .collect();
        plans[0].1.history = Some(export::history_table(&labels, &snap.history, offset));
    }
    let original = matches!(options.target, SaveTarget::Original);
    let (id, target) = (src.workbook_id.clone(), options.target.clone());
    let plans_for_excel = plans.clone();
    let report = run_excel(&state.excel, move |s| match target {
        SaveTarget::Original => s.save_into_multi(&id, &plans_for_excel),
        SaveTarget::Copy { path } => s.save_copy_multi(&id, &plans_for_excel, Path::new(&path)),
    })
    .await?;

    // 'Excel 값 유지' 칸은 취합 값도 Excel 값으로 바꾸고, 원본에 반영했으면 쓴 값을 새 기준으로 삼는다.
    let kept: Vec<Vec<(CellRef, CellValue)>> = keep
        .iter()
        .zip(&excel)
        .zip(&snap.sheets)
        .map(|((keep, values), x)| {
            let cols = x.baseline.cols.max(1);
            keep.iter().map(|&at| (at, values[(at.row * cols + at.col) as usize].clone())).collect()
        })
        .collect();
    let written: Vec<HashMap<CellRef, CellValue>> = plans
        .iter()
        .map(|(_, plan)| {
            plan.blocks
                .iter()
                .flat_map(|b| {
                    b.values
                        .iter()
                        .enumerate()
                        .map(|(i, v)| (CellRef::new(b.range.row + i as u32, b.range.col), v.value.clone()))
                })
                .collect()
        })
        .collect();
    let styles = options.export.styles;
    let closed = matches!(options.after, AfterSave::ReadOnly);
    let kept_count = kept.iter().map(Vec::len).sum::<usize>() as u32;
    let history_entries = snap.history.len() as u32;
    // 저장 계획을 만든 시트(표 아래에 끼워 넣은 행을 기준 시트에 붙일 때 쓴 값 그대로).
    let planned: Vec<SheetModel> = snap.sheets.into_iter().map(|x| x.current).collect();
    let session = state.session()?;
    session
        .with(move |s| {
            for (i, ((kept, written), planned)) in kept.into_iter().zip(written).zip(&planned).enumerate() {
                let Some(slot) = s.sheets.get_mut(i) else { continue };
                for (at, v) in &kept {
                    if let Some(cell) = slot.baseline.cell_mut(*at) {
                        cell.v = v.clone();
                    }
                }
                if original {
                    // 표 아래에 끼워 넣은 행은 이제 원본에 있다: 기준 시트에도 붙인다.
                    collect_core::export::adopt_rows(&mut slot.baseline, planned);
                    for (at, v) in written {
                        if let Some(cell) = slot.baseline.cell_mut(at) {
                            cell.v = v;
                        }
                    }
                    if styles {
                        slot.baseline.styles = slot.model.styles.clone();
                        slot.baseline.merges = slot.model.merges.clone();
                        for (b, m) in slot.baseline.cells.iter_mut().zip(&slot.model.cells) {
                            b.s = m.s;
                        }
                    }
                }
                if !kept.is_empty() {
                    s.host_set(EXCEL_USER, BatchOrigin::Excel, i, kept);
                }
            }
            if closed {
                s.set_closed(true);
            }
        })
        .await?;

    let stopped = matches!(options.after, AfterSave::Stop);
    if stopped && let Some(server) = state.server.lock().await.take() {
        server.stop().await;
    }
    *state.last_save.lock().unwrap() = Some(LastSave {
        at_ms: chrono::Utc::now().timestamp_millis(),
        path: report.path.clone(),
        original,
        history_entries,
    });
    Ok(SaveResult { report, original, kept: kept_count, closed, stopped })
}

/// 취합 마감(참여자 입력을 막음) / 다시 열기.
#[tauri::command]
pub async fn collect_close(state: State<'_, AppState>, closed: bool) -> CmdResult<()> {
    state.session()?.with(move |s| s.set_closed(closed)).await
}

/// 저장한 파일을 탐색기에서 보여 준다.
#[tauri::command]
pub fn reveal_file(path: String) -> CmdResult<()> {
    std::process::Command::new("explorer.exe")
        .arg(format!("/select,{path}"))
        .spawn()
        .map(drop)
        .map_err(|e| format!("탐색기를 열 수 없습니다: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_name_keeps_extension_and_folder() {
        let p = default_copy_path("budget.xlsm", r"C:\work");
        assert!(p.starts_with(r"C:\work\budget_취합_2"), "{p}");
        assert!(p.ends_with(".xlsm"));
        assert!(default_copy_path("통합 문서1", "").ends_with(".xlsx"));
    }

    #[test]
    fn options_json_shape() {
        let o: SaveOptions = serde_json::from_str(
            r#"{"target":{"kind":"copy","path":"C:\\a.xlsx"},"export":{"styles":true,"history":false,"mark":16773296},"after":"readOnly"}"#,
        )
        .unwrap();
        assert!(matches!(o.target, SaveTarget::Copy { .. }) && matches!(o.after, AfterSave::ReadOnly));
        assert_eq!(o.export.mark, Some(0xFFF0B0));
    }
}
