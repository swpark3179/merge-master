//! 6단계 히스토리(1h)·활동 피드(1d): 셀 변경 기록과 그 위의 보기·복원 계산.
//!
//! 기록([`ChangeEntry`])은 세션이 셀이 바뀔 때마다 남긴다. 여기서는 기록을 묶음 단위로 보여 주고(필터·쪽 나누기),
//! CSV로 내보내고, 셀 타임라인을 만들고, 되돌릴 값과 스냅샷과의 차이를 계산한다. 좌표는 모두 모델 기준이다.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::a1::{CellRange, CellRef};
use crate::sheet::{CellValue, SheetModel};

/// 묶음이 어디서 왔나.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum BatchOrigin {
    /// 칸 하나 입력·수정(그리드 편집기, 행 폼).
    #[default]
    Edit,
    /// 붙여넣기.
    Paste,
    /// 범위 지우기(Delete·잘라내기).
    Clear,
    /// 참여자의 실행 취소·다시 실행.
    Undo,
    Redo,
    /// 호스트가 히스토리에서 되돌림('되돌리기'·'묶음 복원'·'이 값으로').
    Revert,
    /// 스냅샷으로 복원.
    Restore,
    /// 저장할 때 'Excel 값 유지'.
    Excel,
    /// 참여자가 표 아래에 행을 추가함(행 배정 기준 열에 추가한 사람 이름).
    AddRow,
}

/// 칸 하나의 변경 종류(1h 필터: 입력·수정·삭제·되돌림).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ChangeKind {
    /// 빈 칸에 값을 넣음.
    Input,
    /// 값을 다른 값으로 바꿈.
    Edit,
    /// 값을 지움.
    Delete,
    /// 되돌림·복원·실행 취소.
    Revert,
}

impl ChangeKind {
    pub fn of(origin: BatchOrigin, old: &CellValue, new: &CellValue) -> Self {
        match origin {
            BatchOrigin::Undo | BatchOrigin::Redo | BatchOrigin::Revert | BatchOrigin::Restore | BatchOrigin::Excel => {
                ChangeKind::Revert
            }
            _ if old.is_empty() && !new.is_empty() => ChangeKind::Input,
            _ if !old.is_empty() && new.is_empty() => ChangeKind::Delete,
            _ => ChangeKind::Edit,
        }
    }
}

/// 셀 변경 기록 한 줄(1g 행 이력, 1h 히스토리, 저장할 때 '변경 이력' 시트).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ChangeEntry {
    #[ts(type = "number")]
    pub seq: u64,
    /// 유닉스 시각(ms).
    #[ts(type = "number")]
    pub at_ms: i64,
    pub user: String,
    pub batch: String,
    #[serde(default)]
    pub origin: BatchOrigin,
    pub kind: ChangeKind,
    /// 세션 안 시트 순번(불러온 순서). 여러 시트를 취합하기 전의 기록은 0.
    #[serde(default)]
    pub sheet: u32,
    pub row: u32,
    pub col: u32,
    #[ts(as = "Option<CellValue>", optional)]
    #[serde(default)]
    pub old: CellValue,
    #[ts(as = "Option<CellValue>", optional)]
    #[serde(default)]
    pub new: CellValue,
}

impl ChangeEntry {
    pub fn at(&self) -> CellRef {
        CellRef::new(self.row, self.col)
    }
}

/// 히스토리 필터. 비어 있는 조건은 모두 통과.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct HistoryFilter {
    /// 이 시각(ms) 이후.
    #[ts(optional, type = "number")]
    pub since_ms: Option<i64>,
    #[ts(optional)]
    pub user: Option<String>,
    /// 이 시트(세션 안 순번)의 칸만. `range`도 이 시트 기준이다.
    #[ts(optional)]
    pub sheet: Option<u32>,
    /// 비어 있으면 모든 종류.
    pub kinds: Vec<ChangeKind>,
    /// 이 범위(모델 좌표)에 걸친 칸만.
    #[ts(optional)]
    pub range: Option<CellRange>,
    /// 이전·새 값에 이 글자가 들어 있는 칸만.
    #[ts(optional)]
    pub text: Option<String>,
}

/// 검색용 값 글자(숫자는 쉼표 없이).
pub fn plain(v: &CellValue) -> String {
    match v {
        CellValue::Empty => String::new(),
        CellValue::Number(n) if n.fract() == 0.0 && n.abs() < 1e15 => format!("{}", *n as i64),
        CellValue::Number(n) => n.to_string(),
        CellValue::Text(s) => s.clone(),
        CellValue::Bool(b) => if *b { "TRUE" } else { "FALSE" }.into(),
        CellValue::Error { err } => format!("#ERR{err}"),
    }
}

impl HistoryFilter {
    pub fn matches(&self, e: &ChangeEntry) -> bool {
        if self.since_ms.is_some_and(|t| e.at_ms < t) {
            return false;
        }
        if self.user.as_deref().is_some_and(|u| !u.is_empty() && u != e.user) {
            return false;
        }
        if self.sheet.is_some_and(|s| s != e.sheet) {
            return false;
        }
        if !self.kinds.is_empty() && !self.kinds.contains(&e.kind) {
            return false;
        }
        if self.range.is_some_and(|r| !r.contains(e.at())) {
            return false;
        }
        if let Some(t) = self.text.as_deref().map(str::trim).filter(|t| !t.is_empty())
            && !plain(&e.old).contains(t)
            && !plain(&e.new).contains(t)
        {
            return false;
        }
        true
    }
}

/// 묶음 하나(범위 지우기·붙여넣기는 여러 칸).
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BatchView {
    pub batch: String,
    #[ts(type = "number")]
    pub at_ms: i64,
    pub user: String,
    pub origin: BatchOrigin,
    /// 묶음의 시트(한 묶음은 한 시트 안에서만 바꾼다).
    pub sheet: u32,
    /// 묶음 안 칸들을 덮는 범위.
    pub range: CellRange,
    /// 묶음 전체 칸 수(필터에 걸린 칸만 `entries`에 든다).
    pub count: u32,
    /// 삭제가 든 여러 칸 묶음이나 큰 묶음(활동 피드에서 빨갛게 띄운다).
    pub alert: bool,
    pub entries: Vec<ChangeEntry>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HistoryPage {
    /// 최근 것부터.
    pub batches: Vec<BatchView>,
    /// 필터에 걸린 묶음·칸 수.
    pub total_batches: u32,
    pub total_entries: u32,
    /// 기록에 나오는 사용자(사용자 필터용).
    pub users: Vec<String>,
}

/// 이만큼 넘는 묶음은 삭제가 없어도 빨갛게 띄운다.
pub const ALERT_CELLS: u32 = 10;

fn bounding(entries: &[&ChangeEntry]) -> CellRange {
    let (mut r0, mut c0, mut r1, mut c1) = (u32::MAX, u32::MAX, 0, 0);
    for e in entries {
        r0 = r0.min(e.row);
        c0 = c0.min(e.col);
        r1 = r1.max(e.row);
        c1 = c1.max(e.col);
    }
    CellRange::new(r0, c0, r1 - r0 + 1, c1 - c0 + 1)
}

/// 기록(오래된 것부터)을 묶음으로 나눈다. 같은 묶음의 칸은 이어서 기록된다.
fn group(log: &[ChangeEntry]) -> Vec<&[ChangeEntry]> {
    let mut out = Vec::new();
    let mut start = 0;
    for i in 1..=log.len() {
        if i == log.len()
            || log[i].batch != log[start].batch
            || log[i].user != log[start].user
            || log[i].sheet != log[start].sheet
        {
            if start < i {
                out.push(&log[start..i]);
            }
            start = i;
        }
    }
    out
}

fn view(all: &[ChangeEntry], hit: Vec<ChangeEntry>) -> BatchView {
    let refs: Vec<&ChangeEntry> = all.iter().collect();
    let deletes = all.iter().filter(|e| e.kind == ChangeKind::Delete).count();
    let count = all.len() as u32;
    let first = &all[0];
    BatchView {
        batch: first.batch.clone(),
        at_ms: all.last().map_or(first.at_ms, |e| e.at_ms),
        user: first.user.clone(),
        origin: first.origin,
        sheet: first.sheet,
        range: bounding(&refs),
        count,
        alert: (deletes > 0 && count >= 2) || count >= ALERT_CELLS || first.origin == BatchOrigin::Restore,
        entries: hit,
    }
}

/// 최근 것부터 필터에 걸린 묶음을 `offset`부터 `limit`개.
pub fn query(log: &[ChangeEntry], filter: &HistoryFilter, offset: usize, limit: usize) -> HistoryPage {
    let mut users: Vec<String> = Vec::new();
    for e in log {
        if !users.contains(&e.user) {
            users.push(e.user.clone());
        }
    }
    let mut page = HistoryPage { users, ..Default::default() };
    for batch in group(log).into_iter().rev() {
        let hit: Vec<ChangeEntry> = batch.iter().filter(|e| filter.matches(e)).cloned().collect();
        if hit.is_empty() {
            continue;
        }
        page.total_batches += 1;
        page.total_entries += hit.len() as u32;
        let n = page.total_batches as usize;
        if n > offset && n <= offset + limit {
            page.batches.push(view(batch, hit));
        }
    }
    page
}

/// 한 묶음의 모든 칸.
pub fn batch_entries<'a>(log: &'a [ChangeEntry], batch: &str) -> Vec<&'a ChangeEntry> {
    log.iter().filter(|e| e.batch == batch).collect()
}

/// 셀 타임라인: 그 시트 그 칸의 기록(오래된 것부터).
pub fn timeline(log: &[ChangeEntry], sheet: u32, at: CellRef) -> Vec<ChangeEntry> {
    log.iter().filter(|e| e.sheet == sheet && e.at() == at).cloned().collect()
}

/// 되돌리기 계획: 바꿀 값과, 그 뒤 다시 바뀌어 지금 값이 기록과 다른 칸.
#[derive(Debug, Clone, Default, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RevertPlan {
    pub changes: Vec<(CellRef, CellValue)>,
    /// 기록 뒤에 다시 바뀐 칸(되돌리면 그 변경도 덮어쓴다).
    pub stale: Vec<CellRef>,
}

/// 기록들을 되돌릴 값. 같은 칸이 여러 번이면 가장 앞 기록의 '이전 값'으로, 지금 값이 가장 뒤 기록의 '새 값'과 다르면 `stale`.
pub fn revert_plan(model: &SheetModel, entries: &[&ChangeEntry]) -> RevertPlan {
    let mut first: HashMap<CellRef, &ChangeEntry> = HashMap::new();
    let mut last: HashMap<CellRef, &ChangeEntry> = HashMap::new();
    let mut order: Vec<CellRef> = Vec::new();
    for e in entries {
        let at = e.at();
        if !first.contains_key(&at) {
            first.insert(at, e);
            order.push(at);
        }
        if last.get(&at).is_none_or(|l| l.seq < e.seq) {
            last.insert(at, e);
        }
        if first.get(&at).is_some_and(|f| f.seq > e.seq) {
            first.insert(at, e);
        }
    }
    let mut plan = RevertPlan::default();
    for at in order {
        let now = model.cell(at).map(|c| c.v.clone()).unwrap_or_default();
        if now != last[&at].new {
            plan.stale.push(at);
        }
        let back = first[&at].old.clone();
        if now != back {
            plan.changes.push((at, back));
        }
    }
    plan
}

/// 스냅샷 값과 지금 시트가 다른 칸(수식 칸 제외, 행 우선).
pub fn diff(values: &[CellValue], model: &SheetModel) -> Vec<CellRef> {
    let cols = model.cols.max(1);
    model
        .cells
        .iter()
        .zip(values)
        .enumerate()
        .filter(|(_, (cell, v))| cell.f.is_none() && cell.v != **v)
        .map(|(i, _)| CellRef::new(i as u32 / cols, i as u32 % cols))
        .collect()
}

/// `seq` 뒤에 바꾼 사람(명단 순서 없이 처음 나온 순서).
pub fn users_after(log: &[ChangeEntry], seq: u64) -> Vec<String> {
    let mut seen = HashSet::new();
    log.iter().filter(|e| e.seq > seq && seen.insert(e.user.clone())).map(|e| e.user.clone()).collect()
}

fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_string() }
}

/// 유닉스 시각(ms) → `YYYY-MM-DD HH:MM:SS`(현지 시각, `offset_minutes` = UTC와의 차이).
pub fn local_time(at_ms: i64, offset_minutes: i32) -> String {
    let secs = at_ms.div_euclid(1000) + i64::from(offset_minutes) * 60;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Howard Hinnant의 civil_from_days
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}", rem / 3600, rem % 3600 / 60, rem % 60)
}

fn kind_label(k: ChangeKind) -> &'static str {
    match k {
        ChangeKind::Input => "입력",
        ChangeKind::Edit => "수정",
        ChangeKind::Delete => "삭제",
        ChangeKind::Revert => "되돌림",
    }
}

/// 기록을 시트 주소로 보일 때 쓰는 시트 정보(세션 안 순번 순서).
#[derive(Debug, Clone, Default)]
pub struct SheetLabels {
    pub name: String,
    /// 불러온 범위의 시트 위치.
    pub origin: CellRef,
    /// 열 이름(머리글 기준).
    pub labels: Vec<String>,
}

impl SheetLabels {
    pub fn of(model: &SheetModel) -> Self {
        let table = crate::detect::TableGuess { range: model.bounds(), header_rows: model.header_rows };
        Self { name: model.sheet.clone(), origin: model.origin, labels: crate::detect::column_labels(model, &table) }
    }

    /// 기록의 시트 주소(`F7`).
    pub fn address(&self, e: &ChangeEntry) -> String {
        CellRef::new(e.row + self.origin.row, e.col + self.origin.col).to_string()
    }

    pub fn label(&self, col: u32) -> String {
        self.labels.get(col as usize).cloned().unwrap_or_default()
    }
}

/// 기록의 시트 정보. 시트가 하나뿐이면 기록의 시트 번호와 상관없이 그 시트다(한 시트만 넘겨 쓰는 경우).
pub fn sheet_of(sheets: &[SheetLabels], sheet: u32) -> Option<&SheetLabels> {
    if sheets.len() == 1 { sheets.first() } else { sheets.get(sheet as usize) }
}

/// CSV(Excel에서 열 수 있게 앞에 BOM). 셀 주소는 시트 기준이고, 시트가 둘 이상이면 '시트' 열을 더한다.
pub fn to_csv(entries: &[ChangeEntry], sheets: &[SheetLabels], offset_minutes: i32) -> String {
    let many = sheets.len() > 1;
    let mut out = String::from(if many {
        "\u{feff}시각,사용자,시트,셀,항목,종류,이전 값,새 값,묶음\r\n"
    } else {
        "\u{feff}시각,사용자,셀,항목,종류,이전 값,새 값,묶음\r\n"
    });
    let none = SheetLabels::default();
    for e in entries {
        let sheet = sheet_of(sheets, e.sheet).unwrap_or(&none);
        let mut row = vec![local_time(e.at_ms, offset_minutes), e.user.clone()];
        if many {
            row.push(sheet.name.clone());
        }
        row.extend([
            sheet.address(e),
            sheet.label(e.col),
            kind_label(e.kind).to_string(),
            plain(&e.old),
            plain(&e.new),
            e.batch.clone(),
        ]);
        out.push_str(&row.iter().map(|f| csv_field(f)).collect::<Vec<_>>().join(","));
        out.push_str("\r\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::tests::budget;

    fn n(x: f64) -> CellValue {
        CellValue::Number(x)
    }

    fn entry(
        seq: u64,
        batch: &str,
        user: &str,
        at: &str,
        old: CellValue,
        new: CellValue,
        origin: BatchOrigin,
    ) -> ChangeEntry {
        let c = CellRef::parse(at).unwrap();
        ChangeEntry {
            seq,
            at_ms: seq as i64 * 1000,
            user: user.into(),
            batch: batch.into(),
            origin,
            kind: ChangeKind::of(origin, &old, &new),
            sheet: 0,
            row: c.row,
            col: c.col,
            old,
            new,
        }
    }

    fn log() -> Vec<ChangeEntry> {
        use BatchOrigin::*;
        vec![
            entry(1, "a", "최도윤", "F7", CellValue::Empty, n(4800.0), Edit),
            entry(2, "b", "최도윤", "H7", CellValue::Empty, CellValue::Text("감사 출장 6회".into()), Edit),
            entry(3, "c", "이서연", "F4", n(5000.0), n(7200.0), Edit),
            entry(4, "d", "최도윤", "F7", n(4800.0), CellValue::Empty, Clear),
            entry(5, "d", "최도윤", "H7", CellValue::Text("감사 출장 6회".into()), CellValue::Empty, Clear),
        ]
    }

    #[test]
    fn kinds() {
        use BatchOrigin::*;
        assert_eq!(ChangeKind::of(Edit, &CellValue::Empty, &n(1.0)), ChangeKind::Input);
        assert_eq!(ChangeKind::of(Edit, &n(1.0), &n(2.0)), ChangeKind::Edit);
        assert_eq!(ChangeKind::of(Clear, &n(1.0), &CellValue::Empty), ChangeKind::Delete);
        assert_eq!(ChangeKind::of(Undo, &n(1.0), &CellValue::Empty), ChangeKind::Revert);
        assert_eq!(ChangeKind::of(Restore, &CellValue::Empty, &n(1.0)), ChangeKind::Revert);
    }

    #[test]
    fn query_groups_newest_first_and_flags_range_deletes() {
        let page = query(&log(), &HistoryFilter::default(), 0, 10);
        assert_eq!((page.total_batches, page.total_entries), (4, 5));
        let first = &page.batches[0];
        assert_eq!((first.batch.as_str(), first.count, first.alert), ("d", 2, true));
        assert_eq!(first.range.to_string(), "F7:H7");
        assert_eq!(first.origin, BatchOrigin::Clear);
        assert!(!page.batches[1].alert);
        assert_eq!(page.users, ["최도윤", "이서연"]);
        // 쪽 나누기
        let p2 = query(&log(), &HistoryFilter::default(), 2, 10);
        assert_eq!(p2.batches.iter().map(|b| b.batch.as_str()).collect::<Vec<_>>(), ["b", "a"]);
        assert_eq!(p2.total_batches, 4);
    }

    #[test]
    fn filters() {
        let l = log();
        let by = |f: HistoryFilter| query(&l, &f, 0, 10).batches.iter().map(|b| b.batch.clone()).collect::<Vec<_>>();
        assert_eq!(by(HistoryFilter { user: Some("이서연".into()), ..Default::default() }), ["c"]);
        assert_eq!(by(HistoryFilter { kinds: vec![ChangeKind::Delete], ..Default::default() }), ["d"]);
        assert_eq!(by(HistoryFilter { text: Some("감사".into()), ..Default::default() }), ["d", "b"]);
        assert_eq!(by(HistoryFilter { text: Some("4800".into()), ..Default::default() }), ["d", "a"]);
        assert_eq!(by(HistoryFilter { since_ms: Some(3000), ..Default::default() }), ["d", "c"]);
        let range = CellRange::parse("H1:H100").unwrap();
        let page = query(&l, &HistoryFilter { range: Some(range), ..Default::default() }, 0, 10);
        assert_eq!(page.batches[0].entries.len(), 1, "묶음 안에서도 걸린 칸만");
        assert_eq!(page.batches[0].count, 2);
    }

    #[test]
    fn revert_batch_and_stale_detection() {
        let l = log();
        let mut m = budget();
        let f7 = CellRef::parse("F7").unwrap();
        let h7 = CellRef::parse("H7").unwrap();
        m.cell_mut(f7).unwrap().v = CellValue::Empty;
        m.cell_mut(h7).unwrap().v = CellValue::Empty;
        let plan = revert_plan(&m, &batch_entries(&l, "d"));
        assert_eq!(plan.changes, [(f7, n(4800.0)), (h7, CellValue::Text("감사 출장 6회".into()))]);
        assert!(plan.stale.is_empty());
        // 그 뒤 F7을 누가 다시 채웠다 → stale
        m.cell_mut(f7).unwrap().v = n(1.0);
        let plan = revert_plan(&m, &batch_entries(&l, "d"));
        assert_eq!(plan.stale, [f7]);
        // 같은 칸 여러 기록: 가장 앞 기록의 이전 값으로
        let all: Vec<&ChangeEntry> = l.iter().filter(|e| e.at() == f7).collect();
        m.cell_mut(f7).unwrap().v = CellValue::Empty;
        assert!(revert_plan(&m, &all).changes.is_empty(), "지금 값이 이미 처음 값(빈 칸)");
        assert_eq!(timeline(&l, 0, f7).len(), 2);
        assert!(timeline(&l, 1, f7).is_empty(), "다른 시트의 같은 주소는 따로");
    }

    #[test]
    fn sheets_split_batches_and_filter() {
        let mut l = log();
        // 같은 묶음 ID라도 시트가 다르면 다른 묶음(호스트가 시트마다 따로 쓴 복원 등).
        l[4].sheet = 1;
        let page = query(&l, &HistoryFilter::default(), 0, 10);
        assert_eq!(page.total_batches, 5);
        assert_eq!((page.batches[0].sheet, page.batches[0].count), (1, 1));
        let only = query(&l, &HistoryFilter { sheet: Some(1), ..Default::default() }, 0, 10);
        assert_eq!((only.total_batches, only.total_entries), (1, 1));
    }

    #[test]
    fn snapshot_diff_and_users_after() {
        let m = budget();
        let mut values: Vec<CellValue> = m.cells.iter().map(|c| c.v.clone()).collect();
        assert!(diff(&values, &m).is_empty());
        values[(3 * m.cols + 5) as usize] = n(1.0); // F4
        values[(2 * m.cols + 6) as usize] = n(9.0); // G3는 수식 → 무시
        assert_eq!(diff(&values, &m), [CellRef::parse("F4").unwrap()]);
        assert_eq!(users_after(&log(), 2), ["이서연", "최도윤"]);
        assert!(users_after(&log(), 5).is_empty());
    }

    #[test]
    fn csv_export() {
        let one = SheetLabels { name: "예산".into(), origin: CellRef::new(0, 0), labels: vec![String::new(); 9] };
        let csv = to_csv(&log()[1..2], std::slice::from_ref(&one), 540);
        let lines: Vec<&str> = csv.lines().collect();
        assert!(lines[0].starts_with("\u{feff}시각,사용자,셀"));
        assert_eq!(lines[1], "1970-01-01 09:00:02,최도윤,H7,,입력,,감사 출장 6회,b");
        // 시트가 둘 이상이면 '시트' 열, 주소는 그 시트의 위치 기준.
        let two = SheetLabels { name: "인력".into(), origin: CellRef::new(2, 1), labels: vec!["".into(); 9] };
        let mut e = log()[1].clone();
        e.sheet = 1;
        let csv = to_csv(&[e], &[one, two], 540);
        let lines: Vec<&str> = csv.lines().collect();
        assert!(lines[0].starts_with("\u{feff}시각,사용자,시트,셀"));
        assert_eq!(lines[1], "1970-01-01 09:00:02,최도윤,인력,I9,,입력,,감사 출장 6회,b");
        assert_eq!(csv_field("a,\"b\""), "\"a,\"\"b\"\"\"");
        assert_eq!(local_time(1_791_000_000_000, 540), "2026-10-03 13:00:00");
    }
}
