//! Excel 전용 STA 스레드.
//!
//! COM 개체는 만든 스레드(아파트) 밖으로 나갈 수 없다(`!Send`). 그래서 모든 Excel 작업을 스레드 하나에서
//! 순서대로 실행하고, 다른 스레드는 [`ExcelThread::run`]으로 작업(클로저)을 보내 결과만 받는다.

use std::sync::mpsc;
use std::thread::JoinHandle;

use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};

use crate::error::{ExcelError, Result};
use crate::session::Session;

type Job = Box<dyn FnOnce(&mut Session) + Send>;

pub struct ExcelThread {
    tx: Option<mpsc::Sender<Job>>,
    join: Option<JoinHandle<()>>,
}

impl ExcelThread {
    pub fn spawn() -> Result<Self> {
        let (tx, rx) = mpsc::channel::<Job>();
        let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<()>>(1);
        let join = std::thread::Builder::new()
            .name("excel-com".into())
            .spawn(move || {
                let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
                if let Err(e) = hr.ok() {
                    let _ = ready_tx.send(Err(e.into()));
                    return;
                }
                let _ = ready_tx.send(Ok(()));
                {
                    let mut session = Session::default();
                    while let Ok(job) = rx.recv() {
                        // 작업 하나가 panic해도 스레드(와 열린 Excel 핸들)는 살려 둔다. 그 작업의 호출자만
                        // 응답 채널이 끊겨 ThreadGone을 받는다.
                        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job(&mut session))).is_err() {
                            tracing::error!("Excel 작업 중 panic — 다음 작업은 계속 처리합니다");
                        }
                    }
                    // session drop: 직접 띄운 백그라운드 Excel 정리
                }
                unsafe { CoUninitialize() };
            })
            .map_err(|_| ExcelError::ThreadGone)?;
        ready_rx.recv().map_err(|_| ExcelError::ThreadGone)??;
        Ok(Self { tx: Some(tx), join: Some(join) })
    }

    /// Excel 스레드에서 `f`를 실행하고 끝날 때까지 기다린다.
    pub fn run<R, F>(&self, f: F) -> Result<R>
    where
        R: Send + 'static,
        F: FnOnce(&mut Session) -> R + Send + 'static,
    {
        let (tx, rx) = mpsc::sync_channel(1);
        let job: Job = Box::new(move |s| {
            let _ = tx.send(f(s));
        });
        self.tx.as_ref().ok_or(ExcelError::ThreadGone)?.send(job).map_err(|_| ExcelError::ThreadGone)?;
        rx.recv().map_err(|_| ExcelError::ThreadGone)
    }
}

impl Drop for ExcelThread {
    fn drop(&mut self) {
        drop(self.tx.take());
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panicking_job_does_not_kill_the_thread() {
        let t = ExcelThread::spawn().unwrap();
        assert_eq!(t.run(|_| 1 + 1).unwrap(), 2);
        assert!(matches!(t.run(|_| -> i32 { panic!("작업 실패") }), Err(ExcelError::ThreadGone)));
        assert_eq!(t.run(|_| "다음 작업").unwrap(), "다음 작업");
    }
}
