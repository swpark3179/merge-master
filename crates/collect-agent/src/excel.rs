//! Excel로 저장·수식 계산(Windows). 원본 파일은 숨은 Excel에서만 열고, 이 PC 사용자의 Excel에는 열지 않는다.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use collect_core::SheetModel;
use collect_excel::{CalcSource, Recalc, run_excel};
use collect_server::SessionHandle;
use excel_com::ExcelThread;

use crate::engine::{Engine, EngineFuture, Notify, SaveJob};

/// Excel 작업 하나를 이만큼 기다린다(넘으면 저장 실패로 — Excel이 대화 상자 등으로 멈춘 경우).
const EXCEL_TIMEOUT: Duration = Duration::from_secs(600);

pub struct ExcelEngine {
    excel: Arc<ExcelThread>,
    installed: bool,
    recalcs: Mutex<HashMap<String, Arc<Recalc>>>,
}

impl ExcelEngine {
    /// Excel 스레드를 띄우고 계산 사본 폴더를 정한다(호스트 앱과 다른 폴더).
    pub fn new(work_dir: PathBuf) -> Result<Self, String> {
        let excel = ExcelThread::spawn().map_err(|e| format!("Excel 스레드를 시작할 수 없습니다: {e}"))?;
        let installed = excel
            .run(move |s| {
                s.set_work_dir(work_dir);
                s.excel_installed()
            })
            .unwrap_or(false);
        if !installed {
            tracing::warn!("Excel이 설치되어 있지 않습니다: 서비스를 받지 않습니다");
        }
        Ok(Self { excel: Arc::new(excel), installed, recalcs: Mutex::new(HashMap::new()) })
    }
}

impl Engine for ExcelEngine {
    fn excel_available(&self) -> bool {
        self.installed
    }

    fn save(&self, job: SaveJob) -> EngineFuture<Result<String, String>> {
        let excel = self.excel.clone();
        Box::pin(async move {
            let SaveJob { source, plans, out } = job;
            let work = run_excel(&excel, move |s| s.save_file_copy_multi(&source, &plans, &out));
            match tokio::time::timeout(EXCEL_TIMEOUT, work).await {
                Ok(Ok(report)) => {
                    if !report.mismatched.is_empty() {
                        tracing::warn!("저장 뒤 다시 읽은 값이 다른 칸 {}개: {}", report.mismatched.len(), report.path);
                    }
                    Ok(report.path)
                }
                Ok(Err(e)) => Err(e),
                Err(_) => {
                    Err("Excel 작업이 10분 안에 끝나지 않았습니다(Excel에 열린 대화 상자가 있는지 확인해 주세요)"
                        .into())
                }
            }
        })
    }

    fn start_recalc(
        &self,
        key: &str,
        session: SessionHandle,
        models: Vec<SheetModel>,
        source: PathBuf,
        sheets: Vec<String>,
        notify: Notify,
    ) {
        if !self.installed {
            notify(&collect_server::RecalcStatus::Unavailable { message: "이 PC에 Excel이 없습니다".into() });
            return;
        }
        let recalc = Arc::new(Recalc::default());
        recalc.start(self.excel.clone(), key, session, &models, CalcSource::File(source), &sheets, notify);
        self.recalcs.lock().unwrap().insert(key.to_string(), recalc);
    }

    fn stop_recalc(&self, key: &str) {
        if let Some(r) = self.recalcs.lock().unwrap().remove(key) {
            r.stop(&self.excel, key);
        }
    }

    fn shutdown(&self) -> EngineFuture<()> {
        let excel = self.excel.clone();
        self.recalcs.lock().unwrap().clear();
        Box::pin(async move {
            let _ = run_excel(&excel, |s| {
                s.close_all_calc();
                s.close_background();
                Ok(())
            })
            .await;
        })
    }

    fn cleanup(&self) -> EngineFuture<()> {
        let excel = self.excel.clone();
        let installed = self.installed;
        Box::pin(async move {
            if !installed {
                return;
            }
            match run_excel(&excel, |s| s.adopt_orphans_many(&[])).await {
                Ok(r) if r.closed_calc > 0 || r.removed_files > 0 => tracing::info!("앞서 남은 계산 사본 정리: {r:?}"),
                Ok(_) => {}
                Err(e) => tracing::warn!("계산 사본 정리 실패: {e}"),
            }
        })
    }
}
