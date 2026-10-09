//! 참여자에게 보낼 시트(서버 초기 전송, 3단계). 1b '참여자 시점으로 보기'와 같은 규칙([`Access`])을 쓴다.
//!
//! '남의 행 숨김'이면 남의 행은 값·수식을 지우고 숨긴 행(높이 0)으로 보낸다. 브라우저에서 감추는 게 아니라
//! 아예 보내지 않는다. 단, 내 행에도 걸친 병합 영역(예: 여러 부서에 걸친 본부명)의 값은 남긴다.

use crate::access::{Access, Viewer};
use crate::config::{CollectConfig, OthersRows};
use crate::sheet::{CellValue, SheetModel};
use crate::view::UnknownPerson;

pub struct Projection {
    pub model: SheetModel,
    /// 셀 상태 비트(보는 사람 기준, [`crate::access::flag::MINE`] 포함).
    pub flags: Vec<u8>,
}

pub fn project(model: &SheetModel, config: &CollectConfig, person: &str) -> Result<Projection, UnknownPerson> {
    let access = Access::compile(model, config);
    let p = access.person(person).ok_or_else(|| UnknownPerson(person.to_string()))?;
    let flags = access.viewer_flags(Viewer::Person(p));
    let mut out = model.clone();
    if config.permissions.others_rows == OthersRows::Hidden {
        let mut visible = vec![false; model.rows as usize];
        for r in 0..model.header_rows.min(model.rows) {
            visible[r as usize] = true;
        }
        for r in access.rows_of(p) {
            visible[r as usize] = true;
        }
        // 보이는 행에 걸친 병합의 왼쪽 위 칸은 값을 남긴다.
        let mut keep = vec![false; model.cells.len()];
        for m in &model.merges {
            if (m.row..=m.last_row()).any(|r| visible.get(r as usize).copied().unwrap_or(false)) {
                keep[(m.row * model.cols + m.col) as usize] = true;
            }
        }
        for r in 0..model.rows {
            if visible[r as usize] {
                continue;
            }
            for c in 0..model.cols {
                let i = (r * model.cols + c) as usize;
                if !keep[i] {
                    out.cells[i].v = CellValue::Empty;
                    out.cells[i].f = None;
                }
            }
            out.row_heights[r as usize] = 0.0;
            if !out.hidden_rows.contains(&r) {
                out.hidden_rows.push(r);
            }
        }
        out.hidden_rows.sort_unstable();
    }
    Ok(Projection { model: out, flags })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CellRef;
    use crate::config::tests::budget;

    fn text(m: &SheetModel, a: &str) -> CellValue {
        m.cell(CellRef::parse(a).unwrap()).unwrap().v.clone()
    }

    #[test]
    fn read_only_policy_sends_everything() {
        let m = budget();
        let cfg = CollectConfig::suggest(&m);
        let p = project(&m, &cfg, "김민지").unwrap();
        assert_eq!(p.model.cells, m.cells);
        assert!(p.model.hidden_rows.is_empty());
    }

    #[test]
    fn hidden_policy_strips_other_rows_but_keeps_shared_merge_values() {
        let mut m = budget();
        // A3:A6 '경영지원'을 A3:A8로 넓혀 김민지 행(7·8행)까지 걸치게 한다.
        m.merges.retain(|r| r.to_string() != "A3:A6" && r.to_string() != "A7:A8");
        m.merges.push(crate::CellRange::parse("A3:A8").unwrap());
        let mut cfg = CollectConfig::suggest(&m);
        cfg.permissions.others_rows = OthersRows::Hidden;
        let p = project(&m, &cfg, "김민지").unwrap();
        assert_eq!(p.model.hidden_rows, [2, 3, 4, 5]);
        assert_eq!(p.model.row_heights[2], 0.0);
        assert_eq!(text(&p.model, "F4"), CellValue::Empty); // 남의 행 값은 보내지 않는다
        assert_eq!(text(&p.model, "D3"), CellValue::Empty);
        assert!(p.model.cell(CellRef::parse("G3").unwrap()).unwrap().f.is_none());
        assert_eq!(text(&p.model, "A3"), CellValue::Text("경영지원".into())); // 내 행에도 걸친 병합
        assert_eq!(text(&p.model, "F7"), CellValue::Number(3500.0)); // 내 행
        assert_eq!(text(&p.model, "A1"), CellValue::Text("기본 정보".into())); // 머리글
        assert!(project(&m, &cfg, "모르는 사람").is_err());
    }
}
