//! 호스트 앱과 에이전트가 같이 쓰는 Excel 작업.
//!
//! - [`run_excel`]: Excel 스레드에 작업을 보내고 기다린다(tokio의 blocking 스레드에서).
//! - [`Recalc`]: 수식 재계산. 시트를 확정하면(에이전트는 서비스를 받으면) 계산 사본(숨은 Excel)을 열고, 세션이 보내는 입력을
//!   모아(300ms) 사본에서 계산한 뒤 수식 칸 결과를 세션에 돌려준다. 세션이 바뀐 칸만 참여자에게 보낸다.
//!
//! Excel을 쓸 수 없으면(설치 안 됨, 사본 열기 실패, 도중에 Excel이 닫힘) 수식은 저장할 때 계산하고 취합은 계속한다.
//! 업무마다 사본 하나(Excel의 사본 키 = 업무 id), 상태가 바뀌면 [`Notify`]를 부른다.
#![cfg(windows)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use collect_core::{CellRef, CellValue};
use collect_server::{RecalcRequest, RecalcStatus, SessionHandle};
use excel_com::{CalcTarget, ExcelThread, Session};
use tokio::sync::mpsc;

/// 입력을 모으는 시간. 붙여넣기·연속 입력을 한 번에 계산한다.
const DEBOUNCE: Duration = Duration::from_millis(300);

/// Excel 스레드에서 `f`를 실행하고 기다린다(tokio의 blocking 스레드에서 기다리므로 런타임을 막지 않는다).
pub async fn run_excel<R, F>(excel: &Arc<ExcelThread>, f: F) -> Result<R, String>
where
    R: Send + 'static,
    F: FnOnce(&mut Session) -> excel_com::Result<R> + Send + 'static,
{
    let excel = excel.clone();
    tokio::task::spawn_blocking(move || excel.run(f))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

/// 재계산 상태가 바뀔 때 부르는 곳(호스트 앱은 화면 이벤트, 에이전트는 상태 보기).
pub type Notify = Arc<dyn Fn(&RecalcStatus) + Send + Sync>;

/// 계산 사본을 만들 원본.
#[derive(Debug, Clone)]
pub enum CalcSource {
    /// 호스트 PC의 Excel에 열린(또는 다시 열) 통합문서(`FullName`).
    Workbook(String),
    /// 넘겨받은 원본 파일(에이전트). 실행 중인 Excel에는 열지 않는다.
    File(PathBuf),
}

/// 업무 하나의 재계산.
#[derive(Default)]
pub struct Recalc {
    status: Arc<std::sync::Mutex<RecalcStatus>>,
    /// 확정할 때마다(그리고 업무를 끝낼 때) 늘어난다. 앞선 확정의 작업은 자기 세대가 아니면 손을 뗀다.
    generation: Arc<AtomicU64>,
}

fn set(notify: &Notify, slot: &std::sync::Mutex<RecalcStatus>, status: RecalcStatus) {
    *slot.lock().unwrap() = status.clone();
    notify(&status);
}

impl Recalc {
    pub fn status(&self) -> RecalcStatus {
        self.status.lock().unwrap().clone()
    }

    /// 업무를 끝낼 때: 진행 중인 작업이 손을 떼게 하고 계산 사본을 닫는다(세션의 재계산 연결은 호출한 쪽이 끊는다).
    /// tokio 런타임 안에서 부른다.
    pub fn stop(&self, excel: &Arc<ExcelThread>, key: &str) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        let (excel, key) = (excel.clone(), key.to_string());
        tokio::spawn(async move {
            let _ = run_excel(&excel, move |s| {
                s.close_calc_for(&key);
                Ok(())
            })
            .await;
        });
    }

    /// 계산 사본을 연다(모든 시트, `sheets`는 세션 안 시트 순번 순서의 Excel 시트 이름). 수식이 다른 시트를 볼 수 있어
    /// 수식이 없는 시트도 사본에 둔다. 사본을 연 뒤 앱에서 지운 행을 지우고 숨긴 줄을 펴서 지금 기준 시트와 맞추고, 기록을
    /// 되짚는다. 여는 사이 행을 또 지웠으면(판이 바뀜) 다시 연다. `key`는 Excel 계산 사본의 키(업무·서비스 id), 상태가
    /// 바뀌면 `notify`를 부른다. tokio 런타임 안에서 부른다.
    pub fn start(
        &self,
        excel: Arc<ExcelThread>,
        key: &str,
        session: SessionHandle,
        source: CalcSource,
        sheets: &[String],
        notify: Notify,
    ) {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let (status, current) = (self.status.clone(), self.generation.clone());
        let (key, sheets) = (key.to_string(), sheets.to_vec());
        tokio::spawn(async move {
            loop {
                let Ok(layout) = session.calc_layout().await else { return };
                let targets: Vec<CalcTarget> = layout
                    .sheets
                    .into_iter()
                    .zip(&sheets)
                    .map(|(l, sheet)| CalcTarget {
                        sheet: sheet.clone(),
                        origin: CellRef::default(), // 아래에서 채운다
                        formulas: l.formulas,
                        cuts: l.cuts,
                        unhide_rows: l.unhide_rows,
                        unhide_cols: l.unhide_cols,
                    })
                    .collect();
                let Ok(origins) = session.with(|s| s.sheets.iter().map(|x| x.model.origin).collect::<Vec<_>>()).await
                else {
                    return;
                };
                let targets: Vec<CalcTarget> =
                    targets.into_iter().zip(origins).map(|(t, origin)| CalcTarget { origin, ..t }).collect();
                if targets.iter().all(|t| t.formulas.is_empty()) {
                    set(&notify, &status, RecalcStatus::None);
                    let k = key.clone();
                    let _ = run_excel(&excel, move |s| {
                        s.close_calc_for(&k);
                        Ok(())
                    })
                    .await;
                    return;
                }
                set(&notify, &status, RecalcStatus::Opening);
                // 원본을 찾아(닫혔으면 다시 열어) 계산 사본을 만든다. 앱을 다시 켜 이어서 할 때도 같은 길. 에이전트는
                // 넘겨받은 원본 파일에서 바로 만든다.
                let (k, src) = (key.clone(), source.clone());
                let opened = run_excel(&excel, move |s| match &src {
                    CalcSource::Workbook(id) => {
                        s.locate(id)?;
                        s.open_calc_for(&k, id, targets)
                    }
                    CalcSource::File(path) => s.open_calc_file_for(&k, path, targets),
                })
                .await;
                if current.load(Ordering::SeqCst) != generation {
                    // 그사이 다시 확정했거나 업무를 끝냈다. 끝냈으면 방금 연 사본을 닫는다(다시 확정했으면 새 사본이
                    // 대신한다).
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
                        return set(&notify, &status, RecalcStatus::Unavailable { message });
                    }
                };
                let (tx, rx) = mpsc::unbounded_channel();
                session.set_recalc(Some(tx));
                // 사본이 열리기 전에 들어온 입력도 반영한다(set_recalc 뒤의 입력은 채널로 온다. 겹쳐도 해롭지 않다).
                // 여는 사이 행을 지웠으면 사본 모양이 다르다: 다시 연다.
                let Ok(now) = session.calc_layout().await else { return };
                if now.epoch != layout.epoch {
                    session.set_recalc(None);
                    continue;
                }
                set(&notify, &status, RecalcStatus::Ready { formulas: info.formulas as u32 });
                let worker = Worker {
                    notify,
                    excel,
                    key,
                    session,
                    status,
                    current,
                    generation,
                    path: info.path,
                    epoch: now.epoch,
                };
                worker.run(rx, now.replay).await;
                return;
            }
        });
    }
}

struct Worker {
    notify: Notify,
    excel: Arc<ExcelThread>,
    /// Excel 계산 사본의 키(업무·서비스 id).
    key: String,
    session: SessionHandle,
    status: Arc<std::sync::Mutex<RecalcStatus>>,
    current: Arc<AtomicU64>,
    generation: u64,
    path: PathBuf,
    /// 이 사본의 판(세션이 행을 지우면 는다 — 결과에 실어 보낸다).
    epoch: u64,
}

/// 계산 사본에 할 일 하나(받은 순서대로 한다).
enum Step {
    Writes(Vec<(u32, CellRef, CellValue)>),
    /// 행 지우기(시트 순번, 그 순간의 모델 행 좌표).
    Cuts(Vec<(u32, collect_core::RowCut)>),
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

    /// 요청 하나를 할 일에 더한다: 행 지우기가 있으면 먼저(그 뒤의 입력은 지운 뒤 좌표다), 다음 입력. 판은 받은 요청 중
    /// 가장 새것(결과에 실어 보낸다 — 세션은 지금 판이 아닌 결과를 버린다).
    fn take(&mut self, req: RecalcRequest, queue: &mut Vec<Step>) {
        self.epoch = self.epoch.max(req.epoch);
        if !req.cuts.is_empty() {
            queue.push(Step::Cuts(req.cuts));
        }
        if !req.writes.is_empty() {
            queue.push(Step::Writes(req.writes));
        }
    }

    async fn run(mut self, mut rx: mpsc::UnboundedReceiver<RecalcRequest>, first: Vec<(u32, CellRef, CellValue)>) {
        // 연 직후에는 입력이 없어도 한 번 계산한다(지운 행·펼친 줄을 반영한 결과).
        let mut queue: Vec<Step> = vec![Step::Writes(first)];
        loop {
            if queue.is_empty() {
                match rx.recv().await {
                    Some(req) => self.take(req, &mut queue),
                    None => break, // 세션이 재계산을 껐다(다시 확정)
                }
                tokio::time::sleep(DEBOUNCE).await;
            }
            while let Ok(req) = rx.try_recv() {
                self.take(req, &mut queue);
            }
            let steps = std::mem::take(&mut queue);
            let (path, key) = (self.path.clone(), self.key.clone());
            let result = run_excel(&self.excel, move |s| {
                // 다시 확정해 다른 사본이 열렸으면 손대지 않는다.
                if s.calc_path_for(&key) != Some(path.as_path()) {
                    return Ok(None);
                }
                // 받은 순서대로: 행 지우기는 그때까지 모은 입력을 쓴 뒤에 한다(입력은 지우기 전 좌표).
                let mut writes = Vec::new();
                for step in steps {
                    match step {
                        Step::Writes(w) => writes.extend(w),
                        Step::Cuts(cuts) => {
                            s.calc_apply_writes_for(&key, &last_per_cell(std::mem::take(&mut writes)))?;
                            s.calc_cut_rows_for(&key, &cuts)?;
                        }
                    }
                }
                s.calc_apply_for(&key, &last_per_cell(writes)).map(Some)
            })
            .await;
            if !self.alive() {
                break;
            }
            match result {
                Ok(Some(values)) => self.session.formula_results_at(self.epoch, values),
                Ok(None) => break,
                Err(e) => {
                    tracing::warn!("수식 재계산 실패: {e}");
                    self.session.set_recalc(None);
                    let message = format!("수식은 저장할 때 계산합니다 ({e})");
                    set(&self.notify, &self.status, RecalcStatus::Unavailable { message });
                    break;
                }
            }
        }
    }
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
