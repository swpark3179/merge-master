//! Excel로 저장(Windows). 에이전트 프로세스는 Excel을 띄우지 않는다: 마감 저장은 같은 실행 파일을 별도 저장 프로세스
//! ([`WORKER_ARG`])로 띄워 그 안의 숨은 Excel에서 한다. Excel·COM이 멈추거나 죽어도 에이전트(참여자 서버·다른 서비스)는
//! 그대로이고, 10분이 넘으면 저장 프로세스와 그 Excel을 끝내고 실패로 친다. 저장은 한 번에 하나씩(여러 서비스의 마감이
//! 겹쳐도 Excel은 하나만 뜬다). 이 PC 사용자의 Excel에는 열지 않는다.
//!
//! 저장 프로세스는 표준 출력에 한 줄 JSON([`WorkerMsg`])으로 알린다: 띄운 Excel의 프로세스 번호, 끝나면 결과.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use agent_protocol::save_data::SaveData;
use excel_com::ExcelThread;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::{Semaphore, watch};

use crate::engine::{Engine, EngineFuture, SaveJob};

/// 저장 프로세스로 띄울 때의 첫 인자(`--save-worker <취합 데이터> <원본> <저장할 파일>`).
pub const WORKER_ARG: &str = "--save-worker";
/// 저장 하나를 이만큼 기다린다(넘으면 끝낸다 — Excel이 대화 상자 등으로 멈춘 경우).
const EXCEL_TIMEOUT: Duration = Duration::from_secs(600);
/// 끝낼 때 저장 프로세스가 정리되기를 기다리는 시간.
const SHUTDOWN_WAIT: Duration = Duration::from_secs(10);
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 저장 프로세스 → 에이전트(표준 출력 한 줄).
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum WorkerMsg {
    /// 저장에 쓸 숨은 Excel을 띄웠다.
    Excel {
        pid: u32,
    },
    Saved {
        path: String,
        mismatched: usize,
    },
    Failed {
        message: String,
    },
}

pub struct ExcelEngine {
    installed: bool,
    exe: PathBuf,
    /// 끝낼 때 `true`(하고 있는 저장을 그만둔다).
    stop: watch::Sender<bool>,
    /// 하고 있는(기다리는 것 포함) 저장 수.
    active: Arc<watch::Sender<usize>>,
    /// 저장 프로세스는 한 번에 하나.
    slot: Arc<Semaphore>,
}

impl ExcelEngine {
    pub fn new() -> Result<Self, String> {
        let exe = std::env::current_exe().map_err(|e| format!("실행 파일 경로를 알 수 없습니다: {e}"))?;
        let installed = excel_installed();
        if !installed {
            tracing::warn!("Excel이 설치되어 있지 않습니다: 마감 때 취합 데이터만 남기고 관리자 PC에서 저장합니다");
        }
        Ok(Self {
            installed,
            exe,
            stop: watch::channel(false).0,
            active: Arc::new(watch::channel(0).0),
            slot: Arc::new(Semaphore::new(1)),
        })
    }
}

/// Excel이 설치되어 있는가(등록만 본다 — Excel을 띄우지 않는다).
pub fn excel_installed() -> bool {
    winreg::RegKey::predef(winreg::enums::HKEY_CLASSES_ROOT).open_subkey(r"Excel.Application\CLSID").is_ok()
}

impl Engine for ExcelEngine {
    fn excel_available(&self) -> bool {
        self.installed
    }

    fn save(&self, job: SaveJob) -> EngineFuture<Result<String, String>> {
        let (exe, mut stop, active, slot) =
            (self.exe.clone(), self.stop.subscribe(), self.active.clone(), self.slot.clone());
        Box::pin(async move {
            active.send_modify(|n| *n += 1);
            let permit = tokio::select! {
                p = slot.acquire_owned() => p.ok(),
                () = async { let _ = stop.wait_for(|s| *s).await; } => None,
            };
            let out = match permit {
                Some(_permit) => run_child(&exe, &job, &mut stop).await,
                None => Err("에이전트를 끝내 저장을 그만뒀습니다".into()),
            };
            active.send_modify(|n| *n -= 1);
            out
        })
    }

    fn shutdown(&self) -> EngineFuture<()> {
        self.stop.send_replace(true);
        let mut active = self.active.subscribe();
        Box::pin(async move {
            let _ = tokio::time::timeout(SHUTDOWN_WAIT, active.wait_for(|n| *n == 0)).await;
        })
    }
}

/// 저장 프로세스를 띄워 결과를 기다린다. 시간이 넘거나 에이전트를 끝내면 그 프로세스와 Excel을 끝낸다.
async fn run_child(exe: &Path, job: &SaveJob, stop: &mut watch::Receiver<bool>) -> Result<String, String> {
    let mut child = tokio::process::Command::new(exe)
        .arg(WORKER_ARG)
        .arg(&job.data)
        .arg(&job.source)
        .arg(&job.out)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("저장 프로세스를 시작할 수 없습니다: {e}"))?;
    if let Some(err) = child.stderr.take() {
        tokio::spawn(async move {
            let mut lines = BufReader::new(err).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::info!(target: "save-worker", "{line}");
            }
        });
    }
    let stdout = child.stdout.take().ok_or("저장 프로세스의 출력을 읽을 수 없습니다")?;
    let excel = Arc::new(AtomicU32::new(0));
    let read = read_outcome(stdout, excel.clone());
    let outcome = tokio::select! {
        r = tokio::time::timeout(EXCEL_TIMEOUT, read) => r.ok(),
        () = async { let _ = stop.wait_for(|s| *s).await; } => None,
    };
    match outcome {
        Some(Some(result)) => {
            let _ = child.wait().await;
            result
        }
        // 결과 없이 끝났다(죽음) 또는 시간이 넘었다·에이전트를 끝낸다: 프로세스와 그 Excel을 끝낸다.
        other => {
            let _ = child.kill().await;
            kill_excel(excel.load(Ordering::SeqCst));
            let _ = std::fs::remove_file(&job.out);
            Err(match other {
                Some(None) => {
                    let status = child.wait().await.map(|s| s.to_string()).unwrap_or_default();
                    format!("저장 프로세스가 비정상으로 끝났습니다({status})")
                }
                _ if *stop.borrow() => "에이전트를 끝내 저장을 그만뒀습니다".into(),
                _ => "Excel 작업이 10분 안에 끝나지 않아 그만뒀습니다(Excel에 열린 대화 상자가 있는지 확인해 주세요)"
                    .into(),
            })
        }
    }
}

/// 출력이 닫힐 때까지 읽는다. 결과 줄이 없으면 `None`.
async fn read_outcome(stdout: tokio::process::ChildStdout, excel: Arc<AtomicU32>) -> Option<Result<String, String>> {
    let mut lines = BufReader::new(stdout).lines();
    let mut outcome = None;
    while let Ok(Some(line)) = lines.next_line().await {
        match serde_json::from_str::<WorkerMsg>(&line) {
            Ok(WorkerMsg::Excel { pid }) => excel.store(pid, Ordering::SeqCst),
            Ok(WorkerMsg::Saved { path, mismatched }) => {
                if mismatched > 0 {
                    tracing::warn!("저장 뒤 다시 읽은 값이 다른 칸 {mismatched}개: {path}");
                }
                outcome = Some(Ok(path));
            }
            Ok(WorkerMsg::Failed { message }) => outcome = Some(Err(message)),
            Err(_) => tracing::info!(target: "save-worker", "{line}"),
        }
    }
    outcome
}

/// 저장 프로세스가 띄운 Excel을 끝낸다(그 번호가 지금도 EXCEL.EXE일 때만 — 번호가 다른 프로그램에 다시 쓰였을 수 있다).
fn kill_excel(pid: u32) {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
        QueryFullProcessImageNameW, TerminateProcess,
    };
    use windows::core::PWSTR;
    if pid == 0 {
        return;
    }
    unsafe {
        let Ok(h) = OpenProcess(PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return };
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let name = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len)
            .map(|()| String::from_utf16_lossy(&buf[..len as usize]))
            .unwrap_or_default();
        if name.to_ascii_lowercase().ends_with("\\excel.exe") {
            tracing::warn!(pid, "저장 프로세스가 띄운 Excel을 끝냅니다");
            let _ = TerminateProcess(h, 1);
        }
        let _ = CloseHandle(h);
    }
}

/// 저장 프로세스(`merge-master-agent.exe --save-worker <취합 데이터> <원본> <저장할 파일>`): 숨은 Excel로 저장하고 결과를
/// 표준 출력에 알린다. 돌려준 값은 프로세스 종료 코드.
pub fn run_worker(data: &Path, source: &Path, out: &Path) -> i32 {
    let _ = tracing_subscriber::fmt().with_writer(std::io::stderr).with_ansi(false).try_init();
    let say = |msg: &WorkerMsg| {
        use std::io::Write;
        let mut o = std::io::stdout().lock();
        let _ = writeln!(o, "{}", serde_json::to_string(msg).expect("직렬화"));
        let _ = o.flush();
    };
    let result = (|| -> Result<excel_com::SaveReport, String> {
        let bytes = std::fs::read(data).map_err(|e| format!("취합 데이터를 읽을 수 없습니다: {e}"))?;
        let plans = SaveData::decode(&bytes)?.plan();
        let excel = ExcelThread::spawn().map_err(|e| format!("Excel 스레드를 시작할 수 없습니다: {e}"))?;
        let (source, out) = (source.to_path_buf(), out.to_path_buf());
        excel
            .run(move |s| s.save_file_copy_multi_with(&source, &plans, &out, &mut |pid| say(&WorkerMsg::Excel { pid })))
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())
    })();
    match result {
        Ok(report) => {
            say(&WorkerMsg::Saved { path: report.path, mismatched: report.mismatched.len() });
            0
        }
        Err(message) => {
            say(&WorkerMsg::Failed { message });
            1
        }
    }
}
