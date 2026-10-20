//! 엑셀 저장·수식 계산을 맡는 곳. 실제로는 Excel([`crate::excel::ExcelEngine`], Windows), 시험·개발에는 [`FakeEngine`].

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use collect_core::SheetModel;
use collect_core::export::ExportPlan;
use collect_server::{RecalcStatus, SessionHandle};

/// 재계산 상태가 바뀔 때 부르는 곳.
pub type Notify = Arc<dyn Fn(&RecalcStatus) + Send + Sync>;
pub type EngineFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// 마감 때 저장할 것: 원본 파일을 `out`으로 복사해 계획을 쓴다.
#[derive(Debug, Clone)]
pub struct SaveJob {
    pub source: PathBuf,
    pub plans: Vec<(String, ExportPlan)>,
    pub out: PathBuf,
}

pub trait Engine: Send + Sync + 'static {
    /// Excel이 있다(없으면 서비스를 받지 않는다 — 마감 때 저장할 수 없다).
    fn excel_available(&self) -> bool;
    /// 저장하고 저장한 파일 경로를 돌려준다.
    fn save(&self, job: SaveJob) -> EngineFuture<Result<String, String>>;
    /// 서비스(`key`)의 계산 사본을 연다(수식이 없으면 상태만 알린다).
    fn start_recalc(
        &self,
        key: &str,
        session: SessionHandle,
        models: Vec<SheetModel>,
        source: PathBuf,
        sheets: Vec<String>,
        notify: Notify,
    );
    fn stop_recalc(&self, key: &str);
    /// 켤 때: 앞서 죽은 에이전트가 남긴 계산 사본을 정리한다.
    fn cleanup(&self) -> EngineFuture<()>;
    /// 끝낼 때: 계산 사본을 모두 닫고 띄운 Excel을 끝낸다(끝날 때까지 기다린다).
    fn shutdown(&self) -> EngineFuture<()>;
}

/// Excel 없이 쓰는 엔진(시험·`--fake-excel`): 계획 요약을 JSON으로 `out`에 쓴다. `fail`번은 저장에 실패한다.
#[derive(Default)]
pub struct FakeEngine {
    pub fail: AtomicU32,
    pub saved: Mutex<Vec<PathBuf>>,
}

impl FakeEngine {
    pub fn fail_next(&self, times: u32) {
        self.fail.store(times, Ordering::SeqCst);
    }
}

impl Engine for FakeEngine {
    fn excel_available(&self) -> bool {
        true
    }

    fn save(&self, job: SaveJob) -> EngineFuture<Result<String, String>> {
        let failing = self.fail.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1)).is_ok();
        let out = if failing {
            Err("시험용 저장 실패".to_string())
        } else {
            let summary: Vec<(String, usize)> = job.plans.iter().map(|(n, p)| (n.clone(), p.cells.len())).collect();
            std::fs::write(&job.out, serde_json::to_vec(&summary).expect("직렬화"))
                .map(|()| {
                    self.saved.lock().unwrap().push(job.out.clone());
                    job.out.display().to_string()
                })
                .map_err(|e| e.to_string())
        };
        Box::pin(async move { out })
    }

    fn start_recalc(
        &self,
        _key: &str,
        _session: SessionHandle,
        _models: Vec<SheetModel>,
        _source: PathBuf,
        _sheets: Vec<String>,
        notify: Notify,
    ) {
        notify(&RecalcStatus::None);
    }

    fn stop_recalc(&self, _key: &str) {}

    fn cleanup(&self) -> EngineFuture<()> {
        Box::pin(async {})
    }

    fn shutdown(&self) -> EngineFuture<()> {
        Box::pin(async {})
    }
}
