//! 권한 엔진: **입력 가능 = (배정 행 ∪ 배정 범위 ∪ 담당자 없는 칸) ∩ 입력 영역 − 입력 금지 − 수식**.
//!
//! 행 배정도 범위 배정도 걸리지 않은 칸은 담당자가 없는 칸으로, 입력 영역이면 **누구나** 입력할 수 있다.
//!
//! [`Access::compile`]이 셀마다 상태 비트([`flag`])를 미리 계산해 두므로 셀 하나의 판정은 O(1)
//! (범위 배정 수만큼의 비교 제외)이다. 호스트 1b 화면, 참여자 시점 미리보기, 3단계 서버가 모두 이것을 쓴다.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::a1::{CellRange, CellRef};
use crate::config::{CollectConfig, ColumnRole, ExceptionKind};
use crate::sheet::{CellValue, MergeIndex, SheetModel};

/// 셀 상태 비트. TS(`packages/shared`)에도 같은 값을 둔다.
pub mod flag {
    /// 입력 영역(배정된 사람이 입력할 수 있는 셀).
    pub const INPUT: u8 = 1;
    pub const REQUIRED: u8 = 2;
    /// 셀 예외가 걸린 셀(화면에 점선 테두리).
    pub const EXCEPTION: u8 = 4;
    pub const FORMULA: u8 = 8;
    pub const HIDDEN: u8 = 16;
    pub const HEADER: u8 = 32;
    /// 병합 영역에서 왼쪽 위가 아닌 셀(값을 갖지 않는다).
    pub const COVERED: u8 = 64;
    /// 보는 사람이 입력할 수 있다(시점별 계산 결과에만 붙는다).
    pub const MINE: u8 = 128;
}

/// 누구의 시점인가. 호스트는 입력 영역 전체를 편집할 수 있다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Viewer {
    Host,
    Person(usize),
}

pub struct Access {
    rows: u32,
    cols: u32,
    header_rows: u32,
    base: Vec<u8>,
    roster: Vec<String>,
    /// 행마다 행 배정 담당자(명단 인덱스), 없으면 -1.
    row_owner: Vec<i32>,
    /// 범위 배정: (범위, 명단 인덱스).
    ranges: Vec<(CellRange, usize)>,
}

fn same_name(a: &str, b: &str) -> bool {
    a.trim() == b.trim()
}

impl Access {
    pub fn compile(model: &SheetModel, config: &CollectConfig) -> Self {
        let (rows, cols) = (model.rows, model.cols);
        let n = (rows * cols) as usize;
        let idx = |at: CellRef| (at.row * cols + at.col) as usize;
        let bounds = model.bounds();

        // 셀 예외: 입력 금지 > 입력 가능, 필수는 따로 더한다.
        let mut exc = vec![0u8; n];
        const EXC_LOCKED: u8 = 1;
        const EXC_INPUT: u8 = 2;
        const EXC_REQUIRED: u8 = 4;
        for e in &config.exceptions {
            let Some(r) = e.range.intersect(&bounds) else { continue };
            let bit = match e.kind {
                ExceptionKind::Locked => EXC_LOCKED,
                ExceptionKind::Input => EXC_INPUT,
                ExceptionKind::Required => EXC_REQUIRED,
            };
            for at in r.cells() {
                exc[idx(at)] |= bit;
            }
        }

        let mut covered = vec![false; n];
        for m in &model.merges {
            for at in m.cells().skip(1) {
                if at.row < rows && at.col < cols {
                    covered[idx(at)] = true;
                }
            }
        }
        let mut hidden_row = vec![false; rows as usize];
        for &r in &model.hidden_rows {
            if let Some(h) = hidden_row.get_mut(r as usize) {
                *h = true;
            }
        }
        let mut hidden_col = vec![false; cols as usize];
        for &c in &model.hidden_cols {
            if let Some(h) = hidden_col.get_mut(c as usize) {
                *h = true;
            }
        }

        let mut base = vec![0u8; n];
        for r in 0..rows {
            for c in 0..cols {
                let at = CellRef::new(r, c);
                let i = idx(at);
                let mut f = 0u8;
                if r < model.header_rows {
                    f |= flag::HEADER;
                }
                if hidden_row[r as usize] || hidden_col[c as usize] {
                    f |= flag::HIDDEN;
                }
                if model.cells[i].f.is_some() {
                    f |= flag::FORMULA;
                }
                if covered[i] {
                    f |= flag::COVERED;
                }
                if exc[i] != 0 {
                    f |= flag::EXCEPTION;
                }
                let rule = config.columns.get(c as usize);
                let col_input = rule.is_some_and(|r| r.role == ColumnRole::Input);
                let blocked = flag::HEADER | flag::HIDDEN | flag::FORMULA | flag::COVERED;
                let input = (col_input || exc[i] & EXC_INPUT != 0) && exc[i] & EXC_LOCKED == 0 && f & blocked == 0;
                if input {
                    f |= flag::INPUT;
                    if rule.is_some_and(|r| r.required) || exc[i] & EXC_REQUIRED != 0 {
                        f |= flag::REQUIRED;
                    }
                }
                base[i] = f;
            }
        }

        // 명단: 행 배정 기준 열의 이름(나온 순서) + 직접 넣은 이름 + 범위 배정 받은 이름.
        let index = MergeIndex::new(model);
        let mut roster: Vec<String> = Vec::new();
        let add = |name: &str, roster: &mut Vec<String>| -> Option<usize> {
            let name = name.trim();
            if name.is_empty() {
                return None;
            }
            Some(roster.iter().position(|p| p == name).unwrap_or_else(|| {
                roster.push(name.to_string());
                roster.len() - 1
            }))
        };
        let mut row_owner = vec![-1i32; rows as usize];
        if let Some(key) = config.assignment.key_col.filter(|&k| k < cols) {
            for r in model.header_rows..rows {
                if let Some(CellValue::Text(name)) = model.effective_value_in(&index, CellRef::new(r, key))
                    && let Some(p) = add(name, &mut roster)
                {
                    row_owner[r as usize] = p as i32;
                }
            }
        }
        for name in &config.assignment.extra_people {
            add(name, &mut roster);
        }
        let ranges = config
            .assignment
            .ranges
            .iter()
            .filter_map(|a| Some((a.range.intersect(&bounds)?, add(&a.person, &mut roster)?)))
            .collect();

        Self { rows, cols, header_rows: model.header_rows, base, roster, row_owner, ranges }
    }

    pub fn roster(&self) -> &[String] {
        &self.roster
    }

    pub fn person(&self, name: &str) -> Option<usize> {
        self.roster.iter().position(|p| same_name(p, name))
    }

    pub fn flags(&self, at: CellRef) -> u8 {
        if at.row >= self.rows || at.col >= self.cols {
            return 0;
        }
        self.base[(at.row * self.cols + at.col) as usize]
    }

    pub fn row_owner(&self, row: u32) -> Option<usize> {
        self.row_owner.get(row as usize).and_then(|&p| (p >= 0).then_some(p as usize))
    }

    /// 행마다 담당자 인덱스(-1 = 없음). 화면의 행 번호 옆 색 점에 쓴다.
    pub fn row_owners(&self) -> &[i32] {
        &self.row_owner
    }

    /// 셀을 맡은 사람들(행 담당자 + 범위 배정). 중복 없음.
    pub fn owners(&self, at: CellRef) -> Vec<usize> {
        let mut out = Vec::new();
        if let Some(p) = self.row_owner(at.row) {
            out.push(p);
        }
        for (r, p) in &self.ranges {
            if r.contains(at) && !out.contains(p) {
                out.push(*p);
            }
        }
        out
    }

    /// 담당자(행 배정·범위 배정)가 아무도 없는 칸인가. 입력 영역이면 누구나 입력할 수 있다.
    pub fn is_open(&self, at: CellRef) -> bool {
        self.row_owner(at.row).is_none() && !self.ranges.iter().any(|(r, _)| r.contains(at))
    }

    pub fn can_edit(&self, viewer: Viewer, at: CellRef) -> bool {
        if self.flags(at) & flag::INPUT == 0 {
            return false;
        }
        match viewer {
            Viewer::Host => true,
            Viewer::Person(p) => match self.row_owner(at.row) {
                Some(owner) => owner == p || self.ranges.iter().any(|(r, q)| *q == p && r.contains(at)),
                // 행 담당자가 없으면: 내 범위 배정이거나, 아무 범위 배정에도 걸리지 않은 칸(누구나).
                None => {
                    let mut covered = false;
                    for (r, q) in &self.ranges {
                        if r.contains(at) {
                            if *q == p {
                                return true;
                            }
                            covered = true;
                        }
                    }
                    !covered
                }
            },
        }
    }

    /// 셀마다 상태 비트에 보는 사람 기준 [`flag::MINE`]을 더한 배열(행 우선).
    pub fn viewer_flags(&self, viewer: Viewer) -> Vec<u8> {
        let mut out = self.base.clone();
        for r in 0..self.rows {
            for c in 0..self.cols {
                let at = CellRef::new(r, c);
                if self.can_edit(viewer, at) {
                    out[(r * self.cols + c) as usize] |= flag::MINE;
                }
            }
        }
        out
    }

    /// 그 사람에게 보이는 데이터 행: 맡은 행(행 배정 + 범위 배정이 걸친 행)과 남의 것이 아닌 행 — 담당자가 아무도
    /// 없는 행, 또는 담당자 없는 입력 칸(누구나 입력)이 있는 행. '남의 행 숨김'일 때 이 행만 보낸다.
    pub fn rows_of(&self, p: usize) -> Vec<u32> {
        let spans = |r: u32, rg: &CellRange| rg.row <= r && r <= rg.last_row();
        (self.header_rows..self.rows)
            .filter(|&r| {
                if self.row_owner(r) == Some(p) || self.ranges.iter().any(|(rg, q)| *q == p && spans(r, rg)) {
                    return true;
                }
                if self.row_owner(r).is_some() {
                    return false; // 남이 맡은 행
                }
                if !self.ranges.iter().any(|(rg, _)| spans(r, rg)) {
                    return true; // 아무도 맡지 않은 행
                }
                // 남의 범위 배정이 일부 걸친 행: 담당자 없는 입력 칸이 있으면 함께 입력하는 행이다.
                (0..self.cols).any(|c| {
                    let at = CellRef::new(r, c);
                    self.flags(at) & flag::INPUT != 0 && self.is_open(at)
                })
            })
            .collect()
    }

    /// 필수 입력 진행률.
    pub fn progress(&self, model: &SheetModel) -> ProgressStats {
        let mut stats = ProgressStats {
            by_person: self
                .roster
                .iter()
                .enumerate()
                .map(|(i, name)| PersonProgress {
                    name: name.clone(),
                    rows: self.row_owner.iter().filter(|&&p| p == i as i32).count() as u32,
                    required: 0,
                    filled: 0,
                })
                .collect(),
            by_column: (0..self.cols).map(|col| ColumnProgress { col, required: 0, filled: 0 }).collect(),
            ..ProgressStats::default()
        };
        for r in self.header_rows..self.rows {
            for c in 0..self.cols {
                let at = CellRef::new(r, c);
                if self.flags(at) & flag::REQUIRED == 0 {
                    continue;
                }
                let filled = model.cell(at).is_some_and(|cell| !cell.v.is_empty());
                let f = u32::from(filled);
                stats.required += 1;
                stats.filled += f;
                let col = &mut stats.by_column[c as usize];
                col.required += 1;
                col.filled += f;
                let owners = self.owners(at);
                if owners.is_empty() {
                    stats.unassigned += 1;
                }
                for p in owners {
                    let person = &mut stats.by_person[p];
                    person.required += 1;
                    person.filled += f;
                }
            }
        }
        stats
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PersonProgress {
    pub name: String,
    /// 행 배정으로 맡은 행 수.
    pub rows: u32,
    pub required: u32,
    pub filled: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, TS)]
#[ts(export)]
pub struct ColumnProgress {
    pub col: u32,
    pub required: u32,
    pub filled: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProgressStats {
    pub required: u32,
    pub filled: u32,
    /// 담당자가 없는 필수 셀(누구나 입력할 수 있다). 담당자별 진행률에는 들지 않는다.
    pub unassigned: u32,
    pub by_person: Vec<PersonProgress>,
    pub by_column: Vec<ColumnProgress>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::tests::{assigned, budget};
    use crate::config::{CellException, RangeAssignment};

    fn at(a: &str) -> CellRef {
        CellRef::parse(a).unwrap()
    }

    fn setup() -> (SheetModel, CollectConfig) {
        let m = budget();
        let mut cfg = assigned(&m);
        cfg.columns[5].required = true; // F 신청 금액
        cfg.columns[7].required = true; // H 산출 근거
        (m, cfg)
    }

    #[test]
    fn roster_and_row_owners_follow_merged_key_column() {
        let (m, cfg) = setup();
        let a = Access::compile(&m, &cfg);
        assert_eq!(a.roster(), ["이서연", "최도윤", "김민지"]);
        // C3:C4 병합 → 3·4행 모두 이서연
        assert_eq!(
            (2..8).map(|r| a.row_owner(r)).collect::<Vec<_>>(),
            [Some(0), Some(0), Some(1), Some(1), Some(2), Some(2)]
        );
        assert_eq!(a.row_owner(0), None);
    }

    #[test]
    fn editable_is_assigned_rows_intersect_input_minus_locked_and_formula() {
        let (m, cfg) = setup();
        let a = Access::compile(&m, &cfg);
        let lee = Viewer::Person(a.person("이서연").unwrap());
        let choi = Viewer::Person(a.person("최도윤").unwrap());
        assert!(a.can_edit(lee, at("F3")));
        assert!(!a.can_edit(choi, at("F3"))); // 남의 행
        assert!(a.can_edit(choi, at("F5")));
        assert!(!a.can_edit(lee, at("E3"))); // 입력 금지 열
        assert!(!a.can_edit(lee, at("G3"))); // 수식
        assert!(!a.can_edit(Viewer::Host, at("F2"))); // 머리글
        assert!(a.can_edit(Viewer::Host, at("F5")));
        assert_eq!(a.flags(at("F3")) & (flag::INPUT | flag::REQUIRED), flag::INPUT | flag::REQUIRED);
        assert_eq!(a.flags(at("I3")) & flag::REQUIRED, 0);
    }

    #[test]
    fn exceptions_and_range_assignments() {
        let (m, mut cfg) = setup();
        cfg.exceptions.push(CellException {
            range: CellRange::parse("F6").unwrap(),
            kind: ExceptionKind::Locked,
            note: "계약 확정값".into(),
        });
        cfg.exceptions.push(CellException {
            range: CellRange::parse("E3:E4").unwrap(),
            kind: ExceptionKind::Input,
            note: String::new(),
        });
        cfg.exceptions.push(CellException {
            range: CellRange::parse("I3").unwrap(),
            kind: ExceptionKind::Required,
            note: String::new(),
        });
        // 입력 금지 예외는 같은 셀의 입력 예외보다 우선한다.
        cfg.exceptions.push(CellException {
            range: CellRange::parse("E4").unwrap(),
            kind: ExceptionKind::Locked,
            note: String::new(),
        });
        cfg.assignment.ranges.push(RangeAssignment {
            range: CellRange::parse("I3:I8").unwrap(),
            person: "이서연".into(),
            note: "집행 시기 일괄".into(),
        });
        cfg.assignment.ranges.push(RangeAssignment {
            range: CellRange::parse("H7:H8").unwrap(),
            person: "한지우".into(),
            note: String::new(),
        });
        let a = Access::compile(&m, &cfg);
        let lee = Viewer::Person(a.person("이서연").unwrap());
        let choi = Viewer::Person(a.person("최도윤").unwrap());
        let kim = Viewer::Person(a.person("김민지").unwrap());
        let han = Viewer::Person(a.person("한지우").unwrap());

        assert!(!a.can_edit(choi, at("F6")) && a.flags(at("F6")) & flag::EXCEPTION != 0);
        assert!(a.can_edit(lee, at("E3")) && !a.can_edit(lee, at("E4")));
        assert_ne!(a.flags(at("I3")) & flag::REQUIRED, 0);
        // 범위 배정은 행 배정에 더해진다: I열은 행 담당자와 이서연 모두 입력 가능.
        assert!(a.can_edit(lee, at("I7")) && a.can_edit(kim, at("I7")));
        assert!(a.can_edit(han, at("H7")) && !a.can_edit(han, at("F7")));
        assert_eq!(a.owners(at("I7")), [2, 0]);
        // 명단에 범위 배정 받은 사람이 더해진다.
        assert_eq!(a.roster().last().map(String::as_str), Some("한지우"));
        assert_eq!(a.rows_of(a.person("한지우").unwrap()), [6, 7]);
        assert_eq!(a.rows_of(a.person("이서연").unwrap()), [2, 3, 4, 5, 6, 7]);
    }

    #[test]
    fn cells_without_any_assignee_are_open_to_everyone() {
        let (mut m, mut cfg) = setup();
        // 김민지 행(7·8행, C7:C8 병합)의 담당자를 비운다 → 그 행은 담당자 없음.
        m.cell_mut(at("C7")).unwrap().v = CellValue::Empty;
        cfg.assignment.extra_people.push("김민지".into());
        cfg.assignment.ranges.push(RangeAssignment {
            range: CellRange::parse("I7").unwrap(),
            person: "최도윤".into(),
            note: String::new(),
        });
        let a = Access::compile(&m, &cfg);
        let lee = Viewer::Person(a.person("이서연").unwrap());
        let choi = Viewer::Person(a.person("최도윤").unwrap());
        assert_eq!(a.row_owner(6), None);
        assert!(a.is_open(at("F7")) && !a.is_open(at("I7")) && !a.is_open(at("F3")));
        // 담당자 없는 입력 칸은 누구나, 범위 배정이 걸린 칸은 그 사람만.
        assert!(a.can_edit(lee, at("F7")) && a.can_edit(choi, at("F7")));
        assert!(a.can_edit(choi, at("I7")) && !a.can_edit(lee, at("I7")));
        assert!(a.can_edit(lee, at("I8")));
        // 입력 금지·수식 칸은 여전히 아무도 못 쓴다.
        assert!(!a.can_edit(lee, at("E7")) && !a.can_edit(lee, at("G7")));
        // 남의 행 숨김: 담당자 없는 칸이 있는 행은 모두에게 보인다.
        assert_eq!(a.rows_of(a.person("이서연").unwrap()), [2, 3, 6, 7]);
        // 담당자 없는 필수 칸(F7·F8·H7·H8)은 누구의 진행률에도 들지 않는다.
        assert_eq!(a.progress(&m).unassigned, 4);
    }

    #[test]
    fn without_any_assignment_everyone_can_edit_every_input_cell() {
        let m = budget();
        let mut cfg = CollectConfig::suggest(&m);
        cfg.assignment.extra_people = vec!["가".into(), "나".into()];
        cfg.permissions.others_rows = crate::config::OthersRows::Hidden;
        let a = Access::compile(&m, &cfg);
        for p in 0..2 {
            assert!(a.can_edit(Viewer::Person(p), at("F3")) && a.can_edit(Viewer::Person(p), at("H8")));
            assert!(!a.can_edit(Viewer::Person(p), at("E3")));
            assert_eq!(a.rows_of(p), (2..8).collect::<Vec<_>>());
        }
    }

    #[test]
    fn merged_hidden_and_header_cells_are_never_input() {
        let (mut m, mut cfg) = setup();
        m.hidden_rows = vec![3];
        cfg.columns[2].role = ColumnRole::Input; // C 담당자(병합 C3:C4)를 입력으로 바꿔도
        let a = Access::compile(&m, &cfg);
        assert_ne!(a.flags(at("C3")) & flag::INPUT, 0);
        assert_ne!(a.flags(at("C4")) & flag::COVERED, 0);
        assert_eq!(a.flags(at("C4")) & flag::INPUT, 0);
        assert_eq!(a.flags(at("F4")) & flag::INPUT, 0); // 숨긴 행
        assert_ne!(a.flags(at("F4")) & flag::HIDDEN, 0);
        assert_ne!(a.flags(at("A1")) & flag::HEADER, 0);
    }

    #[test]
    fn progress_counts_required_by_person_and_column() {
        let (m, cfg) = setup();
        let a = Access::compile(&m, &cfg);
        let p = a.progress(&m);
        // F·H열 × 데이터 6행 = 12칸, 채워진 칸: F4, F7, H4, H7
        assert_eq!((p.required, p.filled, p.unassigned), (12, 4, 0));
        assert_eq!(p.by_column[5], ColumnProgress { col: 5, required: 6, filled: 2 });
        assert_eq!(p.by_person[0], PersonProgress { name: "이서연".into(), rows: 2, required: 4, filled: 2 });
        assert_eq!(p.by_person[1].filled, 0);
    }

    #[test]
    fn viewer_flags_mark_mine() {
        let (m, cfg) = setup();
        let a = Access::compile(&m, &cfg);
        let kim = a.person("김민지").unwrap();
        let flags = a.viewer_flags(Viewer::Person(kim));
        let i = |s: &str| (at(s).row * m.cols + at(s).col) as usize;
        assert_ne!(flags[i("F7")] & flag::MINE, 0);
        assert_eq!(flags[i("F3")] & flag::MINE, 0);
        assert_ne!(Access::compile(&m, &cfg).viewer_flags(Viewer::Host)[i("F3")] & flag::MINE, 0);
    }
}
