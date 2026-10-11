//! 수식 재계산: 시트를 확정하면 계산 사본(숨은 Excel)을 열고, 세션이 보내는 입력을 모아(300ms) 사본에서 계산한 뒤
//! 수식 칸 결과를 세션에 돌려준다. 세션이 바뀐 칸만 참여자에게 보낸다.
//!
//! Excel을 쓸 수 없으면(설치 안 됨, 사본 열기 실패, 도중에 Excel이 닫힘) 수식은 저장할 때 계산하고 취합은 계속한다.
//!
//! 업무마다 사본 하나(Excel의 사본 키 = 업무 id), 상태 이벤트는 `recalc-status:{업무 id}`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use collect_core::{CellRef, CellValue, SheetModel};
use collect_server::{RecalcRequest, SessionHandle};
use excel_com::{CalcTarget, ExcelThread};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::mpsc;
use ts_rs::TS;

use crate::jobs::event;
use crate::{AppState, CmdResult, run_excel};

/// 입력을 모으는 시간. 붙여넣기·연속 입력을 한 번에 계산한다.
const DEBOUNCE: Duration = Duration::from_millis(300);

#[derive(Debug, Clone, Default, PartialEq, Serialize, TS)]
#[serde(tag = "state", rename_all = "camelCase")]
#[ts(export, rename = "RecalcStatus")]
pub enum Status {
    /// 수식이 없는 시트(모든 시트).
    #[default]
    None,
    Opening,
    Ready {
        formulas: u32,
    },
    /// 수식은 저장할 때 계산한다.
    Unavailable {
        message: String,
    },
}

/// 업무 하나의 재계산.
#[derive(Default)]
pub(crate) struct Recalc {
    status: Arc<std::sync::Mutex<Status>>,
    /// 확정할 때마다(그리고 업무를 끝낼 때) 늘어난다. 앞선 확정의 작업은 자기 세대가 아니면 손을 뗀다.
    generation: Arc<AtomicU64>,
}

fn set(app: &AppHandle, job: &str, slot: &std::sync::Mutex<Status>, status: Status) {
    *slot.lock().unwrap() = status.clone();
    let _ = app.emit(&event("recalc-status", job), status);
}

impl Recalc {
    pub(crate) fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }

    /// 업무를 끝낼 때: 진행 중인 작업이 손을 떼게 하고 계산 사본을 닫는다(세션의 재계산 연결은 호출한 쪽이 끊는다).
    pub(crate) fn stop(&self, excel: &Arc<ExcelThread>, job: &str) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        let (excel, key) = (excel.clone(), job.to_string());
        tauri::async_runtime::spawn(async move {
            let _ = run_excel(&excel, move |s| {
                s.close_calc_for(&key);
                Ok(())
            })
            .await;
        });
    }

    /// 계산 사본을 연다(모든 시트, `models`·`sheets`는 세션 안 시트 순번 순서). 수식이 다른 시트를 볼 수 있어 수식이 없는
    /// 시트도 사본에 둔다.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn start(
        &self,
        app: &AppHandle,
        excel: Arc<ExcelThread>,
        job: &str,
        session: SessionHandle,
        models: &[SheetModel],
        workbook_id: &str,
        sheets: &[String],
    ) {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let targets: Vec<CalcTarget> = models
            .iter()
            .zip(sheets)
            .map(|(model, sheet)| {
                let cols = model.cols.max(1);
                let formulas = model
                    .cells
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| c.f.is_some())
                    .map(|(i, _)| CellRef::new(i as u32 / cols, i as u32 % cols))
                    .collect();
                CalcTarget { sheet: sheet.clone(), origin: model.origin, formulas }
            })
            .collect();
        let (app, status, current) = (app.clone(), self.status.clone(), self.generation.clone());
        let key = job.to_string();
        if targets.iter().all(|t| t.formulas.is_empty()) {
            set(&app, &key, &status, Status::None);
            tauri::async_runtime::spawn(async move {
                let _ = run_excel(&excel, move |s| {
                    s.close_calc_for(&key);
                    Ok(())
                })
                .await;
            });
            return;
        }
        set(&app, &key, &status, Status::Opening);
        let id = workbook_id.to_string();
        tauri::async_runtime::spawn(async move {
            // 원본을 찾아(닫혔으면 다시 열어) 계산 사본을 만든다. 앱을 다시 켜 이어서 할 때도 같은 길.
            let k = key.clone();
            let opened = run_excel(&excel, move |s| {
                s.locate(&id)?;
                s.open_calc_for(&k, &id, targets)
            })
            .await;
            if current.load(Ordering::SeqCst) != generation {
                // 그사이 다시 확정했거나 업무를 끝냈다. 끝냈으면 방금 연 사본을 닫는다(다시 확정했으면 새 사본이 대신한다).
                if let Ok(info) = opened {
                    let k = key.clone();
                    let _ = run_excel(&excel, move |s| {
                        if s.calc_path_for(&k) == Some(info.path.as_path()) {
                            s.close_calc_for(&k);
                        }
                        Ok(())
                    })
                    .await;
                }
                return;
            }
            let info = match opened {
                Ok(info) => info,
                Err(e) => {
                    tracing::warn!("계산 사본을 열 수 없습니다: {e}");
                    let message = format!("수식은 저장할 때 계산합니다 ({e})");
                    return set(&app, &key, &status, Status::Unavailable { message });
                }
            };
            let (tx, rx) = mpsc::unbounded_channel();
            session.set_recalc(Some(tx));
            // 사본이 열리기 전에 들어온 입력도 반영한다(set_recalc 뒤의 입력은 채널로 온다. 겹쳐도 해롭지 않다).
            let earlier = session
                .with(|s| {
                    let mut last: HashMap<(u32, CellRef), CellValue> = HashMap::new();
                    for e in s.history() {
                        last.insert((e.sheet, CellRef::new(e.row, e.col)), e.new.clone());
                    }
                    last.into_iter().map(|((sheet, at), v)| (sheet, at, v)).collect::<Vec<_>>()
                })
                .await
                .unwrap_or_default();
            set(&app, &key, &status, Status::Ready { formulas: info.formulas as u32 });
            let job = Worker { app, excel, key, session, status, current, generation, path: info.path };
            job.run(rx, earlier).await;
        });
    }
}

struct Worker {
    app: AppHandle,
    excel: Arc<ExcelThread>,
    /// 업무 id(Excel 계산 사본의 키).
    key: String,
    session: SessionHandle,
    status: Arc<std::sync::Mutex<Status>>,
    current: Arc<AtomicU64>,
    generation: u64,
    path: PathBuf,
}

/// 같은 칸(시트·칸)을 여러 번 바꿨으면 마지막 값만 쓴다(순서는 마지막으로 바뀐 순).
fn last_per_cell(writes: Vec<(u32, CellRef, CellValue)>) -> Vec<(u32, CellRef, CellValue)> {
    let mut at: HashMap<(u32, CellRef), usize> = HashMap::new();
    let mut out: Vec<Option<(u32, CellRef, CellValue)>> = Vec::with_capacity(writes.len());
    for (sheet, cell, v) in writes {
        if let Some(i) = at.insert((sheet, cell), out.len()) {
            out[i] = None;
        }
        out.push(Some((sheet, cell, v)));
    }
    out.into_iter().flatten().collect()
}

impl Worker {
    fn alive(&self) -> bool {
        self.current.load(Ordering::SeqCst) == self.generation
    }

    async fn run(self, mut rx: mpsc::UnboundedReceiver<RecalcRequest>, mut queue: Vec<(u32, CellRef, CellValue)>) {
        loop {
            if queue.is_empty() {
                match rx.recv().await {
                    Some(req) => queue.extend(req.writes),
                    None => break, // 세션이 재계산을 껐다(다시 확정)
                }
                tokio::time::sleep(DEBOUNCE).await;
            }
            while let Ok(req) = rx.try_recv() {
                queue.extend(req.writes);
            }
            let writes = last_per_cell(std::mem::take(&mut queue));
            let (path, key) = (self.path.clone(), self.key.clone());
            let result = run_excel(&self.excel, move |s| {
                // 다시 확정해 다른 사본이 열렸으면 손대지 않는다.
                if s.calc_path_for(&key) != Some(path.as_path()) {
                    return Ok(None);
                }
                s.calc_apply_for(&key, &writes).map(Some)
            })
            .await;
            if !self.alive() {
                break;
            }
            match result {
                Ok(Some(values)) => self.session.formula_results(values),
                Ok(None) => break,
                Err(e) => {
                    tracing::warn!("수식 재계산 실패: {e}");
                    self.session.set_recalc(None);
                    let message = format!("수식은 저장할 때 계산합니다 ({e})");
                    set(&self.app, &self.key, &self.status, Status::Unavailable { message });
                    break;
                }
            }
        }
    }
}

#[tauri::command]
pub(crate) fn recalc_status(state: State<'_, AppState>, job: String) -> CmdResult<Status> {
    Ok(state.job(&job)?.recalc.status())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_last_write_per_cell() {
        let a = CellRef::new(2, 5);
        let b = CellRef::new(3, 5);
        let n = CellValue::Number;
        let got = last_per_cell(vec![(0, a, n(1.0)), (0, b, n(2.0)), (1, a, n(9.0)), (0, a, n(3.0))]);
        assert_eq!(got, [(0, b, n(2.0)), (1, a, n(9.0)), (0, a, n(3.0))]);
    }
}
