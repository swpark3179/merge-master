//! 앱이 강제로 꺼졌을 때 남은 숨은 Excel 정리(Excel 필요): `cargo test -p excel-com --release --test orphans -- --ignored`
#![cfg(windows)]

use std::path::PathBuf;

use collect_core::CellRef;
use excel_com::ExcelThread;

#[test]
#[ignore = "Excel 필요"]
fn next_app_closes_old_calc_copy_and_adopts_hidden_source() {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/generated/budget.xlsx");
    assert!(src.exists(), "pnpm fixture 로 만드세요");
    let dir = std::env::temp_dir().join(format!("mm-orphans-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("고아원본.xlsx");
    std::fs::copy(&src, &path).unwrap();
    let other = dir.join("함께 연 파일.xlsx");
    std::fs::copy(&src, &other).unwrap();

    // 앞 앱: 원본을 숨은 Excel로 열고 계산 사본을 연 채 '죽는다'(정리하지 않음).
    let old = ExcelThread::spawn().unwrap();
    let (p, o) = (path.clone(), other.clone());
    let (id, calc) = old
        .run(move |s| -> excel_com::Result<_> {
            s.open_file(&o)?;
            let id = s.open_file(&p)?.id;
            let calc = s.open_calc(&id, "예산신청서", CellRef::new(0, 0), vec![CellRef::new(2, 6)])?;
            Ok((id, calc.path))
        })
        .unwrap()
        .unwrap();
    assert!(calc.exists());
    std::mem::forget(old);
    let before = excel_pids();

    // 새 앱
    let new = ExcelThread::spawn().unwrap();
    let source = id.clone();
    let report = new.run(move |s| s.adopt_orphans(Some(&source))).unwrap().unwrap();
    assert_eq!(report.closed_calc, 1, "{report:?}");
    assert!(report.adopted, "원본이 열린 숨은 Excel을 받아들인다");
    assert!(!calc.exists(), "옛 계산 사본 파일을 지운다");
    // 받아들인 원본은 이 앱 것처럼 쓸 수 있고, 끝낼 때 함께 닫힌다.
    let source = id.clone();
    let status = new.run(move |s| s.locate(&source)).unwrap().unwrap();
    assert!(status.background);
    new.run(|s| s.close_background()).unwrap();
    let source = id.clone();
    let still = new.run(move |s| s.peek(&source)).unwrap().expect("목록");
    assert!(still.is_none(), "끝낼 때 닫혔다");
    drop(new);
    let _ = std::fs::remove_dir_all(&dir);
    // 앞 앱이 띄운 두 인스턴스(원본·계산 사본)가 모두 끝났다.
    let left: Vec<String> = excel_pids().into_iter().filter(|p| before.contains(p)).collect();
    assert!(left.is_empty(), "남은 Excel: {left:?}");
}

fn excel_pids() -> Vec<String> {
    let out = std::process::Command::new("tasklist")
        .args(["/FI", "IMAGENAME eq EXCEL.EXE", "/FO", "CSV", "/NH"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| l.contains("EXCEL.EXE"))
        .filter_map(|l| l.split(',').nth(1).map(|p| p.trim_matches('"').to_string()))
        .collect()
}

#[test]
#[ignore = "Excel 필요"]
fn adopted_excel_with_unsaved_work_is_left_running() {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/generated/budget.xlsx");
    let dir = std::env::temp_dir().join(format!("mm-orphans-unsaved-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (path, other) = (dir.join("원본.xlsx"), dir.join("고치는 중.xlsx"));
    std::fs::copy(&src, &path).unwrap();
    std::fs::copy(&src, &other).unwrap();

    // 같은 숨은 Excel에 저장하지 않은 변경이 있는 문서가 있다(다른 프로그램의 작업일 수 있다).
    let old = ExcelThread::spawn().unwrap();
    let (p, o) = (path.clone(), other.clone());
    let id = old
        .run(move |s| -> excel_com::Result<_> {
            let other_id = s.open_file(&o)?.id;
            let id = s.open_file(&p)?.id;
            let ws = s.worksheet(&other_id, "예산신청서")?;
            ws.obj_call("Range", vec![excel_com::Var::str("A1")])?.put("Value2", excel_com::Var::str("작업 중"))?;
            Ok(id)
        })
        .unwrap()
        .unwrap();
    std::mem::forget(old);
    let before = excel_pids();

    let new = ExcelThread::spawn().unwrap();
    let source = id.clone();
    assert!(new.run(move |s| s.adopt_orphans(Some(&source))).unwrap().unwrap().adopted);
    new.run(|s| s.close_background()).unwrap();
    drop(new);
    std::thread::sleep(std::time::Duration::from_secs(4));
    let alive: Vec<String> = excel_pids().into_iter().filter(|p| before.contains(p)).collect();
    assert_eq!(alive.len(), 1, "저장하지 않은 문서가 있는 Excel은 끝내지 않는다");
    // 시험 정리: 남겨 둔 Excel을 끝낸다.
    for pid in alive {
        let _ = std::process::Command::new("taskkill").args(["/F", "/PID", &pid]).output();
    }
    let _ = std::fs::remove_dir_all(&dir);
}
