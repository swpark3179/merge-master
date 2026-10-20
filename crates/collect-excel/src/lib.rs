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

use collect_core::{CellRef, CellValue, SheetModel};
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

    /// 계산 사본을 연다(모든 시트, `models`·`sheets`는 세션 안 시트 순번 순서). 수식이 다른 시트를 볼 수 있어 수식이 없는
    /// 시트도 사본에 둔다. `key`는 Excel 계산 사본의 키(업무·서비스 id), 상태가 바뀌면 `notify`를 부른다. tokio 런타임
    /// 안에서 부른다.
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        &self,
        excel: Arc<ExcelThread>,
        key: &str,
        session: SessionHandle,
        models: &[SheetModel],
        source: CalcSource,
        sheets: &[String],
        notify: Notify,
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
        let (status, current) = (self.status.clone(), self.generation.clone());
        let key = key.to_string();
        if targets.iter().all(|t| t.formulas.is_empty()) {
            set(&notify, &status, RecalcStatus::None);
            tokio::spawn(async move {
                let _ = run_excel(&excel, move |s| {
                    s.close_calc_for(&key);
                    Ok(())
                })
                .await;
            });
            return;
        }
        set(&notify, &status, RecalcStatus::Opening);
        tokio::spawn(async move {
            // 원본을 찾아(닫혔으면 다시 열어) 계산 사본을 만든다. 앱을 다시 켜 이어서 할 때도 같은 길. 에이전트는 넘겨받은
            // 원본 파일에서 바로 만든다.
            let k = key.clone();
            let opened = run_excel(&excel, move |s| match &source {
                CalcSource::Workbook(id) => {
                    s.locate(id)?;
                    s.open_calc_for(&k, id, targets)
                }
                CalcSource::File(path) => s.open_calc_file_for(&k, path, targets),
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
                    return set(&notify, &status, RecalcStatus::Unavailable { message });
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
            set(&notify, &status, RecalcStatus::Ready { formulas: info.formulas as u32 });
            let worker = Worker { notify, excel, key, session, status, current, generation, path: info.path };
            worker.run(rx, earlier).await;
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
