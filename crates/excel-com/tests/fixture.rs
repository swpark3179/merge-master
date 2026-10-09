//! Excel이 설치된 PC에서만 실행: `cargo test -p excel-com -- --ignored --test-threads=1`
//! 먼저 `pnpm fixture`로 fixtures/generated/{budget,variants}.xlsx를 만들어 둔다.
#![cfg(windows)]

use std::path::PathBuf;

use collect_core::{CellRange, CellRef, CellValue, HAlign, ValidationRule};
use excel_com::{ExcelError, ExcelThread, ImportItems, ReadOptions, ReadStage, Var, WorkbookInfo};

fn fixture(name: &str) -> PathBuf {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/generated").join(name);
    assert!(path.exists(), "pnpm fixture 로 {} 를 먼저 만드세요", path.display());
    path
}

fn open(excel: &ExcelThread, name: &str) -> WorkbookInfo {
    let path = fixture(name);
    excel.run(move |s| s.open_file(&path)).unwrap().unwrap()
}

#[test]
#[ignore = "Excel 필요"]
fn budget_inspection_and_read() {
    let excel = ExcelThread::spawn().unwrap();
    let info = open(&excel, "budget.xlsx");
    assert_eq!(info.sheets, ["예산신청서", "작성가이드", "코드표"]);

    let id = info.id.clone();
    let ins = excel.run(move |s| s.inspect_sheet(&id, "예산신청서")).unwrap().unwrap();
    assert_eq!(ins.table, CellRange::parse("A1:I1002").unwrap());
    assert_eq!((ins.header_rows, ins.frozen_rows, ins.frozen_cols), (2, 2, 4));
    assert_eq!(ins.columns.iter().map(|c| c.label.as_str()).collect::<Vec<_>>()[..3], ["본부", "부서", "담당자"]);
    assert_eq!(ins.formula_columns.len(), 1);
    assert_eq!((ins.formula_columns[0].col, ins.formula_columns[0].count), (6, 1000));
    assert_eq!(ins.assignee_candidates.first().map(|a| (a.col, a.distinct)), Some((2, 6)));
    assert_eq!(ins.validations.len(), 1);
    assert_eq!(ins.validations[0].label, "집행 시기");

    let id = info.id.clone();
    let opts = ReadOptions { range: Some(ins.table.to_string()), ..Default::default() };
    let (m, _stats) = excel.run(move |s| s.read_sheet(&id, "예산신청서", &opts, &mut |_| true)).unwrap().unwrap();
    excel.run(|s| s.close_background()).unwrap();

    assert_eq!((m.rows, m.cols, m.origin), (1002, 9, CellRef::new(0, 0)));
    assert_eq!((m.header_rows, m.frozen_rows, m.frozen_cols), (2, 2, 4));
    assert!(m.hidden_rows.is_empty() && m.hidden_cols.is_empty());

    let text = |r, c| m.cell(CellRef::new(r, c)).unwrap().v.clone();
    assert_eq!(text(0, 0), CellValue::Text("기본 정보".into()));
    assert_eq!(text(1, 5), CellValue::Text("신청 금액".into()));
    assert!(matches!(text(2, 4), CellValue::Number(_)));
    assert_eq!(m.cells.iter().filter(|c| c.f.is_some()).count(), 1000);
    assert_eq!(m.cell(CellRef::new(2, 6)).unwrap().f.as_deref(), Some(r#"=IF(OR(E3="",F3=""),"",F3/E3-1)"#));

    // 병합: 머리글 3개 + 데이터 블록 440개, 병합된 담당자는 아래 행에서도 같은 값으로 보인다.
    assert_eq!(m.merges.len(), 443);
    assert!(m.merges.contains(&CellRange::parse("A1:D1").unwrap()));
    assert!(m.merges.contains(&CellRange::parse("C3:C7").unwrap()));
    assert_eq!(m.effective_value(CellRef::new(5, 2)), Some(&CellValue::Text("이서연".into())));

    let style = |r, c| &m.styles[m.cell(CellRef::new(r, c)).unwrap().s as usize];
    assert!(style(1, 0).bold);
    assert_eq!(style(2, 0).h_align, HAlign::Center);
    assert_eq!(style(2, 5).number_format, "#,##0");
    assert_eq!(style(2, 4).number_format, "#,##0");
    assert_eq!(style(2, 7).number_format, "General"); // 한국어 Excel의 'G/표준'을 정규화
    assert_eq!(style(2, 4).borders.bottom.map(|b| b.color), Some(0xBFBFBF));

    let v = &m.validations[0];
    assert_eq!(v.range, CellRange::parse("I3:I1002").unwrap());
    match &v.rule {
        ValidationRule::List { items, .. } => assert_eq!(items, &["1분기", "2분기", "3분기", "4분기", "연중"]),
        other => panic!("목록 유효성이어야 함: {other:?}"),
    }
}

#[test]
#[ignore = "Excel 필요"]
fn variants_title_rows_hidden_and_inactive_freeze() {
    let excel = ExcelThread::spawn().unwrap();
    let other = open(&excel, "budget.xlsx");
    let info = open(&excel, "variants.xlsx");
    assert_eq!(info.active_sheet, "다른시트");
    // 같은 Excel에 다른 통합문서가 활성인 상태에서도 비활성 시트의 틀 고정을 읽어야 한다.
    let other_id = other.id.clone();
    excel.run(move |s| s.workbook(&other_id).unwrap().call("Activate", vec![]).map(drop)).unwrap().unwrap();
    // 창이 아주 작아도(SplitRow가 '보이는' 칸 수로 줄어드는 조건) 틀 고정을 정확히 읽어야 한다.
    let id = info.id.clone();
    excel
        .run(move |s| {
            let win = s.workbook(&id)?.obj("Windows")?.obj_call("Item", vec![Var::i32(1)])?;
            win.put("WindowState", Var::i32(-4143))?; // xlNormal
            win.put("Height", Var::f64(60.0))?;
            win.put("Width", Var::f64(80.0))
        })
        .unwrap()
        .unwrap();

    let id = info.id.clone();
    let ins = excel.run(move |s| s.inspect_sheet(&id, "제목있음")).unwrap().unwrap();
    assert_eq!(ins.used_range, CellRange::parse("A1:G30").unwrap());
    // 제목·빈 행·안내 행을 건너뛰고, 아래쪽 빈 테두리 행은 잘라낸다.
    assert_eq!(ins.table, CellRange::parse("A4:G25").unwrap());
    // 4~5행 머리글: 5행은 굵지 않지만 4행의 세로 병합(부서·담당자…)에 걸쳐 있다.
    assert_eq!(ins.header_rows, 2);
    let labels: Vec<_> = ins.columns.iter().map(|c| c.label.as_str()).collect();
    assert_eq!(labels, ["부서", "담당자", "과정명", "인원", "시간", "비고", "작성자 확인"]);
    // 비활성 시트의 틀 고정(C6)을 읽고, 활성 시트는 원래대로 되돌린다.
    assert_eq!((ins.frozen_rows, ins.frozen_cols), (5, 2));
    assert_eq!((ins.hidden_rows, ins.hidden_cols), (1, 1));
    assert_eq!(ins.assignee_candidates.first().map(|a| (a.col, a.label.as_str())), Some((1, "담당자")));
    let status = excel.run(|s| s.list_workbooks()).unwrap().unwrap();
    let book = status.workbooks.iter().find(|b| b.id == info.id).unwrap();
    assert_eq!(book.active_sheet, "다른시트");

    // 숨김 제외(기본): 12행(범위 기준 8) · F열(5)이 숨김 목록에 있고 크기는 0
    let read = |items: ImportItems| {
        let id = info.id.clone();
        let opts = ReadOptions { range: Some("A4:G25".into()), items, ..Default::default() };
        excel.run(move |s| s.read_sheet(&id, "제목있음", &opts, &mut |_| true)).unwrap().unwrap().0
    };
    let m = read(ImportItems::default());
    assert_eq!((m.rows, m.cols, m.header_rows), (22, 7, 2));
    assert_eq!((m.frozen_rows, m.frozen_cols), (2, 2));
    assert_eq!((m.hidden_rows.as_slice(), m.hidden_cols.as_slice()), (&[8u32][..], &[5u32][..]));
    assert_eq!((m.row_heights[8], m.col_widths[5]), (0.0, 0.0));
    assert!(m.merges.contains(&CellRange::parse("A1:A2").unwrap()));
    assert!(m.styles[m.cell(CellRef::new(0, 0)).unwrap().s as usize].bold);

    // 숨김 포함 + 글꼴·테두리 끄기
    let m = read(ImportItems { include_hidden: true, fonts: false, borders: false, ..Default::default() });
    assert!(m.hidden_rows.is_empty() && m.hidden_cols.is_empty());
    assert!(m.row_heights[8] > 0.0 && m.col_widths[5] > 0.0);
    assert!(m.styles.iter().all(|s| !s.bold && s.borders.top.is_none() && s.borders.bottom.is_none()));

    // 같은 파일을 다시 열면 이미 열린 문서를 쓴다.
    assert_eq!(open(&excel, "variants.xlsx").id, info.id);

    // 진행률 콜백이 false를 돌려주면 취소된다.
    let id = info.id.clone();
    let cancelled = excel
        .run(move |s| s.read_sheet(&id, "제목있음", &ReadOptions::default(), &mut |p| p.stage != ReadStage::Styles))
        .unwrap();
    assert!(matches!(cancelled, Err(ExcelError::Cancelled)));

    // 너무 큰 범위는 읽기 전에 거절한다.
    let id = info.id.clone();
    let opts = ReadOptions { range: Some("A1:Z100000".into()), ..Default::default() };
    let big = excel.run(move |s| s.read_sheet(&id, "제목있음", &opts, &mut |_| true)).unwrap();
    assert!(matches!(big, Err(ExcelError::TooLarge { .. })));

    excel.run(|s| s.close_background()).unwrap();
}

#[test]
#[ignore = "Excel 필요"]
fn calc_copy_recalculates_without_touching_original() {
    let excel = ExcelThread::spawn().unwrap();
    let info = open(&excel, "budget.xlsx");
    let id = info.id.clone();
    // G3:G1002 증감률 = IF(OR(E="",F=""),"",F/E-1). 모델 좌표 = 시트 좌표(origin A1).
    let formulas: Vec<CellRef> = (2..1002).map(|r| CellRef::new(r, 6)).collect();
    let result = excel
        .run(move |s| -> excel_com::Result<_> {
            let calc = s.open_calc(&id, "예산신청서", CellRef::new(0, 0), formulas)?;
            let listed = s.list_workbooks()?;
            let before = s.calc_apply(&[])?;
            let after = s.calc_apply(&[
                (CellRef::new(2, 5), CellValue::Number(9800.0)),
                (CellRef::new(2, 7), CellValue::Text("=1+1".into())),
            ])?;
            let original = s.worksheet(&id, "예산신청서")?.obj_call("Range", vec![Var::str("F3")])?.get("Value2")?;
            let path = calc.path.clone();
            s.close_calc();
            Ok((calc, listed, before, after, original.as_cell_value(), path.exists()))
        })
        .unwrap();
    excel.run(|s| s.close_background()).unwrap();
    let (calc, listed, before, after, original, still_there) = result.unwrap();

    assert_eq!(calc.formulas, 1000);
    assert!(listed.workbooks.iter().all(|b| !b.name.starts_with("calc-")), "계산 사본은 목록에 보이지 않는다");
    assert_eq!(before[0], (CellRef::new(2, 6), CellValue::Empty)); // F3이 비어 있어 ""
    let CellValue::Number(g3) = after[0].1 else { panic!("{:?}", after[0]) };
    assert!((g3 - (9800.0 / 9200.0 - 1.0)).abs() < 1e-9, "{g3}");
    assert_eq!(original, CellValue::Empty, "원본은 건드리지 않는다");
    assert!(!still_there, "닫으면 임시 파일을 지운다");
}
