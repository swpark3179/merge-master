//! 통합문서 열거 → 파일 열기 → 사전 감지 → 시트 읽기를 단계별 시간·호출 수와 함께 출력한다.
//!
//! ```text
//! cargo run -p excel-com --example spike --release -- fixtures/generated/budget.xlsx [시트이름]
//! ```
//! 결과 모델을 `<파일>.sheet.json`으로 저장한다(그리드·dev 서버·E2E가 읽는다).

use std::path::PathBuf;
use std::time::Instant;

use excel_com::{ExcelThread, ReadOptions};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = PathBuf::from(args.next().ok_or("통합문서 경로를 넘겨 주세요")?);
    let sheet = args.next();

    let excel = ExcelThread::spawn()?;

    let t = Instant::now();
    let status = excel.run(|s| s.list_workbooks())??;
    println!(
        "[열거] Excel {} · 통합문서 {}개 ({} ms)",
        if status.running { "실행 중" } else { "없음" },
        status.workbooks.len(),
        t.elapsed().as_millis()
    );
    for wb in &status.workbooks {
        println!("  - {} · 시트 {:?}{}", wb.id, wb.sheets, if wb.background { " (백그라운드)" } else { "" });
    }

    let t = Instant::now();
    let p = path.clone();
    let info = excel.run(move |s| s.open_file(&p))??;
    println!(
        "[열기] {} → 시트 {:?}, 활성 '{}'{} ({} ms)",
        info.name,
        info.sheets,
        info.active_sheet,
        if info.background { " · 백그라운드" } else { "" },
        t.elapsed().as_millis()
    );

    let sheet = sheet.unwrap_or_else(|| info.active_sheet.clone());
    let (id, name) = (info.id.clone(), sheet.clone());
    let ins = excel.run(move |s| s.inspect_sheet(&id, &name))??;
    println!(
        "[감지] 사용 범위 {} → 표 {} · 머리글 {}행 · 틀 고정 {}행 {}열 · 숨김 {}행 {}열 ({} ms, 호출 {}회)",
        ins.used_range,
        ins.table,
        ins.header_rows,
        ins.frozen_rows,
        ins.frozen_cols,
        ins.hidden_rows,
        ins.hidden_cols,
        ins.elapsed_ms,
        ins.com_calls
    );
    println!("  열 {:?}", ins.columns.iter().map(|c| c.label.as_str()).collect::<Vec<_>>());
    println!("  수식 열 {:?}", ins.formula_columns);
    for a in &ins.assignee_candidates {
        println!("  배정 기준 후보: {}열 '{}' · {}명 {:?} (점수 {})", a.col, a.label, a.distinct, a.samples, a.score);
    }

    let opts = ReadOptions {
        range: Some(ins.table.to_string()),
        header_rows: Some(ins.header_rows),
        ..ReadOptions::default()
    };
    let id = info.id.clone();
    let (model, stats) = excel.run(move |s| {
        let mut last_stage = None;
        s.read_sheet(&id, &sheet, &opts, &mut |p| {
            if last_stage != Some(p.stage) {
                eprintln!("    · {:?}", p.stage);
                last_stage = Some(p.stage);
            }
            true
        })
    })??;

    println!("[읽기] {}!{} · {}행 × {}열", model.workbook, model.sheet, model.rows, model.cols);
    println!("  값        {:>6} ms", stats.values_ms);
    println!("  수식      {:>6} ms  ({}개)", stats.formulas_ms, model.cells.iter().filter(|c| c.f.is_some()).count());
    println!("  병합      {:>6} ms  ({}개)", stats.merges_ms, model.merges.len());
    println!(
        "  서식      {:>6} ms  (탐색 블록 {} · 확정 영역 {} · 고유 서식 {})",
        stats.styles_ms,
        stats.style_probes,
        stats.style_regions,
        model.styles.len()
    );
    println!(
        "  치수/고정 {:>6} ms  (틀 고정 {}행 {}열 · 숨김 {}행 {}열)",
        stats.dimensions_ms,
        model.frozen_rows,
        model.frozen_cols,
        model.hidden_rows.len(),
        model.hidden_cols.len()
    );
    println!("  유효성    {:>6} ms  ({}개)", stats.validations_ms, model.validations.len());
    println!("  머리글 {}행 · 합계 {} ms · Excel 호출 {}회", model.header_rows, stats.total_ms, stats.com_calls);

    let out = path.with_extension("sheet.json");
    std::fs::write(&out, serde_json::to_vec(&model)?)?;
    println!("[저장] {} ({} KB)", out.display(), std::fs::metadata(&out)?.len() / 1024);

    excel.run(|s| s.close_background())?;
    Ok(())
}
