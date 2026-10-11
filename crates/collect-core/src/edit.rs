//! 1b 화면의 설정 편집 동작. 화면은 [`ConfigEdit`]를 보내고, 판정과 결과 계산은 Rust가 한다.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::a1::CellRange;
use crate::config::{
    CellException, CollectConfig, ColumnRole, ColumnRule, ExceptionKind, Permissions, RangeAssignment,
};
use crate::sheet::SheetModel;

/// 역할 브러시(1b 둘째 줄). 범위를 고른 뒤 누른다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Brush {
    /// 조회조건(열 단위, 켜고 끄기).
    Filter,
    Input,
    Locked,
    /// 필수(켜고 끄기).
    Required,
    /// 지우개: 범위에 걸친 예외·범위 배정을 지우고, 열 전체면 필수·조회조건도 끈다.
    Erase,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "op", rename_all = "camelCase")]
#[ts(export)]
pub enum ConfigEdit {
    Brush {
        range: CellRange,
        brush: Brush,
    },
    SetColumn {
        col: u32,
        rule: ColumnRule,
    },
    AddException {
        range: CellRange,
        kind: ExceptionKind,
        note: String,
    },
    UpdateException {
        index: u32,
        note: String,
    },
    RemoveException {
        index: u32,
    },
    SetKeyColumn {
        #[serde(default)]
        #[ts(optional)]
        col: Option<u32>,
    },
    AddPerson {
        name: String,
    },
    RemovePerson {
        name: String,
    },
    AssignRange {
        range: CellRange,
        person: String,
        note: String,
    },
    RemoveAssignment {
        index: u32,
    },
    SetPermissions {
        permissions: Permissions,
    },
    SetMeta {
        title: String,
        description: String,
        #[serde(default)]
        requester: String,
        #[serde(default)]
        #[ts(optional)]
        deadline: Option<String>,
    },
}

/// 여러 시트를 함께 취합할 때 편집이 어디에 닿나([`crate::config::SessionCommon`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditReach {
    /// 그 시트 설정만(열 역할·셀 예외·행 배정·범위 배정).
    Sheet,
    /// 모든 시트가 같이 쓰는 설정(제목·안내·마감, 직접 넣은 명단): 첫 시트에 하고 다른 시트에 옮겨 적는다.
    Common,
    /// 참여자 권한: 서식·남의 행·필터·행 추가는 그 시트에, '마감 후 읽기 전용'은 공통.
    SheetAndCommon,
}

impl ConfigEdit {
    pub fn reach(&self) -> EditReach {
        match self {
            ConfigEdit::SetMeta { .. } | ConfigEdit::AddPerson { .. } | ConfigEdit::RemovePerson { .. } => {
                EditReach::Common
            }
            ConfigEdit::SetPermissions { .. } => EditReach::SheetAndCommon,
            _ => EditReach::Sheet,
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ConfigError {
    #[error("범위가 표 밖입니다: {0}")]
    OutOfBounds(CellRange),
    #[error("머리글이 아닌 데이터 행을 골라 주세요")]
    HeaderOnly,
    #[error("없는 열입니다: {0}")]
    BadColumn(u32),
    #[error("없는 항목입니다: {0}")]
    BadIndex(u32),
    #[error("이름을 입력해 주세요")]
    EmptyName,
}

impl CollectConfig {
    pub fn apply(&mut self, model: &SheetModel, edit: ConfigEdit) -> Result<(), ConfigError> {
        let bounds = model.bounds();
        let clip = |r: CellRange| r.intersect(&bounds).ok_or(ConfigError::OutOfBounds(r));
        let data = data_area(model);
        let in_data = |r: CellRange| data.and_then(|d| r.intersect(&d)).ok_or(ConfigError::HeaderOnly);
        match edit {
            ConfigEdit::Brush { range, brush } => self.brush(model, clip(range)?, brush)?,
            ConfigEdit::SetColumn { col, mut rule } => {
                // 필수는 입력 영역에만 뜻이 있다(잠금 열은 필수를 끈다).
                if rule.role == ColumnRole::Locked {
                    rule.required = false;
                }
                *self.columns.get_mut(col as usize).ok_or(ConfigError::BadColumn(col))? = rule;
            }
            ConfigEdit::AddException { range, kind, note } => {
                self.exceptions.push(CellException { range: in_data(clip(range)?)?, kind, note });
            }
            ConfigEdit::UpdateException { index, note } => {
                self.exceptions.get_mut(index as usize).ok_or(ConfigError::BadIndex(index))?.note = note;
            }
            ConfigEdit::RemoveException { index } => {
                if index as usize >= self.exceptions.len() {
                    return Err(ConfigError::BadIndex(index));
                }
                self.exceptions.remove(index as usize);
            }
            ConfigEdit::SetKeyColumn { col } => {
                if let Some(c) = col
                    && c >= model.cols
                {
                    return Err(ConfigError::BadColumn(c));
                }
                self.assignment.key_col = col;
            }
            ConfigEdit::AddPerson { name } => {
                let name = name.trim().to_string();
                if name.is_empty() {
                    return Err(ConfigError::EmptyName);
                }
                if !self.assignment.extra_people.contains(&name) {
                    self.assignment.extra_people.push(name);
                }
            }
            ConfigEdit::RemovePerson { name } => self.assignment.extra_people.retain(|p| p.trim() != name.trim()),
            ConfigEdit::AssignRange { range, person, note } => {
                let person = person.trim().to_string();
                if person.is_empty() {
                    return Err(ConfigError::EmptyName);
                }
                self.assignment.ranges.push(RangeAssignment { range: in_data(clip(range)?)?, person, note });
            }
            ConfigEdit::RemoveAssignment { index } => {
                if index as usize >= self.assignment.ranges.len() {
                    return Err(ConfigError::BadIndex(index));
                }
                self.assignment.ranges.remove(index as usize);
            }
            ConfigEdit::SetPermissions { permissions } => self.permissions = permissions,
            ConfigEdit::SetMeta { title, description, requester, deadline } => {
                self.title = title;
                self.description = description;
                self.requester = requester;
                self.deadline = deadline.filter(|d| !d.trim().is_empty());
            }
        }
        Ok(())
    }

    /// 범위가 데이터 행 전체에 걸치거나 머리글을 포함하면 **열 단위**로, 아니면 **셀 예외**로 반영한다.
    fn brush(&mut self, model: &SheetModel, sel: CellRange, brush: Brush) -> Result<(), ConfigError> {
        let cols: Vec<usize> = (sel.col..=sel.last_col()).map(|c| c as usize).collect();
        if brush == Brush::Filter {
            let on = !cols.iter().all(|&c| self.columns[c].filter);
            for &c in &cols {
                self.columns[c].filter = on;
            }
            return Ok(());
        }
        let Some(data) = data_area(model) else { return Err(ConfigError::HeaderOnly) };
        let sel_data = sel.intersect(&data);
        let whole_columns =
            sel.row < model.header_rows || sel_data.is_some_and(|d| d.row == data.row && d.rows == data.rows);
        let column_area = CellRange::new(data.row, sel.col, data.rows, sel.cols);
        let area = if whole_columns { column_area } else { sel_data.ok_or(ConfigError::HeaderOnly)? };

        match brush {
            Brush::Filter => unreachable!(),
            Brush::Input | Brush::Locked => {
                let role = if brush == Brush::Input { ColumnRole::Input } else { ColumnRole::Locked };
                self.remove_inside(area, &[ExceptionKind::Input, ExceptionKind::Locked]);
                if whole_columns {
                    for &c in &cols {
                        self.columns[c].role = role;
                        if role == ColumnRole::Locked {
                            self.columns[c].required = false;
                        }
                    }
                } else if !cols.iter().all(|&c| self.columns[c].role == role) {
                    let kind = if role == ColumnRole::Input { ExceptionKind::Input } else { ExceptionKind::Locked };
                    self.exceptions.push(CellException { range: area, kind, note: String::new() });
                }
            }
            Brush::Required => {
                if whole_columns {
                    let on = !cols.iter().all(|&c| self.columns[c].required);
                    for &c in &cols {
                        self.columns[c].required = on;
                    }
                    self.remove_inside(area, &[ExceptionKind::Required]);
                } else if let Some(i) =
                    self.exceptions.iter().position(|e| e.kind == ExceptionKind::Required && e.range == area)
                {
                    self.exceptions.remove(i);
                } else if !cols.iter().all(|&c| self.columns[c].required) {
                    self.exceptions.push(CellException {
                        range: area,
                        kind: ExceptionKind::Required,
                        note: String::new(),
                    });
                }
            }
            Brush::Erase => {
                self.exceptions.retain(|e| e.range.intersect(&area).is_none());
                self.assignment.ranges.retain(|a| a.range.intersect(&area).is_none());
                if whole_columns {
                    for &c in &cols {
                        self.columns[c].required = false;
                        self.columns[c].filter = false;
                    }
                }
            }
        }
        Ok(())
    }

    fn remove_inside(&mut self, area: CellRange, kinds: &[ExceptionKind]) {
        self.exceptions.retain(|e| !(kinds.contains(&e.kind) && area.contains_range(&e.range)));
    }
}

/// 머리글 아래 데이터 행 전체. 데이터 행이 없으면 `None`.
fn data_area(model: &SheetModel) -> Option<CellRange> {
    let rows = model.rows.checked_sub(model.header_rows).filter(|&r| r > 0)?;
    Some(CellRange::new(model.header_rows, 0, rows, model.cols))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access::{Access, Viewer};
    use crate::config::tests::budget;
    use crate::{CellRef, config::FormatPolicy};

    fn r(a: &str) -> CellRange {
        CellRange::parse(a).unwrap()
    }

    fn brush(cfg: &mut CollectConfig, m: &SheetModel, a: &str, b: Brush) {
        cfg.apply(m, ConfigEdit::Brush { range: r(a), brush: b }).unwrap();
    }

    #[test]
    fn whole_column_brush_sets_column_rule() {
        let m = budget();
        let mut cfg = CollectConfig::suggest(&m);
        // 열 머리글 클릭 = 데이터 행 전체(F3:F8), 머리글 셀을 포함한 선택(F1:F8)도 열 단위
        brush(&mut cfg, &m, "F3:F8", Brush::Required);
        brush(&mut cfg, &m, "H1:H8", Brush::Required);
        assert!(cfg.columns[5].required && cfg.columns[7].required);
        brush(&mut cfg, &m, "E3:E8", Brush::Input);
        assert_eq!(cfg.columns[4].role, ColumnRole::Input);
        brush(&mut cfg, &m, "F3:F8", Brush::Required); // 다시 누르면 끈다
        assert!(!cfg.columns[5].required);
        assert!(cfg.exceptions.is_empty());
        // 잠그면 필수도 꺼진다(필수는 입력 영역에만).
        brush(&mut cfg, &m, "H1:H8", Brush::Locked);
        assert_eq!(cfg.columns[7].role, ColumnRole::Locked);
        assert!(!cfg.columns[7].required);
        let mut rule = cfg.columns[5].clone();
        rule.required = true;
        rule.role = ColumnRole::Locked;
        cfg.apply(&m, ConfigEdit::SetColumn { col: 5, rule }).unwrap();
        assert!(!cfg.columns[5].required);
    }

    #[test]
    fn partial_brush_makes_exceptions_and_locked_wins() {
        let m = budget();
        let mut cfg = CollectConfig::suggest(&m);
        brush(&mut cfg, &m, "F6", Brush::Locked);
        brush(&mut cfg, &m, "F3:F4", Brush::Input); // 이미 입력 열이라 예외를 만들지 않는다
        brush(&mut cfg, &m, "E3:E4", Brush::Input);
        brush(&mut cfg, &m, "H3:H5", Brush::Required);
        let kinds: Vec<_> = cfg.exceptions.iter().map(|e| (e.range.to_string(), e.kind)).collect();
        assert_eq!(
            kinds,
            [
                ("F6".to_string(), ExceptionKind::Locked),
                ("E3:E4".to_string(), ExceptionKind::Input),
                ("H3:H5".to_string(), ExceptionKind::Required),
            ]
        );
        let a = Access::compile(&m, &cfg);
        assert!(!a.can_edit(Viewer::Host, CellRef::parse("F6").unwrap()));
        assert!(a.can_edit(Viewer::Host, CellRef::parse("E3").unwrap()));

        // 같은 범위에 반대 역할을 칠하면 예전 예외를 대신한다.
        brush(&mut cfg, &m, "F6", Brush::Input);
        assert!(cfg.exceptions.iter().all(|e| e.range != r("F6")));
        // 같은 범위 필수를 다시 누르면 끈다.
        brush(&mut cfg, &m, "H3:H5", Brush::Required);
        assert!(cfg.exceptions.iter().all(|e| e.kind != ExceptionKind::Required));
    }

    #[test]
    fn filter_toggles_per_column_and_erase_clears() {
        let m = budget();
        let mut cfg = CollectConfig::suggest(&m);
        brush(&mut cfg, &m, "D3:E5", Brush::Filter); // D는 켜져 있고 E는 꺼져 있으니 둘 다 켠다
        assert!(cfg.columns[3].filter && cfg.columns[4].filter);
        brush(&mut cfg, &m, "D3:E5", Brush::Filter);
        assert!(!cfg.columns[3].filter && !cfg.columns[4].filter);

        brush(&mut cfg, &m, "F6", Brush::Locked);
        cfg.apply(&m, ConfigEdit::AssignRange { range: r("I3:I8"), person: "이서연".into(), note: String::new() })
            .unwrap();
        brush(&mut cfg, &m, "F5:I6", Brush::Erase);
        assert!(cfg.exceptions.is_empty() && cfg.assignment.ranges.is_empty());
        cfg.columns[5].required = true;
        brush(&mut cfg, &m, "F1:F8", Brush::Erase);
        assert!(!cfg.columns[5].required);
    }

    #[test]
    fn validation_errors() {
        let m = budget();
        let mut cfg = CollectConfig::suggest(&m);
        assert_eq!(
            cfg.apply(&m, ConfigEdit::Brush { range: r("Z1:Z9"), brush: Brush::Input }),
            Err(ConfigError::OutOfBounds(r("Z1:Z9")))
        );
        assert_eq!(
            cfg.apply(&m, ConfigEdit::AssignRange { range: r("A1:I2"), person: "김".into(), note: String::new() }),
            Err(ConfigError::HeaderOnly)
        );
        assert_eq!(cfg.apply(&m, ConfigEdit::AddPerson { name: "  ".into() }), Err(ConfigError::EmptyName));
        assert_eq!(cfg.apply(&m, ConfigEdit::RemoveException { index: 3 }), Err(ConfigError::BadIndex(3)));
        assert_eq!(cfg.apply(&m, ConfigEdit::SetKeyColumn { col: Some(20) }), Err(ConfigError::BadColumn(20)));
        // 범위를 넘는 선택은 표 안으로 잘라서 반영한다.
        cfg.apply(&m, ConfigEdit::Brush { range: r("H3:K8"), brush: Brush::Required }).unwrap();
        assert!(cfg.columns[7].required && cfg.columns[8].required);
    }

    #[test]
    fn edits_round_trip_through_json() {
        let m = budget();
        let mut cfg = CollectConfig::suggest(&m);
        let edits: Vec<ConfigEdit> = serde_json::from_str(
            r#"[
              {"op":"brush","range":{"row":2,"col":5,"rows":6,"cols":1},"brush":"required"},
              {"op":"setKeyColumn"},
              {"op":"addPerson","name":"한지우"},
              {"op":"setPermissions","permissions":{"format":"fillOnly","othersRows":"hidden","filterSort":false,"addRows":true,"lockAfterDeadline":false}},
              {"op":"setMeta","title":"2026년 부서별 예산 신청","description":"천원 단위","deadline":"2026-10-10T18:00"}
            ]"#,
        )
        .unwrap();
        for e in edits {
            cfg.apply(&m, e).unwrap();
        }
        assert!(cfg.columns[5].required);
        assert_eq!(cfg.assignment.key_col, None);
        assert_eq!(cfg.assignment.extra_people, ["한지우"]);
        assert_eq!(cfg.permissions.format, FormatPolicy::FillOnly);
        assert_eq!(cfg.deadline.as_deref(), Some("2026-10-10T18:00"));
    }
}
