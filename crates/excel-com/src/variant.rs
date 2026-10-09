//! `VARIANT` 소유 래퍼와 2차원 `SAFEARRAY` 읽기.
//!
//! windows 0.62의 `VARIANT`는 원시 구조체라 스스로 해제되지 않는다. [`Var`]가 `Drop`에서
//! `VariantClear`를 불러 BSTR·인터페이스·배열을 정리한다.

use std::mem::ManuallyDrop;

use collect_core::CellValue;
use windows::Win32::Foundation::{DISP_E_PARAMNOTFOUND, VARIANT_FALSE, VARIANT_TRUE};
use windows::Win32::System::Com::{IDispatch, SAFEARRAY, SAFEARRAYBOUND};
use windows::Win32::System::Ole::{
    SafeArrayAccessData, SafeArrayCreate, SafeArrayDestroy, SafeArrayGetDim, SafeArrayGetLBound, SafeArrayGetUBound,
    SafeArrayUnaccessData,
};
use windows::Win32::System::Variant::{
    VAR_CHANGE_FLAGS, VARENUM, VARIANT, VARIANT_0_0_0, VT_ARRAY, VT_BOOL, VT_BSTR, VT_BYREF, VT_DISPATCH, VT_EMPTY,
    VT_ERROR, VT_I2, VT_I4, VT_NULL, VT_R4, VT_R8, VT_UNKNOWN, VT_VARIANT, VariantChangeType, VariantClear,
};
use windows_core::{BSTR, Interface};

use crate::error::{ExcelError, Result};

/// 소유권을 가진 `VARIANT`. `DISPPARAMS`에 배열째 넘기므로 메모리 배치가 `VARIANT`와 같아야 한다.
#[repr(transparent)]
pub struct Var(pub(crate) VARIANT);

impl Drop for Var {
    fn drop(&mut self) {
        unsafe {
            let _ = VariantClear(&mut self.0);
        }
    }
}

impl Default for Var {
    fn default() -> Self {
        Self::empty()
    }
}

impl std::fmt::Debug for Var {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Var(vt={}, {:?})", self.vt().0, to_cell_value(&self.0))
    }
}

impl Var {
    pub fn empty() -> Self {
        Var(VARIANT::default())
    }

    fn with(vt: VARENUM, set: impl FnOnce(&mut VARIANT_0_0_0)) -> Self {
        let mut v = VARIANT::default();
        unsafe {
            let inner = &mut *v.Anonymous.Anonymous;
            inner.vt = vt;
            set(&mut inner.Anonymous);
        }
        Var(v)
    }

    pub fn i32(x: i32) -> Self {
        Self::with(VT_I4, |u| u.lVal = x)
    }

    pub fn f64(x: f64) -> Self {
        Self::with(VT_R8, |u| u.dblVal = x)
    }

    pub fn bool(x: bool) -> Self {
        Self::with(VT_BOOL, |u| u.boolVal = if x { VARIANT_TRUE } else { VARIANT_FALSE })
    }

    pub fn str(s: &str) -> Self {
        Self::with(VT_BSTR, |u| u.bstrVal = ManuallyDrop::new(BSTR::from(s)))
    }

    /// `rows × cols` 2차원 배열(`Range.Value2`에 한 번에 쓴다). `cell(r, c)`로 칸마다 값을 만든다(0부터).
    pub fn array2d(rows: usize, cols: usize, mut cell: impl FnMut(usize, usize) -> Var) -> Result<Self> {
        if rows == 0 || cols == 0 {
            return Err(ExcelError::Type("빈 배열".into()));
        }
        // 첫 차원 = 행, 둘째 = 열. 데이터는 열 우선(첫 차원이 가장 빨리 바뀐다) — 읽을 때(`Grid`)와 같다.
        let bounds = [
            SAFEARRAYBOUND { cElements: rows as u32, lLbound: 1 },
            SAFEARRAYBOUND { cElements: cols as u32, lLbound: 1 },
        ];
        unsafe {
            let psa = SafeArrayCreate(VT_VARIANT, 2, bounds.as_ptr());
            if psa.is_null() {
                return Err(ExcelError::Type("배열을 만들 수 없습니다".into()));
            }
            let mut ptr = std::ptr::null_mut();
            if let Err(e) = SafeArrayAccessData(psa, &mut ptr) {
                let _ = SafeArrayDestroy(psa);
                return Err(e.into());
            }
            let data = ptr as *mut VARIANT;
            for c in 0..cols {
                for r in 0..rows {
                    // 소유권을 배열로 넘긴다(배열이 해제될 때 같이 정리된다).
                    let v = ManuallyDrop::new(cell(r, c));
                    std::ptr::write(data.add(c * rows + r), std::ptr::read(&v.0));
                }
            }
            SafeArrayUnaccessData(psa)?;
            Ok(Self::with(VARENUM(VT_ARRAY.0 | VT_VARIANT.0), |u| u.parray = psa))
        }
    }

    /// 생략된 선택 인수(`Type.Missing`).
    pub fn missing() -> Self {
        Self::with(VT_ERROR, |u| u.scode = DISP_E_PARAMNOTFOUND.0)
    }

    pub fn dispatch(d: &IDispatch) -> Self {
        Self::with(VT_DISPATCH, |u| u.pdispVal = ManuallyDrop::new(Some(d.clone())))
    }

    pub fn vt(&self) -> VARENUM {
        vt_of(&self.0)
    }

    /// 범위 속성이 셀마다 달라 Excel이 `Null`을 돌려준 경우.
    pub fn is_null(&self) -> bool {
        self.vt() == VT_NULL
    }

    pub fn is_empty(&self) -> bool {
        self.vt() == VT_EMPTY
    }

    pub fn as_f64(&self) -> Option<f64> {
        to_f64(&self.0)
    }

    pub fn as_i32(&self) -> Option<i32> {
        self.as_f64().map(|x| x as i32)
    }

    pub fn as_bool(&self) -> Option<bool> {
        to_bool(&self.0)
    }

    pub fn as_string(&self) -> Option<String> {
        to_string(&self.0)
    }

    pub fn as_cell_value(&self) -> CellValue {
        to_cell_value(&self.0)
    }

    pub fn as_dispatch(&self) -> Option<IDispatch> {
        unsafe {
            let inner = &*self.0.Anonymous.Anonymous;
            match inner.vt {
                VT_DISPATCH => (*inner.Anonymous.pdispVal).clone(),
                VT_UNKNOWN => (*inner.Anonymous.punkVal).as_ref().and_then(|u| u.cast().ok()),
                _ => None,
            }
        }
    }

    /// `Range.Value2`처럼 2차원 배열 또는 (단일 셀이면) 스칼라를 돌려주는 결과를 표 형태로 읽는다.
    pub fn into_grid(self) -> Result<Grid> {
        let vt = self.vt();
        if vt == VARENUM(VT_ARRAY.0 | VT_VARIANT.0) {
            let psa = unsafe { self.0.Anonymous.Anonymous.Anonymous.parray };
            Grid::from_safearray(self, psa)
        } else if vt.0 & VT_ARRAY.0 != 0 || vt.0 & VT_BYREF.0 != 0 {
            Err(ExcelError::Type(format!("지원하지 않는 배열 형식 vt={}", vt.0)))
        } else {
            Ok(Grid { rows: 1, cols: 1, data: GridData::Scalar(self) })
        }
    }
}

pub(crate) fn vt_of(v: &VARIANT) -> VARENUM {
    unsafe { v.Anonymous.Anonymous.vt }
}

pub(crate) fn to_f64(v: &VARIANT) -> Option<f64> {
    unsafe {
        let inner = &*v.Anonymous.Anonymous;
        match inner.vt {
            VT_R8 => Some(inner.Anonymous.dblVal),
            VT_R4 => Some(inner.Anonymous.fltVal as f64),
            VT_I4 => Some(inner.Anonymous.lVal as f64),
            VT_I2 => Some(inner.Anonymous.iVal as f64),
            VT_BOOL => Some(if inner.Anonymous.boolVal.0 != 0 { -1.0 } else { 0.0 }),
            VT_EMPTY | VT_NULL | VT_ERROR | VT_BSTR | VT_DISPATCH | VT_UNKNOWN => None,
            _ => {
                let mut out = VARIANT::default();
                VariantChangeType(&mut out, v, VAR_CHANGE_FLAGS(0), VT_R8).ok()?;
                let x = out.Anonymous.Anonymous.Anonymous.dblVal;
                let _ = VariantClear(&mut out);
                Some(x)
            }
        }
    }
}

pub(crate) fn to_bool(v: &VARIANT) -> Option<bool> {
    unsafe {
        let inner = &*v.Anonymous.Anonymous;
        match inner.vt {
            VT_BOOL => Some(inner.Anonymous.boolVal.0 != 0),
            _ => to_f64(v).map(|x| x != 0.0),
        }
    }
}

pub(crate) fn to_string(v: &VARIANT) -> Option<String> {
    unsafe {
        let inner = &*v.Anonymous.Anonymous;
        match inner.vt {
            VT_BSTR => Some((*inner.Anonymous.bstrVal).to_string()),
            VT_EMPTY | VT_NULL | VT_DISPATCH | VT_UNKNOWN => None,
            _ => {
                let mut out = VARIANT::default();
                VariantChangeType(&mut out, v, VAR_CHANGE_FLAGS(0), VT_BSTR).ok()?;
                let s = (*out.Anonymous.Anonymous.Anonymous.bstrVal).to_string();
                let _ = VariantClear(&mut out);
                Some(s)
            }
        }
    }
}

pub(crate) fn to_cell_value(v: &VARIANT) -> CellValue {
    unsafe {
        let inner = &*v.Anonymous.Anonymous;
        match inner.vt {
            VT_EMPTY | VT_NULL => CellValue::Empty,
            VT_BSTR => {
                let s = (*inner.Anonymous.bstrVal).to_string();
                if s.is_empty() { CellValue::Empty } else { CellValue::Text(s) }
            }
            VT_BOOL => CellValue::Bool(inner.Anonymous.boolVal.0 != 0),
            // Excel 오류 값은 0x800A07xx 형태로 온다(#N/A = 0x800A07FA → 2042).
            VT_ERROR => CellValue::Error { err: inner.Anonymous.scode & 0xFFFF },
            _ => to_f64(v).map_or(CellValue::Empty, CellValue::Number),
        }
    }
}

/// 2차원 결과. 인덱스는 0부터.
pub struct Grid {
    pub rows: usize,
    pub cols: usize,
    data: GridData,
}

enum GridData {
    Scalar(Var),
    /// `SafeArrayAccessData`로 잠근 배열. 메모리는 열 우선(첫 인덱스가 가장 빨리 변함)이다.
    Array {
        _owner: Var,
        psa: *mut SAFEARRAY,
        ptr: *const VARIANT,
    },
}

impl Grid {
    fn from_safearray(owner: Var, psa: *mut SAFEARRAY) -> Result<Self> {
        unsafe {
            if psa.is_null() || SafeArrayGetDim(psa) != 2 {
                return Err(ExcelError::Type("2차원 배열이 아닙니다".into()));
            }
            let rows = (SafeArrayGetUBound(psa, 1)? - SafeArrayGetLBound(psa, 1)? + 1) as usize;
            let cols = (SafeArrayGetUBound(psa, 2)? - SafeArrayGetLBound(psa, 2)? + 1) as usize;
            let mut ptr = std::ptr::null_mut();
            SafeArrayAccessData(psa, &mut ptr)?;
            Ok(Grid { rows, cols, data: GridData::Array { _owner: owner, psa, ptr: ptr as *const VARIANT } })
        }
    }

    pub fn get(&self, row: usize, col: usize) -> &VARIANT {
        assert!(row < self.rows && col < self.cols, "Grid 인덱스 범위 밖: ({row}, {col})");
        match &self.data {
            GridData::Scalar(v) => &v.0,
            GridData::Array { ptr, .. } => unsafe { &*ptr.add(col * self.rows + row) },
        }
    }

    pub fn value(&self, row: usize, col: usize) -> CellValue {
        to_cell_value(self.get(row, col))
    }

    pub fn string(&self, row: usize, col: usize) -> Option<String> {
        to_string(self.get(row, col))
    }
}

impl Drop for Grid {
    fn drop(&mut self) {
        if let GridData::Array { psa, .. } = &self.data {
            unsafe {
                let _ = SafeArrayUnaccessData(*psa);
            }
        }
        // 이후 `_owner`(Var)가 drop되며 VariantClear → SafeArrayDestroy.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalar_round_trip() {
        assert_eq!(Var::i32(7).as_f64(), Some(7.0));
        assert_eq!(Var::f64(1.5).as_cell_value(), CellValue::Number(1.5));
        assert_eq!(Var::str("예산").as_string().as_deref(), Some("예산"));
        assert_eq!(Var::str("").as_cell_value(), CellValue::Empty);
        assert_eq!(Var::bool(true).as_bool(), Some(true));
        assert!(Var::empty().is_empty());
        // 숫자 → 문자열 변환은 VariantChangeType을 거친다.
        assert_eq!(Var::i32(42).as_string().as_deref(), Some("42"));
    }

    #[test]
    fn array2d_round_trips_through_grid() {
        let v = Var::array2d(2, 3, |r, c| Var::f64((r * 10 + c) as f64)).unwrap();
        let g = v.into_grid().unwrap();
        assert_eq!((g.rows, g.cols), (2, 3));
        assert_eq!(g.value(1, 2), CellValue::Number(12.0));
        assert_eq!(g.value(0, 1), CellValue::Number(1.0));
        let t = Var::array2d(1, 1, |_, _| Var::str("가")).unwrap().into_grid().unwrap();
        assert_eq!(t.value(0, 0), CellValue::Text("가".into()));
        assert!(Var::array2d(0, 1, |_, _| Var::empty()).is_err());
    }

    #[test]
    fn scalar_grid_is_one_by_one() {
        let g = Var::f64(3.0).into_grid().unwrap();
        assert_eq!((g.rows, g.cols), (1, 1));
        assert_eq!(g.value(0, 0), CellValue::Number(3.0));
    }
}
