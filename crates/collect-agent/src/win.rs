//! Windows 도우미: 한 번만 실행, 메시지 창, 클립보드, 절전 막기, 시작 시 실행(HKCU Run), 탐색기 열기.

use std::path::Path;

use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HGLOBAL};
use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::CF_UNICODETEXT;
use windows::Win32::System::Power::{ES_CONTINUOUS, ES_SYSTEM_REQUIRED, SetThreadExecutionState};
use windows::Win32::System::Threading::CreateMutexW;
use windows::core::HSTRING;

const TITLE: &str = "엑셀 취합 에이전트";
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE: &str = "merge-master-agent";

/// 프로세스가 끝날 때까지 쥐고 있는 단일 실행 표시.
pub struct SingleInstance(HANDLE);

impl Drop for SingleInstance {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// 같은 데이터 폴더로 이미 실행 중이면 `None`.
pub fn single_instance(data_dir: &Path) -> Option<SingleInstance> {
    // 데이터 폴더마다 하나(개발용 `--data-dir`은 따로 띄울 수 있게).
    let key = data_dir.to_string_lossy().to_lowercase();
    let hash = key.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3));
    let name = HSTRING::from(format!("Local\\merge-master-agent-{hash:016x}"));
    let handle = unsafe { CreateMutexW(None, true, &name) }.ok()?;
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        unsafe {
            let _ = CloseHandle(handle);
        }
        return None;
    }
    Some(SingleInstance(handle))
}

/// 알림 창(확인 단추 하나).
pub fn message(text: &str) {
    let _ = rfd::MessageDialog::new()
        .set_title(TITLE)
        .set_description(text)
        .set_level(rfd::MessageLevel::Info)
        .set_buttons(rfd::MessageButtons::Ok)
        .show();
}

/// 예/아니요를 묻는다.
pub fn confirm(text: &str) -> bool {
    rfd::MessageDialog::new()
        .set_title(TITLE)
        .set_description(text)
        .set_level(rfd::MessageLevel::Warning)
        .set_buttons(rfd::MessageButtons::YesNo)
        .show()
        == rfd::MessageDialogResult::Yes
}

/// 폴더를 고른다(지금 폴더에서 시작).
pub fn pick_folder(start: &Path) -> Option<std::path::PathBuf> {
    rfd::FileDialog::new().set_title("기본 저장 폴더 고르기").set_directory(start).pick_folder()
}

/// 글자를 클립보드에 넣는다.
pub fn copy_text(text: &str) -> bool {
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        if OpenClipboard(None).is_err() {
            return false;
        }
        let ok = (|| -> windows::core::Result<()> {
            EmptyClipboard()?;
            let mem: HGLOBAL = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2)?;
            let ptr = GlobalLock(mem) as *mut u16;
            if ptr.is_null() {
                return Err(windows::core::Error::from_thread());
            }
            std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr, wide.len());
            let _ = GlobalUnlock(mem);
            // 넣은 메모리는 클립보드가 가진다.
            SetClipboardData(u32::from(CF_UNICODETEXT.0), Some(HANDLE(mem.0)))?;
            Ok(())
        })()
        .is_ok();
        let _ = CloseClipboard();
        ok
    }
}

/// 서비스가 있는 동안 PC가 절전으로 들어가지 않게 한다(부른 스레드 기준 — 트레이 스레드에서 부른다).
pub fn keep_awake(on: bool) {
    let flags = if on { ES_CONTINUOUS | ES_SYSTEM_REQUIRED } else { ES_CONTINUOUS };
    unsafe {
        SetThreadExecutionState(flags);
    }
}

/// Windows에 로그인하면 이 실행 파일을 켠다(지금 사용자만).
pub fn autostart_enabled() -> bool {
    let hkcu = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER);
    hkcu.open_subkey(RUN_KEY).and_then(|k| k.get_value::<String, _>(RUN_VALUE)).is_ok()
}

pub fn set_autostart(on: bool) -> Result<(), String> {
    let hkcu = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER);
    let (key, _) = hkcu.create_subkey(RUN_KEY).map_err(|e| e.to_string())?;
    if on {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        key.set_value(RUN_VALUE, &format!("\"{}\"", exe.display())).map_err(|e| e.to_string())
    } else {
        match key.delete_value(RUN_VALUE) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
            _ => Ok(()),
        }
    }
}

/// 시작 시 실행이 켜져 있으면 지금 실행 파일 경로로 다시 쓴다(새 버전을 다른 이름으로 받아 켠 경우).
pub fn refresh_autostart() {
    if autostart_enabled()
        && let Err(e) = set_autostart(true)
    {
        tracing::warn!("시작 시 실행 경로를 고칠 수 없습니다: {e}");
    }
}

/// 탐색기로 폴더를 연다(파일이면 그 파일을 고른 채로).
pub fn reveal(path: &Path) {
    let mut cmd = std::process::Command::new("explorer.exe");
    if path.is_file() {
        cmd.arg(format!("/select,{}", path.display()));
    } else {
        cmd.arg(path);
    }
    if let Err(e) = cmd.spawn() {
        tracing::warn!("탐색기를 열 수 없습니다: {e}");
    }
}

/// 메모장 등 연결된 프로그램으로 연다(설정 파일).
pub fn open_file(path: &Path) {
    if let Err(e) = std::process::Command::new("notepad.exe").arg(path).spawn() {
        tracing::warn!("{}을(를) 열 수 없습니다: {e}", path.display());
    }
}
