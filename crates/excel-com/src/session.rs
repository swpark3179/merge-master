//! Excel 스레드가 소유하는 상태: 열거한 통합문서 핸들, 직접 띄운 백그라운드 인스턴스, 수식 계산 사본.

use std::collections::HashMap;
use std::path::Path;

use serde::Serialize;
use ts_rs::TS;
use windows::Win32::System::Com::{
    CLSCTX_LOCAL_SERVER, CLSIDFromProgID, CoCreateInstance, CreateBindCtx, GetRunningObjectTable, IDispatch,
};
use windows::Win32::System::Ole::GetActiveObject;
use windows_core::{IUnknown, w};

use crate::calc::CalcCopy;
use crate::dispatch::Dispatch;
use crate::error::{ExcelError, Result};
use crate::variant::Var;

const WORKBOOK_EXTS: [&str; 5] = [".xlsx", ".xlsm", ".xlsb", ".xls", ".csv"];

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct WorkbookInfo {
    /// 통합문서 식별자(`FullName`). 저장 안 된 문서는 `통합 문서1` 같은 이름이다.
    pub id: String,
    pub name: String,
    /// 저장 폴더. 저장 안 된 문서는 빈 문자열.
    pub path: String,
    pub sheets: Vec<String>,
    pub active_sheet: String,
    /// 앱이 '파일 열기'로 띄운 숨은 인스턴스에서 연 문서인지.
    pub background: bool,
}

/// [`Session::adopt_orphans`] 결과.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct OrphanReport {
    /// 닫은 옛 계산 사본 수.
    pub closed_calc: u32,
    /// 원본이 열린 숨은 Excel을 이 앱 것으로 받아들였다.
    pub adopted: bool,
    pub removed_files: u32,
}

/// 통합문서 목록과 함께, 사용자가 띄운 Excel이 실행 중인지(제목 줄 '● Excel 연결됨').
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct ExcelStatus {
    pub running: bool,
    pub workbooks: Vec<WorkbookInfo>,
}

#[derive(Default)]
pub struct Session {
    workbooks: HashMap<String, Dispatch>,
    /// '파일 열기'용으로 직접 띄운 Excel. 세션이 끝나면 닫는다.
    background_app: Option<Dispatch>,
    background_books: Vec<String>,
    /// 앞 앱이 남긴 숨은 Excel을 받아들였으면 그 프로세스(끝낼 때 확실히 끝낸다).
    adopted_pid: Option<u32>,
    /// 수식 재계산용 사본([`Session::open_calc`]).
    pub(crate) calc: Option<CalcCopy>,
}

impl Session {
    /// 실행 중인 Excel의 통합문서를 모두 나열한다.
    ///
    /// `GetActiveObject`는 ROT에 먼저 등록된 Excel 인스턴스 하나만 돌려주므로, 다른 인스턴스에 열린 파일은
    /// ROT의 파일 모니커를 훑어 보충한다(저장 안 된 문서는 모니커가 없어 첫 인스턴스 것만 보인다).
    /// 창이 모두 숨겨진 통합문서(개인용 매크로 통합문서 PERSONAL.XLSB 등)는 뺀다.
    pub fn list_workbooks(&mut self) -> Result<ExcelStatus> {
        let mut running_app = active_excel()?;
        let mut found: Vec<(Dispatch, bool)> = Vec::new();
        if let Some(app) = running_app.clone() {
            match collect_books(&app, &mut found, false) {
                // 방금 끝난(끝낸) Excel이 아직 '실행 중'으로 등록되어 있다.
                Err(ExcelError::Disconnected) => running_app = None,
                r => r?,
            }
        }
        if let Some(app) = self.live_background_app() {
            collect_books(&app, &mut found, true)?;
        }
        for book in rot_workbooks()? {
            found.push((book, false));
        }

        let mut infos = Vec::new();
        let mut handles = HashMap::new();
        for (book, background) in found {
            let id = match book.get_string("FullName") {
                Ok(id) => id,
                Err(ExcelError::Disconnected) => continue,
                Err(e) => return Err(e),
            };
            if handles.contains_key(&id) || !has_visible_window(&book) || self.is_calc_copy(&id) {
                continue;
            }
            infos.push(describe(&book, &id, background || self.background_books.contains(&id))?);
            handles.insert(id, book);
        }
        self.workbooks = handles;
        Ok(ExcelStatus { running: running_app.is_some(), workbooks: infos })
    }

    /// 파일을 연다. 이미 열려 있으면 그 문서를 쓰고, 실행 중인 Excel이 있으면 그 Excel에서(사용자에게 보임),
    /// 없으면 앱이 띄운 숨은 Excel 인스턴스에서 연다.
    pub fn open_file(&mut self, path: &Path) -> Result<WorkbookInfo> {
        let full = std::path::absolute(path).map_err(|e| ExcelError::Type(e.to_string()))?;
        let full = full.to_string_lossy().to_string();
        let status = self.list_workbooks()?;
        if let Some(info) = status.workbooks.iter().find(|b| b.id.to_lowercase() == full.to_lowercase()) {
            return Ok(info.clone());
        }
        let (app, background) = match active_excel()? {
            Some(app) => (app, false),
            None => (self.background_app()?, true),
        };
        let book = app
            .obj("Workbooks")?
            .obj_call("Open", vec![Var::str(&full), Var::i32(0) /* UpdateLinks: 안 함 */, Var::bool(false)])?;
        let id = book.get_string("FullName")?;
        if background {
            self.background_books.push(id.clone());
        }
        let info = describe(&book, &id, background)?;
        self.workbooks.insert(id, book);
        Ok(info)
    }

    /// 계산 사본은 ROT에 파일로 보이지만 목록에는 넣지 않는다.
    fn is_calc_copy(&self, id: &str) -> bool {
        self.calc.as_ref().is_some_and(|c| c.path.to_string_lossy().eq_ignore_ascii_case(id))
    }

    /// 살아 있는 백그라운드 인스턴스. 사용자가 작업 관리자로 끄는 등 연결이 끊겼으면 버린다.
    fn live_background_app(&mut self) -> Option<Dispatch> {
        let app = self.background_app.as_ref()?;
        if app.get("Version").is_ok() {
            return Some(app.clone());
        }
        self.background_app = None;
        self.background_books.clear();
        None
    }

    fn background_app(&mut self) -> Result<Dispatch> {
        if let Some(app) = self.live_background_app() {
            return Ok(app);
        }
        let app = create_excel()?;
        app.put("Visible", Var::bool(false))?;
        app.put("DisplayAlerts", Var::bool(false))?;
        self.background_app = Some(app.clone());
        Ok(app)
    }

    pub fn workbook(&self, id: &str) -> Result<&Dispatch> {
        self.workbooks.get(id).ok_or_else(|| ExcelError::WorkbookNotFound(id.to_string()))
    }

    pub fn worksheet(&self, workbook_id: &str, sheet: &str) -> Result<Dispatch> {
        self.workbook(workbook_id)?
            .obj("Worksheets")?
            .obj_call("Item", vec![Var::str(sheet)])
            .map_err(|_| ExcelError::SheetNotFound(sheet.to_string()))
    }

    /// 앞서 실행된 앱(강제 종료 등)이 남긴 숨은 Excel을 정리한다.
    ///
    /// - 계산 사본(임시 폴더 `merge-master\calc-*`)이 열려 있으면 닫고, 그 Excel에 다른 문서가 없으면 끝낸다.
    /// - `source`(원본 통합문서)가 보이지 않는 Excel에 열려 있으면 이 앱이 띄운 숨은 인스턴스로 받아들인다
    ///   (앱을 끝낼 때 함께 닫힌다).
    pub fn adopt_orphans(&mut self, source: Option<&str>) -> Result<OrphanReport> {
        let mut report = OrphanReport::default();
        let calc_dir = std::env::temp_dir().join("merge-master").to_string_lossy().to_lowercase();
        let own_calc = self.calc.as_ref().map(|c| c.path.to_string_lossy().to_lowercase());
        for book in rot_workbooks()? {
            let Ok(id) = book.get_string("FullName") else { continue };
            let lower = id.to_lowercase();
            let is_calc = lower.starts_with(&calc_dir) && lower.contains("calc-");
            if is_calc && own_calc.as_deref() != Some(lower.as_str()) {
                let app = book.obj("Application").ok();
                let pid = app.as_ref().and_then(excel_pid);
                let _ = book.call("Close", vec![Var::bool(false)]);
                if let Some(app) = app
                    && app.obj("Workbooks").and_then(|w| w.get("Count")).ok().and_then(|n| n.as_i32()) == Some(0)
                {
                    let _ = app.call("Quit", vec![]);
                    drop(app);
                    drop(book);
                    // 죽은 앱의 참조가 남아 Quit만으로는 끝나지 않는다.
                    if let Some(pid) = pid {
                        ensure_exit(pid, 3000);
                    }
                }
                let _ = std::fs::remove_file(&id);
                report.closed_calc += 1;
                continue;
            }
            if source.is_some_and(|s| s.eq_ignore_ascii_case(&id))
                && !self.background_books.contains(&id)
                && let Ok(app) = book.obj("Application")
                && app.get("Visible").ok().and_then(|v| v.as_bool()) == Some(false)
            {
                self.adopted_pid = excel_pid(&app);
                self.background_app = Some(app);
                self.background_books.push(id.clone());
                self.workbooks.insert(id, book);
                report.adopted = true;
            }
        }
        // 닫혀서 남은 옛 계산 사본 파일(잠겨 있으면 지우지 못하고 넘어간다).
        if let Ok(entries) = std::fs::read_dir(std::env::temp_dir().join("merge-master")) {
            let mine = format!("calc-{}-", std::process::id());
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if name.starts_with("calc-") && !name.starts_with(&mine) && std::fs::remove_file(e.path()).is_ok() {
                    report.removed_files += 1;
                }
            }
        }
        Ok(report)
    }

    /// 백그라운드로 연 문서를 저장하지 않고 닫고 인스턴스를 종료한다.
    pub fn close_background(&mut self) {
        for id in self.background_books.drain(..) {
            if let Some(book) = self.workbooks.remove(&id) {
                let _ = book.call("Close", vec![Var::bool(false)]);
            }
        }
        let adopted = self.adopted_pid.take();
        if let Some(app) = self.background_app.take() {
            // 받아들인 인스턴스에 남은 문서(앞 앱이 함께 열어 둔 파일 등)는 저장된 상태면 닫아도 잃는 것이 없다.
            // 저장하지 않은 변경이 있으면 다른 프로그램의 숨은 Excel일 수 있어 그대로 둔다.
            if adopted.is_some() {
                let mut left = Vec::new();
                let _ = collect_books(&app, &mut left, true);
                let unsaved =
                    left.iter().filter(|(b, _)| b.get("Saved").ok().and_then(|v| v.as_bool()) == Some(false)).count();
                if unsaved > 0 {
                    tracing::warn!(unsaved, "받아들인 Excel에 저장하지 않은 문서가 있어 끝내지 않습니다");
                    return;
                }
            }
            let _ = app.call("Quit", vec![]);
        }
        // 받아들인(앞 앱이 띄운) 인스턴스는 죽은 앱의 참조 때문에 Quit만으로 끝나지 않는다.
        if let Some(pid) = adopted {
            ensure_exit(pid, 3000);
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.close_calc();
        self.close_background();
    }
}

fn excel_clsid() -> Result<windows_core::GUID> {
    Ok(unsafe { CLSIDFromProgID(w!("Excel.Application"))? })
}

fn active_excel() -> Result<Option<Dispatch>> {
    let clsid = excel_clsid()?;
    let mut unk: Option<IUnknown> = None;
    match unsafe { GetActiveObject(&clsid, None, &mut unk) } {
        Ok(()) => match unk.map(|u| Dispatch::from_unknown(&u)).transpose() {
            // 끝난(끝낸) Excel이 아직 등록되어 있다.
            Err(ExcelError::Disconnected) => Ok(None),
            r => r,
        },
        Err(_) => Ok(None), // MK_E_UNAVAILABLE: 실행 중이 아님
    }
}

/// Excel 인스턴스의 프로세스 번호(주 창 핸들로 찾는다).
fn excel_pid(app: &Dispatch) -> Option<u32> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
    let hwnd = app.get("Hwnd").ok()?.as_f64()? as isize;
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(HWND(hwnd as *mut std::ffi::c_void), Some(&mut pid)) };
    (pid != 0).then_some(pid)
}

/// 이 앱이 띄운 Excel이 `wait_ms` 안에 끝나지 않으면 끝낸다.
fn ensure_exit(pid: u32, wait_ms: u32) {
    use windows::Win32::Foundation::{CloseHandle, WAIT_TIMEOUT};
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, TerminateProcess, WaitForSingleObject,
    };
    unsafe {
        let Ok(h) = OpenProcess(PROCESS_TERMINATE | PROCESS_SYNCHRONIZE, false, pid) else { return };
        if WaitForSingleObject(h, wait_ms) == WAIT_TIMEOUT {
            tracing::warn!(pid, "앞 앱이 남긴 Excel이 끝나지 않아 끝냅니다");
            let _ = TerminateProcess(h, 1);
        }
        let _ = CloseHandle(h);
    }
}

pub(crate) fn create_excel() -> Result<Dispatch> {
    let clsid = excel_clsid()?;
    let app: IDispatch = unsafe { CoCreateInstance(&clsid, None, CLSCTX_LOCAL_SERVER)? };
    Ok(Dispatch(app))
}

fn collect_books(app: &Dispatch, out: &mut Vec<(Dispatch, bool)>, background: bool) -> Result<()> {
    let books = app.obj("Workbooks")?;
    let n = books.get("Count")?.as_i32().unwrap_or(0);
    for i in 1..=n {
        out.push((books.obj_call("Item", vec![Var::i32(i)])?, background));
    }
    Ok(())
}

/// ROT에 등록된 파일 모니커 중 통합문서로 보이는 것을 `Workbook` 개체로 얻는다.
fn rot_workbooks() -> Result<Vec<Dispatch>> {
    let mut out = Vec::new();
    unsafe {
        let rot = GetRunningObjectTable(0)?;
        let ctx = CreateBindCtx(0)?;
        let monikers = rot.EnumRunning()?;
        loop {
            let mut slot = [None];
            let mut fetched = 0u32;
            if monikers.Next(&mut slot, Some(&mut fetched)).is_err() || fetched == 0 {
                break;
            }
            let Some(moniker) = slot[0].take() else { break };
            let Ok(name) = moniker.GetDisplayName(&ctx, None) else { continue };
            let display = name.to_string().unwrap_or_default();
            windows::Win32::System::Com::CoTaskMemFree(Some(name.0 as _));
            let lower = display.to_lowercase();
            if !WORKBOOK_EXTS.iter().any(|ext| lower.ends_with(ext)) {
                continue;
            }
            let Ok(unk) = rot.GetObject(&moniker) else { continue };
            let Ok(book) = Dispatch::from_unknown(&unk) else { continue };
            // 파일 모니커가 Excel 통합문서가 아닐 수도 있다(다른 앱이 연 파일). FullName이 있어야 통합문서로 본다.
            if book.get_string("FullName").is_ok() {
                out.push(book);
            }
        }
    }
    Ok(out)
}

fn has_visible_window(book: &Dispatch) -> bool {
    let Ok(windows) = book.obj("Windows") else { return true };
    let n = windows.get("Count").ok().and_then(|v| v.as_i32()).unwrap_or(0);
    (1..=n).any(|i| {
        let visible = windows.obj_call("Item", vec![Var::i32(i)]).and_then(|w| w.get("Visible"));
        visible.ok().and_then(|v| v.as_bool()) != Some(false)
    })
}

fn describe(book: &Dispatch, id: &str, background: bool) -> Result<WorkbookInfo> {
    let sheets_obj = book.obj("Worksheets")?;
    let n = sheets_obj.get("Count")?.as_i32().unwrap_or(0);
    let mut sheets = Vec::with_capacity(n as usize);
    for i in 1..=n {
        sheets.push(sheets_obj.obj_call("Item", vec![Var::i32(i)])?.get_string("Name")?);
    }
    let active_sheet = book.obj("ActiveSheet").and_then(|s| s.get_string("Name")).unwrap_or_default();
    Ok(WorkbookInfo {
        id: id.to_string(),
        name: book.get_string("Name")?,
        path: book.get_string("Path").unwrap_or_default(),
        sheets,
        active_sheet,
        background,
    })
}
