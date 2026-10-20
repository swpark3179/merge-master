//! 수식 재계산용 계산 사본(4단계).
//!
//! 원본 통합문서는 최종 저장 전까지 건드리지 않는다. 대신 확정한 시점의 원본을 `SaveCopyAs`로 임시 파일에 복사하고,
//! **별도의 숨은 Excel 인스턴스**에서 열어 계산 엔진으로 쓴다. 참여자 입력을 사본에 쓰고 다시 계산한 뒤
//! 수식 칸의 결과만 읽어 돌려준다.
//!
//! 좌표는 모두 [`SheetModel`](collect_core::SheetModel) 기준(불러온 범위 왼쪽 위 = 0,0)이고, 원본 시트 위치(`origin`)는
//! 여기서 더한다.
//!
//! 호스트가 여러 업무를 함께 진행하면 업무마다 사본이 하나씩 있다(키 = 업무 id, `*_for` 메서드). 키 없는 옛 메서드는
//! 키 `""` 사본 하나를 다룬다.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use collect_core::{CellRange, CellRef, CellValue, RowCut};

use crate::dispatch::Dispatch;
use crate::error::{ExcelError, Result};
use crate::session::{Session, create_excel};
use crate::variant::Var;

pub(crate) const XL_CALCULATION_MANUAL: i32 = -4135;
/// `msoAutomationSecurityForceDisable`: 사본을 열 때 매크로를 실행하지 않는다.
pub(crate) const MSO_AUTOMATION_SECURITY_FORCE_DISABLE: i32 = 3;

static NEXT_COPY: AtomicU32 = AtomicU32::new(1);

/// 계산 사본의 시트 하나: 불러온 범위의 시트 위치와 결과를 읽을 수식 칸.
struct CalcSheet {
    sheet: Dispatch,
    origin: CellRef,
    /// 수식 칸(모델 좌표)과 그 칸들을 덮는 최소 직사각형(한 번에 읽기).
    formulas: Vec<CellRef>,
    bounds: Option<CellRange>,
}

impl CalcSheet {
    fn address(&self, at: CellRef) -> String {
        CellRef::new(at.row + self.origin.row, at.col + self.origin.col).to_string()
    }
}

pub(crate) struct CalcCopy {
    app: Dispatch,
    book: Dispatch,
    /// 세션 안 시트 순번 순서(수식이 없는 시트도 — 다른 시트 수식이 그 칸을 볼 수 있다).
    sheets: Vec<CalcSheet>,
    pub(crate) path: PathBuf,
}

impl CalcCopy {
    fn close(self) {
        let _ = self.book.call("Close", vec![Var::bool(false)]);
        let _ = self.app.call("Quit", vec![]);
        drop((self.sheets, self.book, self.app));
        let _ = std::fs::remove_file(&self.path);
    }
}

/// 계산 사본에 열 시트 하나(세션 안 시트 순번 순서로 넘긴다).
#[derive(Debug, Clone, Default)]
pub struct CalcTarget {
    pub sheet: String,
    /// 불러온 범위의 시트 위치.
    pub origin: CellRef,
    /// 결과를 읽어 올 수식 칸(모델 좌표 — `cuts`를 적용한 뒤의 좌표).
    pub formulas: Vec<CellRef>,
    /// 사본을 연 뒤 이 순서로 지울 행(앱에서 지운 불러온 행 — 그 순간의 모델 행 좌표).
    pub cuts: Vec<RowCut>,
    /// 숨김을 풀 행·열(원본에서 숨겨져 있던 줄 — 저장할 파일과 같게 `SUBTOTAL` 등을 계산한다).
    pub unhide_rows: Vec<u32>,
    pub unhide_cols: Vec<u32>,
}

impl CalcSheet {
    /// 사본에서 행 `cut`을 지우고(아래 행이 올라온다) 수식 칸 좌표를 맞춘다.
    fn cut_rows(&mut self, cut: RowCut) -> Result<()> {
        if cut.count == 0 {
            return Ok(());
        }
        let first = self.address(CellRef::new(cut.from, 0));
        let last = self.address(CellRef::new(cut.from + cut.count - 1, 0));
        self.sheet
            .obj_call("Range", vec![Var::str(&format!("{first}:{last}"))])?
            .obj("EntireRow")?
            .call("Delete", vec![])?;
        let end = cut.from + cut.count;
        self.formulas = self
            .formulas
            .iter()
            .filter(|at| !(cut.from..end).contains(&at.row))
            .map(|at| if at.row >= end { CellRef::new(at.row - cut.count, at.col) } else { *at })
            .collect();
        self.bounds = bounding(&self.formulas);
        Ok(())
    }

    /// 사본에서 숨긴 줄을 편다.
    fn unhide(&self, rows: &[u32], cols: &[u32]) -> Result<()> {
        for &r in rows {
            let at = self.address(CellRef::new(r, 0));
            self.sheet.obj_call("Range", vec![Var::str(&at)])?.obj("EntireRow")?.put("Hidden", Var::bool(false))?;
        }
        for &c in cols {
            let at = self.address(CellRef::new(0, c));
            self.sheet.obj_call("Range", vec![Var::str(&at)])?.obj("EntireColumn")?.put("Hidden", Var::bool(false))?;
        }
        Ok(())
    }
}

/// 셀에 쓸 값. 글자는 앞에 `'`를 붙여 Excel이 숫자·날짜·수식으로 바꾸지 않게 한다.
pub(crate) fn to_var(v: &CellValue) -> Var {
    match v {
        CellValue::Empty => Var::empty(),
        CellValue::Number(n) => Var::f64(*n),
        CellValue::Text(s) if s.is_empty() => Var::empty(),
        CellValue::Text(s) => Var::str(&format!("'{s}")),
        CellValue::Bool(b) => Var::bool(*b),
        CellValue::Error { .. } => Var::empty(),
    }
}

fn bounding(cells: &[CellRef]) -> Option<CellRange> {
    let first = cells.first()?;
    let (mut r0, mut c0, mut r1, mut c1) = (first.row, first.col, first.row, first.col);
    for at in cells {
        r0 = r0.min(at.row);
        c0 = c0.min(at.col);
        r1 = r1.max(at.row);
        c1 = c1.max(at.col);
    }
    Some(CellRange::new(r0, c0, r1 - r0 + 1, c1 - c0 + 1))
}

/// 계산 사본을 연 결과.
#[derive(Debug, Clone)]
pub struct CalcInfo {
    pub path: PathBuf,
    pub formulas: usize,
}

/// 키 없는 옛 메서드가 쓰는 사본의 키.
const DEFAULT_KEY: &str = "";

impl Session {
    /// 통합문서의 현재 상태를 복사해 숨은 Excel에서 연다(한 시트). [`Session::open_calc_multi`] 참고.
    pub fn open_calc(
        &mut self,
        workbook_id: &str,
        sheet: &str,
        origin: CellRef,
        formulas: Vec<CellRef>,
    ) -> Result<CalcInfo> {
        let target = CalcTarget { sheet: sheet.to_string(), origin, formulas, ..Default::default() };
        self.open_calc_multi(workbook_id, vec![target])
    }

    /// 통합문서의 현재 상태를 복사해 숨은 Excel에서 연다. 이미 열린 사본은 닫는다. [`Session::open_calc_for`] 참고.
    pub fn open_calc_multi(&mut self, workbook_id: &str, targets: Vec<CalcTarget>) -> Result<CalcInfo> {
        self.open_calc_for(DEFAULT_KEY, workbook_id, targets)
    }

    /// 업무(`key`)의 계산 사본을 연다: 통합문서의 현재 상태를 복사해 숨은 Excel에서 연다. 그 업무의 앞 사본만 닫는다
    /// (다른 업무의 사본은 그대로). 사본은 통합문서 전체라 시트를 넘는 수식도 계산된다. `targets`는 세션 안 시트 순번 순서.
    pub fn open_calc_for(&mut self, key: &str, workbook_id: &str, targets: Vec<CalcTarget>) -> Result<CalcInfo> {
        self.close_calc_for(key);
        let source = self.workbook(workbook_id)?;
        let name = source.get_string("Name")?;
        let path = self.next_calc_path(&name)?;
        source.call("SaveCopyAs", vec![Var::str(&path.to_string_lossy())])?;
        self.open_copy(key, path, targets)
    }

    /// 업무(`key`)의 계산 사본을 원본 **파일**에서 연다(에이전트 PC에 넘겨받은 사본). 파일을 작업 폴더로 복사해 숨은
    /// Excel에서 연다 — 실행 중인 Excel(그 PC 사용자의 Excel)에는 열지 않는다.
    pub fn open_calc_file_for(&mut self, key: &str, source: &Path, targets: Vec<CalcTarget>) -> Result<CalcInfo> {
        self.close_calc_for(key);
        let path = self.next_calc_path(&source.to_string_lossy())?;
        std::fs::copy(source, &path).map_err(|e| ExcelError::Type(format!("원본을 복사할 수 없습니다: {e}")))?;
        self.open_copy(key, path, targets)
    }

    /// 다음 계산 사본 파일 이름(작업 폴더의 `calc-<프로세스>-<번호>.<원본 확장자>`).
    fn next_calc_path(&self, name: &str) -> Result<PathBuf> {
        let ext = Path::new(name)
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .filter(|e| ["xlsx", "xlsm", "xlsb", "xls"].contains(&e.as_str()))
            .unwrap_or_else(|| "xlsx".into());
        let dir = self.calc_dir();
        std::fs::create_dir_all(&dir).map_err(|e| ExcelError::Type(format!("임시 폴더: {e}")))?;
        let n = NEXT_COPY.fetch_add(1, Ordering::Relaxed);
        Ok(dir.join(format!("calc-{}-{n}.{ext}", std::process::id())))
    }

    /// 복사해 둔 사본 파일(`path`)을 숨은 Excel에서 열어 업무(`key`)의 계산 사본으로 둔다. 실패하면 그 파일을 지운다.
    fn open_copy(&mut self, key: &str, path: PathBuf, targets: Vec<CalcTarget>) -> Result<CalcInfo> {
        let full = path.to_string_lossy().to_string();
        let opened = (|| -> Result<CalcCopy> {
            let app = create_excel()?;
            app.put("Visible", Var::bool(false))?;
            app.put("DisplayAlerts", Var::bool(false))?;
            app.put("ScreenUpdating", Var::bool(false))?;
            let _ = app.put("AutomationSecurity", Var::i32(MSO_AUTOMATION_SECURITY_FORCE_DISABLE));
            let _ = app.put("EnableEvents", Var::bool(false));
            let open = app
                .obj("Workbooks")?
                .obj_call("Open", vec![Var::str(&full), Var::i32(0) /* UpdateLinks: 안 함 */, Var::bool(false)]);
            let book = match open {
                Ok(b) => b,
                Err(e) => {
                    let _ = app.call("Quit", vec![]);
                    return Err(e);
                }
            };
            // 칸마다 다시 계산하지 않도록 수동으로 두고, 모아 쓴 뒤 한 번 계산한다.
            let _ = app.put("Calculation", Var::i32(XL_CALCULATION_MANUAL));
            let found = (|| -> Result<Vec<CalcSheet>> {
                let worksheets = book.obj("Worksheets")?;
                targets
                    .into_iter()
                    .map(|t| {
                        let ws = worksheets
                            .obj_call("Item", vec![Var::str(&t.sheet)])
                            .map_err(|_| ExcelError::SheetNotFound(t.sheet.clone()))?;
                        // 앱에서 지운 행을 지우고(수식 칸 좌표는 이미 지운 뒤의 것) 숨긴 줄을 편다.
                        let mut sheet = CalcSheet { sheet: ws, origin: t.origin, formulas: Vec::new(), bounds: None };
                        for cut in &t.cuts {
                            sheet.cut_rows(*cut)?;
                        }
                        sheet.unhide(&t.unhide_rows, &t.unhide_cols)?;
                        sheet.bounds = bounding(&t.formulas);
                        sheet.formulas = t.formulas;
                        Ok(sheet)
                    })
                    .collect()
            })();
            match found {
                Ok(sheets) => Ok(CalcCopy { app, book, sheets, path: path.clone() }),
                Err(e) => {
                    let _ = book.call("Close", vec![Var::bool(false)]);
                    let _ = app.call("Quit", vec![]);
                    Err(e)
                }
            }
        })();
        match opened {
            Ok(copy) => {
                let formulas = copy.sheets.iter().map(|s| s.formulas.len()).sum();
                let info = CalcInfo { path: copy.path.clone(), formulas };
                self.calc.insert(key.to_string(), copy);
                Ok(info)
            }
            Err(e) => {
                let _ = std::fs::remove_file(&path);
                Err(e)
            }
        }
    }

    /// 입력(첫 시트)을 사본에 쓰고 다시 계산해 수식 칸 결과를 돌려준다. [`Session::calc_apply_multi`] 참고.
    pub fn calc_apply(&mut self, writes: &[(CellRef, CellValue)]) -> Result<Vec<(CellRef, CellValue)>> {
        let writes: Vec<(u32, CellRef, CellValue)> = writes.iter().map(|(at, v)| (0, *at, v.clone())).collect();
        Ok(self.calc_apply_multi(&writes)?.into_iter().filter(|(s, _, _)| *s == 0).map(|(_, at, v)| (at, v)).collect())
    }

    /// 입력(시트 순번, 모델 좌표)을 사본에 쓰고 한 번 다시 계산해 모든 시트의 수식 칸 결과를 돌려준다(바뀐 것만 고르는 일은
    /// 세션이 한다).
    pub fn calc_apply_multi(&mut self, writes: &[(u32, CellRef, CellValue)]) -> Result<Vec<(u32, CellRef, CellValue)>> {
        self.calc_apply_for(DEFAULT_KEY, writes)
    }

    /// 업무(`key`)의 사본에 입력만 쓴다(다시 계산하지 않는다 — 행을 지우기 전에 그 전 좌표의 입력을 쓸 때).
    pub fn calc_apply_writes_for(&mut self, key: &str, writes: &[(u32, CellRef, CellValue)]) -> Result<()> {
        let copy = self.calc.get(key).ok_or_else(|| ExcelError::Type("계산 사본이 열려 있지 않습니다".into()))?;
        for (sheet, at, v) in writes {
            let Some(s) = copy.sheets.get(*sheet as usize) else { continue };
            let cell = s.sheet.obj_call("Range", vec![Var::str(&s.address(*at))])?;
            cell.put("Value2", to_var(v))?;
        }
        Ok(())
    }

    /// 업무(`key`)의 사본에 입력을 쓰고 다시 계산해 그 사본 모든 시트의 수식 칸 결과를 돌려준다.
    pub fn calc_apply_for(
        &mut self,
        key: &str,
        writes: &[(u32, CellRef, CellValue)],
    ) -> Result<Vec<(u32, CellRef, CellValue)>> {
        self.calc_apply_writes_for(key, writes)?;
        let copy = self.calc.get(key).ok_or_else(|| ExcelError::Type("계산 사본이 열려 있지 않습니다".into()))?;
        copy.app.call("Calculate", vec![])?;
        let mut out = Vec::new();
        for (i, s) in copy.sheets.iter().enumerate() {
            let Some(bounds) = s.bounds else { continue };
            let addr = format!(
                "{}:{}",
                s.address(bounds.top_left()),
                s.address(CellRef::new(bounds.last_row(), bounds.last_col()))
            );
            let grid = s.sheet.obj_call("Range", vec![Var::str(&addr)])?.get("Value2")?.into_grid()?;
            out.extend(
                s.formulas.iter().map(|at| {
                    (i as u32, *at, grid.value((at.row - bounds.row) as usize, (at.col - bounds.col) as usize))
                }),
            );
        }
        Ok(out)
    }

    /// 업무(`key`)의 사본에서 행을 지운다(앱에서 엑셀에서 불러온 행을 지웠다 — 그 순간의 모델 행 좌표). 수식 칸 좌표도
    /// 맞춘다. 다시 계산은 하지 않는다(다음 [`Session::calc_apply_for`]에서).
    pub fn calc_cut_rows_for(&mut self, key: &str, cuts: &[(u32, RowCut)]) -> Result<()> {
        let copy = self.calc.get_mut(key).ok_or_else(|| ExcelError::Type("계산 사본이 열려 있지 않습니다".into()))?;
        for (sheet, cut) in cuts {
            if let Some(s) = copy.sheets.get_mut(*sheet as usize) {
                s.cut_rows(*cut)?;
            }
        }
        Ok(())
    }

    /// 열려 있는 계산 사본 파일. 다시 확정해 사본이 바뀌었는지 확인할 때 쓴다.
    pub fn calc_path(&self) -> Option<&std::path::Path> {
        self.calc_path_for(DEFAULT_KEY)
    }

    /// 업무(`key`)의 계산 사본 파일.
    pub fn calc_path_for(&self, key: &str) -> Option<&std::path::Path> {
        self.calc.get(key).map(|c| c.path.as_path())
    }

    /// 계산 사본을 모두 저장하지 않고 닫는다([`Session::close_all_calc`]).
    pub fn close_calc(&mut self) {
        self.close_all_calc();
    }

    /// 업무(`key`)의 계산 사본을 저장하지 않고 닫고 그 Excel을 끝낸 뒤 임시 파일을 지운다.
    pub fn close_calc_for(&mut self, key: &str) {
        if let Some(copy) = self.calc.remove(key) {
            copy.close();
        }
    }

    /// 모든 업무의 계산 사본을 닫는다(앱을 끝낼 때).
    pub fn close_all_calc(&mut self) {
        for (_, copy) in self.calc.drain() {
            copy.close();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_written_as_literal() {
        assert_eq!(to_var(&CellValue::Text("=SUM(A1)".into())).as_string().as_deref(), Some("'=SUM(A1)"));
        assert_eq!(to_var(&CellValue::Number(3.0)).as_f64(), Some(3.0));
        assert!(to_var(&CellValue::Empty).is_empty());
        assert!(to_var(&CellValue::Text(String::new())).is_empty());
    }

    #[test]
    fn bounding_box() {
        assert_eq!(bounding(&[]), None);
        let cells = [CellRef::new(2, 6), CellRef::new(1001, 6), CellRef::new(5, 3)];
        assert_eq!(bounding(&cells), Some(CellRange::parse("D3:G1002").unwrap()));
    }
}
