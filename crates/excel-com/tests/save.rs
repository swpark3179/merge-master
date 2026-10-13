//! 5단계 엑셀 저장 왕복 시험(Excel 필요): `cargo test -p excel-com --release --test save -- --ignored --test-threads=1`
//! 픽스처(fixtures/generated/budget.xlsx)를 임시 폴더에 복사해 쓰므로 픽스처 자체는 바뀌지 않는다.
#![cfg(windows)]

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use collect_core::export::{ExportOptions, changed_cells, compare, plan};
use collect_core::format::{FormatEdit, StylePatch, apply_format};
use collect_core::history::{BatchOrigin, ChangeEntry, ChangeKind};
use collect_core::{CellRange, CellRef, CellValue, SheetModel};
use excel_com::{ExcelThread, HISTORY_SHEET, ReadOptions, Session, Var};

fn workdir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("merge-master-save-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn fixture_copy(dir: &Path, name: &str) -> PathBuf {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/generated/budget.xlsx");
    assert!(src.exists(), "pnpm fixture 로 {} 를 먼저 만드세요", src.display());
    let dst = dir.join(name);
    std::fs::copy(&src, &dst).unwrap();
    dst
}

fn at(a: &str) -> CellRef {
    CellRef::parse(a).unwrap()
}

fn cell(s: &Session, id: &str, sheet: &str, a: &str) -> excel_com::Dispatch {
    s.worksheet(id, sheet).unwrap().obj_call("Range", vec![Var::str(a)]).unwrap()
}

fn value(s: &Session, id: &str, sheet: &str, a: &str) -> CellValue {
    cell(s, id, sheet, a).get("Value2").unwrap().as_cell_value()
}

const SHEET: &str = "예산신청서";

#[test]
#[ignore = "Excel 필요"]
fn save_copy_and_save_into_round_trip() {
    let dir = workdir();
    let source = fixture_copy(&dir, "원본.xlsx");
    let out = dir.join("원본_취합.xlsx");
    let _ = std::fs::remove_file(&out);
    let excel = ExcelThread::spawn().unwrap();
    let (source2, out2) = (source.clone(), out.clone());

    let result = excel.run(move |s| -> excel_com::Result<()> {
        let id = s.open_file(&source2)?.id;
        let opts = ReadOptions { range: Some("A1:I1002".into()), ..Default::default() };
        let (baseline, _) = s.read_sheet(&id, SHEET, &opts, &mut |_| true)?;
        let status = s.locate(&id)?;
        assert!(status.saved && !status.read_only);

        // 참여자 입력과 앱에서 바꾼 서식·병합
        let mut current: SheetModel = baseline.clone();
        let mut set = |a: &str, v: CellValue| current.cell_mut(at(a)).unwrap().v = v;
        set("F3", CellValue::Number(9800.0));
        set("H3", CellValue::Text("=1+1".into())); // 글자 그대로(수식이 되면 안 됨)
        set("H4", CellValue::Text("0012".into())); // 글자 그대로(숫자 12가 되면 안 됨)
        set("I3", CellValue::Text("1분기".into()));
        set("F4", CellValue::Number(4500.0));
        let bold = StylePatch { bold: Some(true), ..Default::default() };
        apply_format(&mut current, FormatEdit::Style { range: CellRange::parse("H3").unwrap(), patch: bold }).unwrap();
        apply_format(&mut current, FormatEdit::Merge { range: CellRange::parse("H8:I9").unwrap() }).unwrap();
        let history = vec![ChangeEntry {
            seq: 1,
            batch: "b1".into(),
            origin: BatchOrigin::Edit,
            kind: ChangeKind::Input,
            at_ms: 1_791_000_000_000,
            user: "이서연".into(),
            sheet: 0,
            row: 2,
            col: 5,
            old: CellValue::Empty,
            new: CellValue::Number(9800.0),
            gone: false,
        }];
        let options = ExportOptions { mark: Some(0xFFF3B0), styles: true, history: true, cancelled: true };
        let p = plan(&baseline, &current, &[], &history, options, 540, &HashSet::new());
        assert_eq!(p.cells.len(), 5);

        // 저장 전 확인: Excel에서 F4를 직접 고친 경우 충돌로 잡는다.
        cell(s, &id, SHEET, "F4").put("Value2", Var::f64(1111.0))?;
        let now = s.read_range_values(&id, SHEET, baseline.origin, baseline.rows, baseline.cols)?;
        let check = compare(&baseline, &current, &now, &p.cells);
        assert_eq!(check.conflicts.len(), 1, "{check:?}");
        assert_eq!(check.conflicts[0].at, at("F4"));
        assert!(!check.suspicious);
        // 'Excel 값 유지'를 고르면 그 칸은 쓰지 않는다.
        let skip: HashSet<CellRef> = [at("F4")].into();
        let p = plan(&baseline, &current, &[], &history, options, 540, &skip);

        // ---- 다른 이름으로 저장: 원본은 그대로
        let report = s.save_copy(&id, SHEET, &p, &out2)?;
        assert_eq!(report.written, 4);
        assert!(report.mismatched.is_empty(), "{:?}", report.mismatched);
        assert_eq!(report.history_sheet.as_deref(), Some(HISTORY_SHEET));
        assert!(out2.exists());
        assert_eq!(value(s, &id, SHEET, "F3"), CellValue::Empty, "원본은 바뀌지 않는다");

        let copy = s.open_file(&out2)?.id;
        assert_eq!(value(s, &copy, SHEET, "F3"), CellValue::Number(9800.0));
        assert_eq!(value(s, &copy, SHEET, "H3"), CellValue::Text("=1+1".into()));
        assert_eq!(cell(s, &copy, SHEET, "H3").get("HasFormula")?.as_bool(), Some(false));
        assert_eq!(value(s, &copy, SHEET, "H4"), CellValue::Text("0012".into()));
        assert_eq!(value(s, &copy, SHEET, "F4"), CellValue::Number(1111.0), "Excel 값 유지");
        let CellValue::Number(g3) = value(s, &copy, SHEET, "G3") else { panic!("G3는 수식 결과") };
        assert!((g3 - (9800.0 / 9200.0 - 1.0)).abs() < 1e-9, "수식이 다시 계산된다: {g3}");
        assert_eq!(cell(s, &copy, SHEET, "H3").obj("Font")?.get("Bold")?.as_bool(), Some(true));
        assert_eq!(cell(s, &copy, SHEET, "H8").get("MergeCells")?.as_bool(), Some(true));
        assert_eq!(cell(s, &copy, SHEET, "F3").obj("Interior")?.get("Color")?.as_f64(), Some(f64::from(0xB0F3FFu32)));
        assert_eq!(value(s, &copy, HISTORY_SHEET, "B1"), CellValue::Text("사용자".into()));
        assert_eq!(value(s, &copy, HISTORY_SHEET, "B2"), CellValue::Text("이서연".into()));
        assert_eq!(value(s, &copy, HISTORY_SHEET, "C2"), CellValue::Text("F3".into()));
        assert!(matches!(value(s, &copy, HISTORY_SHEET, "A2"), CellValue::Number(_)));
        s.workbook(&copy)?.call("Close", vec![Var::bool(false)])?;

        // 원본과 같은 이름이나 다른 형식으로는 저장하지 않는다.
        assert!(s.save_copy(&id, SHEET, &p, &source2).is_err());
        assert!(s.save_copy(&id, SHEET, &p, &out2.with_extension("xlsm")).is_err());

        // ---- 원본에 반영: 쓰고 저장
        let only_values = plan(&baseline, &current, &[], &[], ExportOptions::default(), 0, &skip);
        let report = s.save_into(&id, SHEET, &only_values)?;
        assert_eq!(report.written, 4);
        assert!(report.mismatched.is_empty());
        assert!(s.locate(&id)?.saved);
        s.workbook(&id)?.call("Close", vec![Var::bool(false)])?;
        let reopened = s.open_file(&source2)?.id;
        assert_eq!(value(s, &reopened, SHEET, "F3"), CellValue::Number(9800.0));
        assert_eq!(value(s, &reopened, SHEET, "F4"), CellValue::Number(1111.0));
        assert_eq!(value(s, &reopened, SHEET, "H4"), CellValue::Text("0012".into()));
        assert_ne!(
            cell(s, &reopened, SHEET, "F3").obj("Interior")?.get("Color")?.as_f64(),
            Some(f64::from(0xB0F3FFu32))
        );

        // 다시 저장하려면 쓴 값이 새 기준이 된다: 바뀐 칸이 없다.
        let mut saved = baseline.clone();
        for a in ["F3", "H3", "H4", "I3"] {
            saved.cell_mut(at(a)).unwrap().v = current.cell(at(a)).unwrap().v.clone();
        }
        saved.cell_mut(at("F4")).unwrap().v = CellValue::Number(1111.0);
        current.cell_mut(at("F4")).unwrap().v = CellValue::Number(1111.0);
        assert!(changed_cells(&saved, &current).is_empty());
        s.workbook(&reopened)?.call("Close", vec![Var::bool(false)])?;
        Ok(())
    });
    excel.run(|s| s.close_background()).unwrap();
    drop(excel);
    let _ = std::fs::remove_dir_all(&dir);
    result.unwrap().unwrap();
}
