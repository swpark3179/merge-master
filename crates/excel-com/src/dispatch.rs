//! `IDispatch` 레이트 바인딩 래퍼.
//!
//! Excel은 프로세스 밖 서버라 호출 한 번이 곧 프로세스 간 왕복이다. 그래서
//! - 멤버 이름 → DISPID 조회 결과를 스레드 로컬에 캐시하고(Excel 개체 모델은 같은 이름에 같은 DISPID를 쓴다),
//! - 호출 횟수를 세어 [`com_calls`]로 노출해 읽기 전략의 비용을 잴 수 있게 한다.
//!
//! `EXCEL_COM_VERIFY_DISPIDS=1`이면 캐시를 쓰더라도 매번 다시 조회해 같은지 검사한다.

use std::cell::RefCell;
use std::collections::HashMap;
use std::mem::ManuallyDrop;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{DISP_E_EXCEPTION, RPC_E_CALL_REJECTED, RPC_E_SERVERCALL_RETRYLATER};
use windows::Win32::System::Com::{
    DISPATCH_FLAGS, DISPATCH_METHOD, DISPATCH_PROPERTYGET, DISPATCH_PROPERTYPUT, DISPPARAMS, EXCEPINFO, IDispatch,
};
use windows::Win32::System::Ole::DISPID_PROPERTYPUT;
use windows_core::{GUID, HSTRING, IUnknown, Interface, PCWSTR};

use crate::error::{ExcelError, Result};
use crate::variant::Var;

const LOCALE_USER_DEFAULT: u32 = 0x0400;
/// Excel이 바쁠 때(셀 편집 중, 모달 대화상자) 재시도할 최대 시간.
const BUSY_TIMEOUT: Duration = Duration::from_secs(8);

static COM_CALLS: AtomicU64 = AtomicU64::new(0);

/// 지금까지 실행한 Excel 호출 수(이름 조회 포함).
pub fn com_calls() -> u64 {
    COM_CALLS.load(Ordering::Relaxed)
}

thread_local! {
    static DISPIDS: RefCell<HashMap<String, i32>> = RefCell::new(HashMap::new());
    static VERIFY: bool = std::env::var_os("EXCEL_COM_VERIFY_DISPIDS").is_some();
}

#[derive(Clone)]
pub struct Dispatch(pub(crate) IDispatch);

impl std::fmt::Debug for Dispatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Dispatch({:p})", self.0.as_raw())
    }
}

impl Dispatch {
    pub fn from_unknown(u: &IUnknown) -> Result<Self> {
        u.cast().map(Self).map_err(|e| classify("QueryInterface", e.code().0 as u32, e.message()))
    }

    pub fn raw(&self) -> &IDispatch {
        &self.0
    }

    fn lookup(&self, name: &str) -> Result<i32> {
        let wide = HSTRING::from(name);
        let names = [PCWSTR(wide.as_ptr())];
        let mut id = 0i32;
        COM_CALLS.fetch_add(1, Ordering::Relaxed);
        unsafe { self.0.GetIDsOfNames(&GUID::zeroed(), names.as_ptr(), 1, LOCALE_USER_DEFAULT, &mut id) }
            .map_err(|e| classify(name, e.code().0 as u32, e.message()))?;
        Ok(id)
    }

    fn dispid(&self, name: &str) -> Result<i32> {
        let cached = DISPIDS.with(|m| m.borrow().get(name).copied());
        match cached {
            Some(id) if !VERIFY.with(|v| *v) => Ok(id),
            Some(id) => {
                let fresh = self.lookup(name)?;
                assert_eq!(id, fresh, "DISPID 캐시 불일치: {name}");
                Ok(id)
            }
            None => {
                let id = self.lookup(name)?;
                DISPIDS.with(|m| m.borrow_mut().insert(name.to_string(), id));
                Ok(id)
            }
        }
    }

    /// `args`는 Excel 문서의 인수 순서 그대로 넘긴다(내부에서 뒤집는다).
    fn invoke(&self, name: &str, flags: DISPATCH_FLAGS, mut args: Vec<Var>) -> Result<Var> {
        let id = self.dispid(name)?;
        args.reverse();
        let mut named = DISPID_PROPERTYPUT;
        let is_put = flags == DISPATCH_PROPERTYPUT;
        let params = DISPPARAMS {
            rgvarg: if args.is_empty() { std::ptr::null_mut() } else { args.as_mut_ptr().cast() },
            rgdispidNamedArgs: if is_put { &mut named } else { std::ptr::null_mut() },
            cArgs: args.len() as u32,
            cNamedArgs: u32::from(is_put),
        };
        let started = Instant::now();
        loop {
            let mut result = Var::empty();
            let mut excep = EXCEPINFO::default();
            COM_CALLS.fetch_add(1, Ordering::Relaxed);
            let r = unsafe {
                self.0.Invoke(
                    id,
                    &GUID::zeroed(),
                    LOCALE_USER_DEFAULT,
                    flags,
                    &params,
                    Some(&mut result.0),
                    Some(&mut excep),
                    None,
                )
            };
            match r {
                Ok(()) => return Ok(result),
                Err(e) if e.code() == RPC_E_CALL_REJECTED || e.code() == RPC_E_SERVERCALL_RETRYLATER => {
                    if started.elapsed() > BUSY_TIMEOUT {
                        return Err(ExcelError::Busy);
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(e) => {
                    let (hresult, message) = if e.code() == DISP_E_EXCEPTION {
                        take_excepinfo(&mut excep, e.code().0)
                    } else {
                        (e.code().0 as u32, e.message())
                    };
                    return Err(classify(name, hresult, message));
                }
            }
        }
    }

    /// 인수 없는 속성 읽기.
    pub fn get(&self, name: &str) -> Result<Var> {
        self.invoke(name, DISPATCH_PROPERTYGET, Vec::new())
    }

    /// 인수 있는 속성 읽기 또는 메서드 호출(`Range("A1")`, `Item(1)`, `Borders(7)` …).
    pub fn call(&self, name: &str, args: Vec<Var>) -> Result<Var> {
        self.invoke(name, DISPATCH_FLAGS(DISPATCH_METHOD.0 | DISPATCH_PROPERTYGET.0), args)
    }

    pub fn put(&self, name: &str, value: Var) -> Result<()> {
        self.invoke(name, DISPATCH_PROPERTYPUT, vec![value]).map(drop)
    }

    pub fn obj(&self, name: &str) -> Result<Dispatch> {
        to_obj(name, self.get(name)?)
    }

    pub fn obj_call(&self, name: &str, args: Vec<Var>) -> Result<Dispatch> {
        to_obj(name, self.call(name, args)?)
    }

    pub fn get_string(&self, name: &str) -> Result<String> {
        let v = self.get(name)?;
        v.as_string().ok_or_else(|| ExcelError::Type(format!("{name}: 문자열이 아님 (vt={})", v.vt().0)))
    }
}

/// Excel이 닫혔거나(프로세스 종료) 개체가 끊긴 경우를 따로 구분한다.
const DISCONNECTED: [u32; 4] = [
    0x8001_0108, // RPC_E_DISCONNECTED
    0x8007_06BA, // RPC_S_SERVER_UNAVAILABLE
    0x8007_06BE, // RPC_S_CALL_FAILED
    0x8004_01FD, // CO_E_OBJNOTCONNECTED
];

fn classify(member: &str, hresult: u32, message: String) -> ExcelError {
    if DISCONNECTED.contains(&hresult) {
        ExcelError::Disconnected
    } else {
        ExcelError::Call { member: member.to_string(), hresult, message }
    }
}

fn to_obj(name: &str, v: Var) -> Result<Dispatch> {
    v.as_dispatch().map(Dispatch).ok_or_else(|| ExcelError::Type(format!("{name}: 개체가 아님 (vt={})", v.vt().0)))
}

fn take_excepinfo(excep: &mut EXCEPINFO, fallback: i32) -> (u32, String) {
    unsafe {
        if let Some(fill) = excep.pfnDeferredFillIn {
            let _ = fill(excep);
        }
        let description = ManuallyDrop::take(&mut excep.bstrDescription).to_string();
        drop(ManuallyDrop::take(&mut excep.bstrSource));
        drop(ManuallyDrop::take(&mut excep.bstrHelpFile));
        let code = if excep.scode != 0 { excep.scode } else { fallback };
        (code as u32, if description.is_empty() { "Excel 예외".into() } else { description })
    }
}
