//! 마감 때 엑셀로 저장하는 곳. 실제로는 Excel([`crate::excel::ExcelEngine`], Windows — 별도 저장 프로세스), 시험·개발에는
//! [`FakeEngine`]. 서비스 중에는 Excel을 쓰지 않는다(수식은 저장할 때 Excel이 계산한다).

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use agent_protocol::save_data::SaveData;

pub type EngineFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// 마감 때 저장할 것: 원본 파일(`source`)을 `out`으로 복사해 취합 데이터 파일(`data`, [`SaveData`])의 계획을 쓴다.
#[derive(Debug, Clone)]
pub struct SaveJob {
    pub data: PathBuf,
    pub source: PathBuf,
    pub out: PathBuf,
}

pub trait Engine: Send + Sync + 'static {
    /// Excel이 있다(없으면 마감 때 취합 데이터만 남기고 관리자 PC에서 저장한다).
    fn excel_available(&self) -> bool;
    /// 저장하고 저장한 파일 경로를 돌려준다.
    fn save(&self, job: SaveJob) -> EngineFuture<Result<String, String>>;
    /// 끝낼 때: 하고 있는 저장을 그만둔다(끝날 때까지 기다린다).
    fn shutdown(&self) -> EngineFuture<()>;
}

/// Excel 없이 쓰는 엔진(시험·`--fake-excel`): 취합 데이터 파일을 읽어 계획 요약을 JSON으로 `out`에 쓴다. `fail`번은
/// 저장에 실패한다. [`FakeEngine::without_excel`]은 Excel이 없는 PC.
#[derive(Default)]
pub struct FakeEngine {
    pub fail: AtomicU32,
    pub saved: Mutex<Vec<PathBuf>>,
    no_excel: AtomicBool,
}

impl FakeEngine {
    pub fn fail_next(&self, times: u32) {
        self.fail.store(times, Ordering::SeqCst);
    }

    pub fn without_excel() -> Self {
        let e = Self::default();
        e.no_excel.store(true, Ordering::SeqCst);
        e
    }
}

impl Engine for FakeEngine {
    fn excel_available(&self) -> bool {
        !self.no_excel.load(Ordering::SeqCst)
    }

    fn save(&self, job: SaveJob) -> EngineFuture<Result<String, String>> {
        let failing = self.fail.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1)).is_ok();
        let out = if failing {
            Err("시험용 저장 실패".to_string())
        } else {
            std::fs::read(&job.data)
                .map_err(|e| format!("취합 데이터를 읽을 수 없습니다: {e}"))
                .and_then(|bytes| SaveData::decode(&bytes))
                .and_then(|data| {
                    let summary: Vec<(String, usize)> =
                        data.plan().iter().map(|(n, p)| (n.clone(), p.cells.len())).collect();
                    std::fs::write(&job.out, serde_json::to_vec(&summary).expect("직렬화")).map_err(|e| e.to_string())
                })
                .map(|()| {
                    self.saved.lock().unwrap().push(job.out.clone());
                    job.out.display().to_string()
                })
        };
        Box::pin(async move { out })
    }

    fn shutdown(&self) -> EngineFuture<()> {
        Box::pin(async {})
    }
}
