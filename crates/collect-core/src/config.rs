//! 취합 설정(1b): 열마다 역할·입력 형식, 셀 단위 예외, 담당자 배정, 참여자 권한.
//!
//! 좌표는 모두 [`SheetModel`] 기준(불러온 범위 왼쪽 위 = 0,0)이다. 실제 판정은 [`crate::access::Access`]가 한다.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::a1::{CellRange, CellRef};
use crate::detect::{TableGuess, assignee_candidates};
use crate::sheet::{CellValue, MergeIndex, SheetModel, ValidationRule};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ColumnRole {
    /// 입력 영역: 배정된 참여자가 값을 넣는다.
    Input,
    /// 입력 금지: 읽기 전용(키 열·수식·확정값).
    Locked,
}

/// 참여자 입력 형식. 서버가 입력값을 이 형식으로 검사한다(4단계).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum InputFormat {
    Text,
    /// 숫자. `unit`은 표시용 단위(예: 천원).
    Number {
        unit: String,
    },
    /// 목록에서 고르기(Excel 목록 유효성에서 가져오거나 직접 입력).
    List {
        items: Vec<String>,
    },
    Date,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ColumnRule {
    pub role: ColumnRole,
    pub required: bool,
    /// 조회조건: 참여자 화면에 드롭다운 필터로 노출한다.
    pub filter: bool,
    pub format: InputFormat,
    /// 참여자에게 보일 안내(예: "천원 단위, 숫자만").
    pub hint: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ExceptionKind {
    /// 이 셀만 입력 금지(예: 계약 확정값). 언제나 다른 설정보다 우선한다.
    Locked,
    /// 입력 금지 열 안에서 이 셀만 입력 가능.
    Input,
    /// 이 셀도 필수.
    Required,
}

/// 열 단위 역할 위에 덮어쓰는 셀 단위 예외.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CellException {
    pub range: CellRange,
    pub kind: ExceptionKind,
    pub note: String,
}

/// 행 배정에 더해 특정 범위를 다른 사람에게도 준다(예: I열 집행 시기 일괄 → 이서연).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RangeAssignment {
    pub range: CellRange,
    pub person: String,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Assignment {
    /// 행 배정 기준 열: 이 열의 값(병합 고려)이 접속 이름과 같으면 그 행을 맡는다. `None`이면 행 배정 없음.
    #[ts(optional)]
    pub key_col: Option<u32>,
    /// 기준 열에 없지만 명단에 넣을 사람(범위 배정 등).
    pub extra_people: Vec<String>,
    pub ranges: Vec<RangeAssignment>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum FormatPolicy {
    /// 참여자는 서식을 바꿀 수 없다.
    None,
    FillOnly,
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum OthersRows {
    /// 남의 행은 회색 읽기 전용으로 보인다.
    ReadOnly,
    /// 남의 행은 아예 보내지 않는다.
    Hidden,
    /// 담당자 란은 '내 행' 필터용 표시일 뿐, 입력 영역이면 누구나 입력할 수 있다.
    Editable,
}

/// 마감 시각이 되면 하는 일(1c '마감 시각이 되면'). 마감 시각은 호스트 PC 시계로 잰다.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum DeadlineAction {
    /// 참여자 입력을 막는다(접속은 그대로, 보기만 한다).
    #[default]
    ReadOnly,
    /// 이 업무의 공유를 닫는다(참여자 연결이 끊긴다). 같은 마감 시각으로는 한 번만 닫고, 호스트가 다시 열면 그대로 둔다.
    StopSharing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Permissions {
    pub format: FormatPolicy,
    pub others_rows: OthersRows,
    pub filter_sort: bool,
    pub add_rows: bool,
    /// 참여자가 자기가 추가한 행을 지울 수 있다(1b 참여자 권한 '행 삭제'). 기본은 금지.
    #[serde(default)]
    pub delete_rows: bool,
    /// 마감 시각이 되면 읽기 전용으로 바꿀지 공유를 닫을지. 예전 '마감 후 자동 읽기 전용' 설정은 읽지 않는다(읽기 전용).
    #[serde(default)]
    pub deadline_action: DeadlineAction,
    /// 참여자가 열 너비·행 높이를 바꿀 수 있다(1b 참여자 권한 '크기 조절', 바꾸면 모두에게 보인다). 기본은 금지.
    #[serde(default)]
    pub resize: bool,
}

impl Default for Permissions {
    fn default() -> Self {
        Self {
            format: FormatPolicy::None,
            others_rows: OthersRows::ReadOnly,
            filter_sort: true,
            add_rows: false,
            delete_rows: false,
            deadline_action: DeadlineAction::ReadOnly,
            resize: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CollectConfig {
    pub title: String,
    pub description: String,
    /// 요청자(1c '내 이름', 필수). 1e '박지훈 (기획팀) 님의 엑셀 취합 요청'이고, 호스트 PC가 IP 명단에 없으면
    /// 공지·강조·채팅의 보낸 사람 이름이다.
    #[serde(default)]
    pub requester: String,
    /// 마감(로컬 시각 `YYYY-MM-DDTHH:MM`).
    #[ts(optional)]
    pub deadline: Option<String>,
    /// 모델 열마다 하나.
    pub columns: Vec<ColumnRule>,
    pub exceptions: Vec<CellException>,
    pub assignment: Assignment,
    pub permissions: Permissions,
}

/// 여러 시트를 함께 취합할 때 모든 시트가 같이 쓰는 설정: 제목·안내·요청자·마감·마감 동작과 직접 넣은 명단.
/// 첫 시트의 설정이 주인이고, 세션이 바뀔 때마다 다른 시트 설정에 그대로 옮겨 적는다(나머지 설정은 시트마다 따로).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SessionCommon {
    pub title: String,
    pub description: String,
    pub requester: String,
    pub deadline: Option<String>,
    pub deadline_action: DeadlineAction,
    pub extra_people: Vec<String>,
}

impl SessionCommon {
    pub fn of(c: &CollectConfig) -> Self {
        Self {
            title: c.title.clone(),
            description: c.description.clone(),
            requester: c.requester.clone(),
            deadline: c.deadline.clone(),
            deadline_action: c.permissions.deadline_action,
            extra_people: c.assignment.extra_people.clone(),
        }
    }

    /// `c`의 공통 설정을 이것으로 바꾼다. 바뀐 것이 있으면 `true`.
    pub fn write_to(&self, c: &mut CollectConfig) -> bool {
        if Self::of(c) == *self {
            return false;
        }
        c.title.clone_from(&self.title);
        c.description.clone_from(&self.description);
        c.requester.clone_from(&self.requester);
        c.deadline.clone_from(&self.deadline);
        c.permissions.deadline_action = self.deadline_action;
        c.assignment.extra_people.clone_from(&self.extra_people);
        true
    }
}

impl CollectConfig {
    /// 불러온 시트를 보고 설정 초안을 만든다(호스트가 1b에서 고친다).
    ///
    /// - 제목: 엑셀 파일 이름(확장자 제외).
    /// - 행 배정 기준: 정하지 않는다(호스트가 '담당자 배정'에서 고른다). 후보 1순위([`suggested_key_col`])는 아래 판단에만 쓴다.
    /// - 열 역할: 모두 입력 영역. 잠글 열은 호스트가 1b에서 정한다(수식 칸은 열 역할과 상관없이 입력할 수 없다).
    /// - 조회조건: 데이터가 거의 다 채워진 글자 열 중 값이 2~50가지로 반복되는 열(본부·부서·비목 같은 키,
    ///   수식 열·배정 기준 후보 열 제외).
    /// - 형식: 목록 유효성 → 목록, 숫자가 있거나 숫자 표시 형식 → 숫자(머리글 괄호 안을 단위로), 날짜 형식 → 날짜.
    pub fn suggest(model: &SheetModel) -> Self {
        let index = MergeIndex::new(model);
        let key_col = suggested_key_col(model);
        let hidden: BTreeSet<u32> = model.hidden_rows.iter().copied().collect();
        let data_rows: Vec<u32> = (model.header_rows..model.rows).filter(|r| !hidden.contains(r)).collect();

        let columns = (0..model.cols)
            .map(|c| {
                let mut filled = 0usize;
                let mut numbers = 0usize;
                let mut formulas = 0usize;
                let mut texts = BTreeSet::new();
                let mut text_count = 0usize;
                for &r in &data_rows {
                    let at = CellRef::new(r, c);
                    if model.cell(at).is_some_and(|cell| cell.f.is_some()) {
                        formulas += 1;
                    }
                    match model.effective_value_in(&index, at) {
                        Some(CellValue::Empty) | None => {}
                        Some(CellValue::Number(_)) => {
                            filled += 1;
                            numbers += 1;
                        }
                        Some(CellValue::Text(s)) => {
                            filled += 1;
                            text_count += 1;
                            texts.insert(s.trim().to_string());
                        }
                        Some(_) => filled += 1,
                    }
                }
                let n = data_rows.len().max(1);
                let is_formula = formulas * 2 > n;
                let is_key = key_col == Some(c);
                let mostly_empty = (n - filled.min(n)) * 10 >= n * 3;
                let filter = !mostly_empty
                    && !is_key
                    && !is_formula
                    && numbers == 0
                    && (2..=50).contains(&texts.len())
                    && texts.len() < text_count;
                let format = suggest_format(model, c, numbers > 0, &header_texts(model, &index, c));
                let hint = default_hint(&format);
                ColumnRule { role: ColumnRole::Input, required: false, filter, format, hint }
            })
            .collect();

        CollectConfig {
            title: default_title(model),
            description: String::new(),
            requester: String::new(),
            deadline: None,
            columns,
            exceptions: Vec::new(),
            assignment: Assignment::default(),
            permissions: Permissions::default(),
        }
    }
}

impl ColumnRule {
    /// 앱에서 붙인 열의 규칙: 입력 열, 글자, 필수·조회조건 아님.
    pub fn input() -> Self {
        Self { role: ColumnRole::Input, required: false, filter: false, format: InputFormat::Text, hint: String::new() }
    }
}

/// 범위 목록에서 지운 줄을 당기고, 다 지워진 범위는 뺀다.
fn retain_ranges<T>(
    list: &mut Vec<T>,
    range: impl Fn(&mut T) -> &mut CellRange,
    map: impl Fn(&CellRange) -> Option<CellRange>,
) {
    list.retain_mut(|item| {
        let r = range(item);
        match map(r) {
            Some(m) => {
                *r = m;
                true
            }
            None => false,
        }
    });
}

impl CollectConfig {
    /// 표 오른쪽에 열을 붙였다: 열 규칙을 시트 열 수(`cols`)만큼 입력 열로 채운다.
    pub fn on_cols_appended(&mut self, cols: u32) {
        self.columns.resize(cols as usize, ColumnRule::input());
    }

    /// 행을 지웠다: 셀 예외·범위 배정을 당기거나 줄이고, 다 지워진 것은 뺀다.
    pub fn on_rows_removed(&mut self, from: u32, count: u32) {
        retain_ranges(&mut self.exceptions, |e| &mut e.range, |r| r.without_rows(from, count));
        retain_ranges(&mut self.assignment.ranges, |a| &mut a.range, |r| r.without_rows(from, count));
    }

    /// 열을 지웠다: 열 규칙을 빼고, 셀 예외·범위 배정·행 배정 기준 열을 당긴다(기준 열을 지웠으면 행 배정 없음).
    pub fn on_cols_removed(&mut self, from: u32, count: u32) {
        let end = (from.saturating_add(count) as usize).min(self.columns.len());
        if (from as usize) < end {
            self.columns.drain(from as usize..end);
        }
        retain_ranges(&mut self.exceptions, |e| &mut e.range, |r| r.without_cols(from, count));
        retain_ranges(&mut self.assignment.ranges, |a| &mut a.range, |r| r.without_cols(from, count));
        self.assignment.key_col = self.assignment.key_col.and_then(|k| crate::a1::shift_after_removal(k, from, count));
    }

    /// 담당자를 배정했는가(행 배정 기준 열이나 범위 배정). 없으면 참여자 '내 담당만'이 뜻이 없어 숨긴다.
    pub fn has_assignment(&self) -> bool {
        self.assignment.key_col.is_some() || !self.assignment.ranges.is_empty()
    }
}

/// 행 배정 기준 열 후보 1순위(담당·작성자 같은 머리글, 사람 이름 모양의 값).
pub fn suggested_key_col(model: &SheetModel) -> Option<u32> {
    let table = TableGuess { range: model.bounds(), header_rows: model.header_rows };
    assignee_candidates(model, &table).first().map(|c| c.col)
}

/// 엑셀 파일 이름에서 확장자를 뺀 것(`budget.xlsx` → `budget`). 비면 시트 이름.
fn default_title(model: &SheetModel) -> String {
    let name = model.workbook.trim();
    let stem = match name.rfind('.') {
        Some(i) if i > 0 => &name[..i],
        _ => name,
    };
    if stem.trim().is_empty() { model.sheet.clone() } else { stem.trim().to_string() }
}

/// 열의 모든 머리글 행 글자(병합 고려, 위에서부터).
fn header_texts(model: &SheetModel, index: &MergeIndex, col: u32) -> Vec<String> {
    (0..model.header_rows)
        .filter_map(|r| match model.effective_value_in(index, CellRef::new(r, col)) {
            Some(CellValue::Text(s)) if !s.trim().is_empty() => Some(s.trim().to_string()),
            _ => None,
        })
        .collect()
}

/// 머리글의 괄호 안 글자: `"금액 (천원)"` → `"천원"`.
fn unit_in(text: &str) -> Option<String> {
    let open = text.rfind(['(', '（'])?;
    let rest = &text[open + text[open..].chars().next()?.len_utf8()..];
    let close = rest.find([')', '）'])?;
    let unit = rest[..close].trim();
    (!unit.is_empty() && unit.chars().count() <= 6).then(|| unit.to_string())
}

fn is_date_format(fmt: &str) -> bool {
    let f = fmt.to_lowercase();
    !f.contains('#')
        && !f.contains('0')
        && (f.contains("yy") || f.contains("mm/dd") || f.contains("m/d") || f.contains("dd"))
}

fn is_number_format(fmt: &str) -> bool {
    fmt.contains('0') || fmt.contains('#')
}

fn suggest_format(model: &SheetModel, col: u32, has_numbers: bool, headers: &[String]) -> InputFormat {
    let data = CellRange::new(model.header_rows, col, model.rows.saturating_sub(model.header_rows).max(1), 1);
    for v in &model.validations {
        if let ValidationRule::List { items, .. } = &v.rule
            && v.range.intersect(&data).is_some()
        {
            return InputFormat::List { items: items.clone() };
        }
    }
    // 데이터 첫 행의 표시 형식을 그 열의 형식으로 본다.
    let fmt = model
        .cell(CellRef::new(model.header_rows.min(model.rows.saturating_sub(1)), col))
        .and_then(|c| model.styles.get(c.s as usize))
        .map(|s| s.number_format.as_str())
        .unwrap_or("");
    if is_date_format(fmt) {
        return InputFormat::Date;
    }
    if has_numbers || is_number_format(fmt) {
        let unit = headers.iter().rev().find_map(|h| unit_in(h)).unwrap_or_default();
        return InputFormat::Number { unit };
    }
    InputFormat::Text
}

pub fn default_hint(format: &InputFormat) -> String {
    match format {
        InputFormat::Number { unit } if !unit.is_empty() => format!("{unit} 단위, 숫자만"),
        InputFormat::Number { .. } => "숫자만".into(),
        InputFormat::List { .. } => "목록에서 고르세요".into(),
        InputFormat::Date => "날짜 (예: 2026-10-10)".into(),
        InputFormat::Text => String::new(),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::sheet::{Cell, CellStyle, Validation};

    /// 예산신청서를 줄인 모델: 머리글 2행 + 데이터 6행, A~I열.
    pub(crate) fn budget() -> SheetModel {
        let rows: Vec<Vec<&str>> = vec![
            vec!["기본 정보", "", "", "", "금액 (천원)", "", "", "세부", ""],
            vec!["본부", "부서", "담당자", "비목", "전년 실적", "신청 금액", "증감률", "산출 근거", "집행 시기"],
            vec!["경영지원", "인사팀", "이서연", "교육훈련비", "9200", "", "=F3/E3-1", "", ""],
            vec!["", "", "", "회의비", "4400", "5000", "=F4/E4-1", "간담회 4회", "연중"],
            vec!["", "재무팀", "최도윤", "출장비", "1800", "", "=F5/E5-1", "", ""],
            vec!["", "", "", "회의비", "2000", "", "=F6/E6-1", "", ""],
            vec!["영업", "국내영업팀", "김민지", "출장비", "3000", "3500", "=F7/E7-1", "거점 방문", "1분기"],
            vec!["", "", "", "회의비", "1200", "", "=F8/E8-1", "", ""],
        ];
        let cols = 9u32;
        let mut cells = Vec::new();
        for row in &rows {
            for (c, raw) in row.iter().enumerate() {
                let mut cell = Cell { s: u32::from(c == 4 || c == 5), ..Default::default() };
                if let Some(f) = raw.strip_prefix('=') {
                    cell.f = Some(format!("={f}"));
                    cell.v = CellValue::Number(0.1);
                } else if let Ok(n) = raw.parse::<f64>() {
                    cell.v = CellValue::Number(n);
                } else if !raw.is_empty() {
                    cell.v = CellValue::Text((*raw).into());
                }
                cells.push(cell);
            }
        }
        let merges =
            ["A1:D1", "E1:G1", "H1:I1", "A3:A6", "B3:B4", "C3:C4", "B5:B6", "C5:C6", "A7:A8", "B7:B8", "C7:C8"];
        SheetModel {
            workbook: "budget.xlsx".into(),
            sheet: "예산신청서".into(),
            origin: CellRef::new(0, 0),
            rows: rows.len() as u32,
            cols,
            cells,
            styles: vec![CellStyle::default(), CellStyle { number_format: "#,##0".into(), ..Default::default() }],
            merges: merges.iter().map(|a| CellRange::parse(a).unwrap()).collect(),
            col_widths: vec![60.0; cols as usize],
            row_heights: vec![15.0; rows.len()],
            header_rows: 2,
            frozen_rows: 2,
            frozen_cols: 4,
            validations: vec![Validation {
                range: CellRange::parse("I3:I8").unwrap(),
                rule: ValidationRule::List {
                    source: "=코드표!$A$2:$A$6".into(),
                    items: vec!["1분기".into(), "2분기".into(), "연중".into()],
                },
            }],
            hidden_rows: vec![],
            hidden_cols: vec![],
            added_rows: Vec::new(),
            cancelled_rows: Vec::new(),
            layout: 0,
        }
    }

    /// 설정 초안에서 호스트가 와이어프레임 1b처럼 키·실적 열(A~E)과 수식 열(G)을 잠근 것(잠금 열이 필요한 시험용).
    pub(crate) fn configured(m: &SheetModel) -> CollectConfig {
        let mut cfg = CollectConfig::suggest(m);
        for c in [0, 1, 2, 3, 4, 6] {
            cfg.columns[c].role = ColumnRole::Locked;
        }
        cfg
    }

    /// [`configured`] + 행 배정 기준을 추천 열(C 담당자)로 정한 것(배정이 필요한 시험용).
    pub(crate) fn assigned(m: &SheetModel) -> CollectConfig {
        let mut cfg = configured(m);
        cfg.assignment.key_col = suggested_key_col(m);
        cfg
    }

    #[test]
    fn suggests_wireframe_like_defaults() {
        let m = budget();
        let cfg = CollectConfig::suggest(&m);
        assert_eq!(cfg.title, "budget");
        assert_eq!(cfg.assignment.key_col, None); // 담당자 배정은 호스트가 정한다
        assert_eq!(suggested_key_col(&m), Some(2));
        // 처음엔 모든 열이 입력(잠글 열은 호스트가 정한다). G 수식 칸은 열 역할과 상관없이 입력할 수 없다.
        assert!(cfg.columns.iter().all(|c| c.role == ColumnRole::Input));
        let filters: Vec<u32> = (0..9).filter(|&c| cfg.columns[c as usize].filter).collect();
        assert_eq!(filters, [0, 1, 3]); // 본부·부서·비목 (담당자는 배정 기준이라 제외)
        assert_eq!(cfg.columns[5].format, InputFormat::Number { unit: "천원".into() });
        assert_eq!(cfg.columns[5].hint, "천원 단위, 숫자만");
        assert!(matches!(&cfg.columns[8].format, InputFormat::List { items } if items.len() == 3));
        assert_eq!(cfg.columns[7].format, InputFormat::Text);
        assert!(cfg.columns.iter().all(|c| !c.required));
        assert_eq!(cfg.permissions, Permissions::default());
    }

    #[test]
    fn session_common_is_copied_between_sheets() {
        let m = budget();
        let mut first = CollectConfig::suggest(&m);
        first.title = "예산".into();
        first.deadline = Some("2026-10-31T18:00".into());
        first.permissions.deadline_action = DeadlineAction::StopSharing;
        first.assignment.extra_people.push("정하늘".into());
        let mut other = CollectConfig::suggest(&m);
        other.permissions.add_rows = true; // 시트별 권한은 그대로
        let common = SessionCommon::of(&first);
        assert!(common.write_to(&mut other));
        assert!(!common.write_to(&mut other), "같으면 바뀐 것 없음");
        assert_eq!((other.title.as_str(), other.deadline.as_deref()), ("예산", Some("2026-10-31T18:00")));
        assert!(other.permissions.deadline_action == DeadlineAction::StopSharing && other.permissions.add_rows);
        assert_eq!(other.assignment.extra_people, ["정하늘"]);
    }

    #[test]
    fn title_is_file_stem() {
        let mut m = budget();
        assert_eq!(default_title(&m), "budget");
        m.workbook = "2026 예산.v2.xlsm".into();
        assert_eq!(default_title(&m), "2026 예산.v2");
        m.workbook = "통합 문서1".into(); // 저장 안 한 통합문서
        assert_eq!(default_title(&m), "통합 문서1");
        m.workbook = String::new();
        assert_eq!(default_title(&m), "예산신청서");
    }

    #[test]
    fn unit_parsing() {
        assert_eq!(unit_in("금액 (천원)").as_deref(), Some("천원"));
        assert_eq!(unit_in("시간（h）").as_deref(), Some("h"));
        assert_eq!(unit_in("신청 금액"), None);
        assert_eq!(unit_in("비고 ()"), None);
    }

    #[test]
    fn config_json_shape() {
        let cfg = CollectConfig::suggest(&budget());
        let json = serde_json::to_value(&cfg).unwrap();
        assert_eq!(json["columns"][5]["format"], serde_json::json!({"kind": "number", "unit": "천원"}));
        assert_eq!(json["permissions"]["othersRows"], "readOnly");
        assert!(json["assignment"]["keyCol"].is_null());
        let back: CollectConfig = serde_json::from_value(json).unwrap();
        assert_eq!(back, cfg);
    }

    #[test]
    fn removed_lines_move_exceptions_assignments_and_key_column() {
        let m = budget();
        let mut cfg = assigned(&m);
        let cols = cfg.columns.len() as u32;
        cfg.on_cols_appended(cols + 2);
        assert_eq!(cfg.columns.len() as u32, cols + 2);
        assert_eq!(cfg.columns[cols as usize], ColumnRule::input());
        cfg.exceptions.push(CellException {
            range: CellRange::new(8, 0, 2, cols + 2),
            kind: ExceptionKind::Input,
            note: String::new(),
        });
        cfg.assignment.ranges.push(RangeAssignment {
            range: CellRange::new(9, cols + 1, 1, 1),
            person: "이서연".into(),
            note: String::new(),
        });
        cfg.on_rows_removed(8, 1);
        assert_eq!(cfg.exceptions[0].range, CellRange::new(8, 0, 1, cols + 2));
        assert_eq!(cfg.assignment.ranges[0].range, CellRange::new(8, cols + 1, 1, 1));
        cfg.on_cols_removed(cols, 1);
        assert_eq!(cfg.columns.len() as u32, cols + 1);
        assert_eq!(cfg.assignment.ranges[0].range.col, cols);
        assert_eq!(cfg.assignment.key_col, Some(2), "앞쪽 기준 열은 그대로");
        cfg.on_rows_removed(8, 1);
        assert!(cfg.exceptions.is_empty() && cfg.assignment.ranges.is_empty(), "다 지워진 범위는 뺀다");
        cfg.on_cols_removed(0, 1);
        assert_eq!(cfg.assignment.key_col, Some(1));
        assert!(cfg.has_assignment());
        cfg.on_cols_removed(1, 1);
        assert!(!cfg.has_assignment(), "기준 열을 지우면 행 배정 없음");
    }
}
