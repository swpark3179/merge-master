//! 세션 액터: 시트·설정·권한 엔진·참여자·편집 상태를 tokio 작업 **하나**가 소유한다.
//!
//! - 시트를 확정할 때 [`SessionHandle::spawn`]으로 만들고 앱이 끝날 때까지 산다. 서비스 시작·중지는
//!   이 세션에 HTTP 서버를 붙였다 뗄 뿐이다.
//! - WebSocket 연결과 호스트 명령은 [`Command`]를 보내기만 하므로 모든 변경이 한 줄로 처리되고 락 경합이 없다.
//! - 호스트는 [`SessionHandle::with`]로 세션 안에서 시트·설정을 읽거나 고친다(설정의 주인은 세션 하나).
//! - 호스트 화면(1c 접속자 표)에는 [`HostSnapshot`]을, 1i 소통 패널에는 [`CommsSnapshot`]을 `watch` 채널로,
//!   1d 모니터링 그리드에는 셀 변경([`HostEvent`])을 `broadcast` 채널로 내보낸다.
//! - 셀 변경 기록과 스냅샷은 저장소([`Store`], SQLite)가 있으면 파일에도 남긴다.
//! - 접속: 호스트의 IP 명단([`IpNames`])에 있는 IP는 그 이름으로 바로 들어오고, 명단 밖 IP는 이름을 입력한 뒤 호스트가
//!   승인해야 들어온다. 승인받은 브라우저는 재접속 토큰으로 다시 묻지 않는다(토큰은 기록 파일에도 남아 앱을 다시 켜도 유효).

mod comms;
mod edit;
pub mod files;
mod persist;
mod snapshots;
mod structure;

pub use comms::CommsData;
pub use comms::{CommsSnapshot, HighlightAudience, HighlightSummary, NoticeSummary, NoticeTarget};
pub use persist::{
    BLOB_BASELINE, BLOB_COMMS, BLOB_CONFIG, BLOB_PEOPLE, BLOB_SHEET, META_CLOSED, META_DEADLINE_FIRED, META_DISCARDED,
    META_ENDED, META_JOB, META_SERVER, META_SHEETS, Recovered, Restore, ServerMeta, meta_sheets, recover,
};
pub use snapshots::AUTO_SNAPSHOT;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::PathBuf;
use std::time::Duration;

use collect_core::a1::sheet_prefix;
use collect_core::access::{Access, ProgressStats, Viewer, union_roster};
use collect_core::config::{CollectConfig, DeadlineAction, OthersRows, SessionCommon};
use collect_core::detect::{TableGuess, column_labels};
use collect_core::edit::{ConfigEdit, EditReach};
use collect_core::export::{ExportOptions, ExportPlan, SheetPlanInput, plan_workbook};
use collect_core::format::Axis;
use collect_core::history::{BatchOrigin, SheetLabels};
use collect_core::project::project_with;
use collect_core::structure::{Actor, StructureEdit, check_add, check_delete_cols, check_delete_rows};
use collect_core::view::{ConfigView, config_view_with, encode_flags};
use collect_core::{CellRange, CellRef, CellStyle, CellValue, RowCut, SheetCell, SheetModel};
use collect_store::{SnapshotKind, SnapshotMeta, Store};
use serde::Serialize;
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio::time::Instant;
use ts_rs::TS;

use crate::ipnames::IpNames;
use crate::protocol::{
    CellUpdate, ChangeEntry, ChatReaction, ChatTo, ClientMsg, KickReason, LockEntry, PROTOCOL_VERSION, PersonProgress,
    PresenceEntry, PresenceStatus, ServerMsg, SessionInfo, SheetView, StyleChange, Welcome,
};

/// 이만큼 입력·이동이 없으면 '자리 비움'.
pub const AWAY_AFTER: Duration = Duration::from_secs(180);
/// 연결이 끊긴 사람의 편집 잠금을 남겨 두는 시간(다시 붙으면 이어서 편집).
pub const LOCK_GRACE: Duration = Duration::from_secs(30);
/// 편집 잠금을 이보다 오래 잡고 있으면 풀어 준다(편집기를 열어 둔 채 자리를 비운 경우).
pub const LOCK_MAX: Duration = Duration::from_secs(600);
const TICK: Duration = Duration::from_secs(5);
/// 마감 시각 점검 주기(호스트 PC 시계로 마감 시각이 지나는 순간을 1초 안에 잡는다).
const DEADLINE_TICK: Duration = Duration::from_secs(1);
const MAX_NAME_CHARS: usize = 20;
/// 재접속 토큰·묶음 ID의 최대 길이(이보다 길면 버린다).
const MAX_TOKEN_CHARS: usize = 64;
pub(crate) const MAX_BATCH_ID_CHARS: usize = 80;

pub type ConnId = u64;

pub(crate) enum Outgoing {
    Msg(ServerMsg),
    Close,
}

/// 세션의 시트 하나: 지금 시트·그 시트 설정·기준 시트(엑셀 저장은 이것과 비교해 바뀐 칸만 쓴다).
///
/// 여러 시트를 함께 취합할 때 공통 설정([`SessionCommon`]: 제목·안내·요청자·마감·'마감 후 읽기 전용'·직접 넣은 명단)은
/// **첫 시트 설정이 주인**이고, 세션이 다른 시트 설정에 옮겨 적는다(다른 시트에서 직접 고친 공통 설정은 덮어쓴다).
#[derive(Debug, Clone)]
pub struct SheetSlot {
    pub model: SheetModel,
    pub config: CollectConfig,
    /// 불러온 시점(또는 마지막으로 원본에 저장한 시점)의 시트.
    pub baseline: SheetModel,
}

impl SheetSlot {
    pub fn new(model: SheetModel, config: CollectConfig) -> Self {
        Self { baseline: model.clone(), model, config }
    }
}

/// 첫 시트의 공통 설정을 다른 시트에 옮겨 적는다.
fn sync_common(sheets: &mut [SheetSlot]) {
    let Some((first, rest)) = sheets.split_first_mut() else { return };
    let common = SessionCommon::of(&first.config);
    for s in rest {
        common.write_to(&mut s.config);
    }
}

pub struct SessionInit {
    /// 불러온 순서(= 통합문서 시트 순서). 하나 이상.
    pub sheets: Vec<(SheetModel, CollectConfig)>,
    /// 셀 변경 기록·스냅샷을 남길 파일(없으면 메모리에만).
    pub store: Option<Store>,
    /// 기록 파일에서 되살릴 상태([`recover`]가 채운다).
    pub restore: Option<Restore>,
}

impl SessionInit {
    pub fn new(model: SheetModel, config: CollectConfig) -> Self {
        Self::multi(vec![(model, config)])
    }

    pub fn multi(sheets: Vec<(SheetModel, CollectConfig)>) -> Self {
        assert!(!sheets.is_empty(), "시트가 하나 이상 있어야 합니다");
        Self { sheets, store: None, restore: None }
    }
}

/// 1d 모니터링 그리드가 받는 셀 변경. `reset`이면 시트를 다시 읽는다(설정·서식이 바뀌었거나 다른 시트).
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(export)]
pub enum HostEvent {
    /// `by`가 빈 문자열이면 수식 재계산 결과.
    Cells {
        sheet: u32,
        by: String,
        cells: Vec<CellUpdate>,
    },
    Styles {
        sheet: u32,
        styles: Vec<CellStyle>,
        changes: Vec<StyleChange>,
    },
    /// 열 너비·행 높이가 바뀜(`from`부터 `count`개 줄이 `size` pt, 숨긴 줄은 그대로).
    Resized {
        sheet: u32,
        axis: Axis,
        from: u32,
        count: u32,
        size: f32,
    },
    Reset,
    /// 마감 시각이 되었고 마감 동작이 '공유 닫기'다(같은 마감 시각으로는 한 번만). 호스트 앱이 이 업무의 공유를 닫는다.
    DeadlineClose,
}

const HOST_FEED_CAPACITY: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ParticipantStatus {
    Online,
    Away,
    Offline,
}

/// 1c 접속자 표 한 줄.
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ParticipantRow {
    pub name: String,
    /// 명단 순서(색).
    pub color: i32,
    pub status: ParticipantStatus,
    /// 마지막 활동 뒤 지난 분(접속 중일 때).
    pub idle_mins: u32,
    /// 예: "행 3–7, 13–17 + I3:I1002". 시트가 둘 이상이면 시트마다 "예산신청서: 행 3–7 · 인력계획: F3:F9".
    pub assignment: String,
    #[ts(optional)]
    pub cursor: Option<SheetCell>,
    /// 지금 편집 중(잠근) 칸.
    #[ts(optional)]
    pub editing: Option<SheetCell>,
    /// 필수 칸(모든 시트 합).
    pub required: u32,
    pub filled: u32,
    #[ts(optional)]
    pub ip: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[ts(export)]
pub struct PendingRow {
    pub name: String,
    pub ip: String,
}

/// 시트 하나의 필수 진행률(1c·1d 시트 탭).
#[derive(Debug, Clone, Default, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SheetProgress {
    pub name: String,
    pub required: u32,
    pub filled: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HostSnapshot {
    pub participants: Vec<ParticipantRow>,
    pub pending: Vec<PendingRow>,
    pub online: u32,
    pub roster: u32,
    /// 필수 칸(모든 시트 합).
    pub required: u32,
    pub filled: u32,
    pub read_only: bool,
    /// 호스트가 취합을 마감했다.
    pub closed: bool,
    /// 참여자에게 보이는 호스트 이름(호스트 PC의 IP가 IP 명단에 있으면 그 이름).
    pub host_name: String,
    /// 시트마다(불러온 순서).
    pub sheets: Vec<SheetProgress>,
}

/// 수식 재계산 요청: 바뀐 입력 칸(시트 순번, 모델 좌표)과 값. 호스트(Excel)가 계산해 [`SessionHandle::formula_results`]로
/// 돌려준다.
#[derive(Debug, Clone)]
pub struct RecalcRequest {
    /// 먼저 계산 사본에서 지울 행(시트 순번, 그 순간의 행 좌표 — 엑셀에서 불러온 행을 지웠다).
    pub cuts: Vec<(u32, RowCut)>,
    pub writes: Vec<(u32, CellRef, CellValue)>,
    /// 이 요청을 보낼 때의 계산 사본 판([`SessionHandle::formula_results_at`]에 돌려준다 — 행을 지우기 전에 계산한
    /// 결과는 버린다).
    pub epoch: u64,
}

/// 계산 사본을 열 때 맞출 것([`SessionHandle::calc_layout`]).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CalcLayout {
    /// 계산 사본 판(행을 지울 때마다 는다). 사본을 연 뒤 재계산을 붙였을 때 판이 달라졌으면 다시 연다.
    pub epoch: u64,
    /// 세션 안 시트 순번 순서.
    pub sheets: Vec<CalcSheetLayout>,
    /// 기록에서 칸마다 마지막 값(지운 기록·계산 사본 밖 칸은 뺀다). 사본을 연 뒤 한 번 쓴다.
    pub replay: Vec<(u32, CellRef, CellValue)>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct CalcSheetLayout {
    /// 결과를 읽을 수식 칸(모델 좌표, 엑셀에서 불러온 범위 안).
    pub formulas: Vec<CellRef>,
    /// 원본에서 이 순서로 지우면 지금 기준 시트와 같아지는 행.
    pub cuts: Vec<RowCut>,
    /// 원본에서 숨겨져 있던 행·열(펼친 줄).
    pub unhide_rows: Vec<u32>,
    pub unhide_cols: Vec<u32>,
}

/// 수식 재계산 상태(Excel 계산 사본). 호스트 앱 1c와 에이전트 상태에 보인다.
#[derive(Debug, Clone, Default, PartialEq, Serialize, serde::Deserialize, TS)]
#[serde(tag = "state", rename_all = "camelCase")]
#[ts(export)]
pub enum RecalcStatus {
    /// 수식이 없는 시트(모든 시트).
    #[default]
    None,
    Opening,
    Ready {
        formulas: u32,
    },
    /// 수식은 저장할 때 계산한다.
    Unavailable {
        message: String,
    },
}

/// 엑셀 저장 재료: 시트마다 기준 시트·지금 시트·설정·열 이름, 그리고 모든 변경 기록([`SessionHandle::save_parts`]).
#[derive(Debug, Clone)]
pub struct SaveParts {
    pub sheets: Vec<SaveSheet>,
    pub history: Vec<ChangeEntry>,
}

#[derive(Debug, Clone)]
pub struct SaveSheet {
    pub baseline: SheetModel,
    pub current: SheetModel,
    pub config: CollectConfig,
    pub labels: Vec<String>,
}

impl SaveParts {
    /// 통합문서 저장 계획(`names`는 Excel의 시트 이름, 세션 안 순번 순서). `keep`은 시트마다 쓰지 않을 칸.
    pub fn plan(
        &self,
        names: &[String],
        opts: ExportOptions,
        offset_minutes: i32,
        keep: &[HashSet<CellRef>],
    ) -> Vec<(String, ExportPlan)> {
        let inputs: Vec<SheetPlanInput<'_>> = self
            .sheets
            .iter()
            .zip(names)
            .map(|(x, name)| SheetPlanInput { name, baseline: &x.baseline, current: &x.current, labels: &x.labels })
            .collect();
        plan_workbook(&inputs, &self.history, opts, offset_minutes, keep)
    }
}

/// 호스트가 `with` 안에서 요청한 일(작업이 끝난 뒤 순서대로 한다). `Resized`는 호스트가 바꾼 열 너비·행 높이(시트에는
/// 이미 반영): 참여자·호스트 화면에 알리고 시트를 남긴다.
enum Action {
    Set {
        user: String,
        origin: BatchOrigin,
        sheet: usize,
        cells: Vec<(CellRef, CellValue)>,
    },
    Snapshot {
        kind: SnapshotKind,
        label: String,
    },
    Close(bool),
    Notice {
        text: String,
        important: bool,
        target: NoticeTarget,
    },
    Highlight {
        sheet: usize,
        range: CellRange,
        note: String,
        audience: HighlightAudience,
    },
    ClearHighlight(u32),
    Chat {
        to: ChatTo,
        text: String,
    },
    ChatReact {
        id: u32,
        kind: ChatReaction,
    },
    Store(Store),
    Meta(String, String),
    Resized {
        sheet: usize,
        axis: Axis,
        from: u32,
        count: u32,
        size: f32,
    },
    /// 검사를 마친 표 구조 바꾸기([`Shared::structure`]).
    Structure {
        sheet: usize,
        edit: StructureEdit,
    },
}

const NO_SHEET: &str = "없는 시트입니다";

/// [`SessionHandle::with`] 안에서 호스트가 보는 세션 상태.
pub struct Shared<'a> {
    /// 시트마다(불러온 순서). 시트를 더하거나 빼려면 [`Shared::replace`].
    pub sheets: &'a mut [SheetSlot],
    rt: &'a [SheetRt],
    history: &'a [ChangeEntry],
    snapshots: &'a [(SnapshotMeta, Vec<u8>)],
    closed: bool,
    /// 호스트가 Excel로 저장하는 중([`SessionHandle::set_saving`] — 행·열을 지우지 않는다).
    saving: bool,
    host_name: String,
    changed: bool,
    replacement: Option<Vec<(SheetModel, CollectConfig)>>,
    actions: Vec<Action>,
}

impl Shared<'_> {
    /// 호스트 이름(변경 기록·공지·채팅에 남는다): 호스트 PC의 IP가 IP 명단에 있으면 그 이름, 없으면 요청자, 그것도
    /// 없으면 '호스트'.
    pub fn host_name(&self) -> &str {
        &self.host_name
    }

    /// 설정·서식을 바꿨다: 권한을 다시 계산하고 접속 중인 참여자에게 새 시트를 보낸다.
    pub fn changed(&mut self) {
        self.changed = true;
    }

    /// 호스트가 열 너비·행 높이를 바꾼다(1b·1d 머리글 경계 끌기). 권한은 그대로라 참여자에게 새 시트를 다시 보내지 않고
    /// 바뀐 크기만 알린다(`resized`). 실제로 쓴 크기(범위 안으로·0.25pt 단위)를 돌려준다.
    pub fn resize(&mut self, sheet: usize, axis: Axis, from: u32, count: u32, size: f32) -> Result<f32, String> {
        let slot = self.sheets.get_mut(sheet).ok_or_else(|| NO_SHEET.to_string())?;
        let size = collect_core::format::resize(&mut slot.model, axis, from, count, size).map_err(|e| e.to_string())?;
        self.actions.push(Action::Resized { sheet, axis, from, count, size });
        Ok(size)
    }

    /// 호스트가 표 구조를 바꾼다(1b·1d 오른쪽 클릭 메뉴): 행·열 붙이기, 행 지우기(엑셀에서 불러온 행도), 앱에서 붙인 열
    /// 지우기, 행 취소 표시.
    /// 여기서 검사하고(지울 수 없는 행·열이면 이유), 작업이 끝난 뒤 시트·설정·편집 상태·기록을 함께 바꾸고 모두에게
    /// 새 시트를 보낸다. 같은 작업 안에서 여러 번 부르면 앞의 것이 반영되기 전 시트로 검사하므로 한 번씩 부른다.
    pub fn structure(&mut self, sheet: usize, edit: StructureEdit) -> Result<(), String> {
        let slot = self.slot(sheet)?;
        let rt = &self.rt[sheet];
        match edit {
            StructureEdit::AddRows { .. } | StructureEdit::AddCols { .. } => {
                check_add(&slot.model, &slot.baseline, edit)?;
            }
            StructureEdit::DeleteRows { .. } | StructureEdit::DeleteCols { .. } if self.saving => {
                return Err(structure::SAVING.into());
            }
            StructureEdit::DeleteRows { from, count } => {
                check_delete_rows(&slot.model, &slot.config, &rt.access, from, count, Actor::Host)?;
            }
            StructureEdit::DeleteCols { from, count } => {
                check_delete_cols(&slot.model, slot.baseline.cols, &slot.config, from, count)?;
            }
            StructureEdit::CancelRows { from, count, .. } => {
                if count == 0 || from.saturating_add(count) > slot.model.rows || from < slot.model.header_rows {
                    return Err("데이터 행만 취소 표시할 수 있습니다".into());
                }
            }
        }
        self.actions.push(Action::Structure { sheet, edit });
        Ok(())
    }

    /// 다른 시트(들)로 바꿨다(다시 불러와 확정): 작업이 끝나면 셀 버전·잠금·이력을 새로 시작한다.
    pub fn replace(&mut self, sheets: Vec<(SheetModel, CollectConfig)>) {
        assert!(!sheets.is_empty(), "시트가 하나 이상 있어야 합니다");
        self.replacement = Some(sheets);
        self.changed = true;
    }

    fn slot(&self, sheet: usize) -> Result<&SheetSlot, String> {
        self.sheets.get(sheet).ok_or_else(|| NO_SHEET.to_string())
    }

    /// 지금 명단(모든 시트 명단의 합집합, 사람 인덱스 = 색).
    pub fn roster(&self) -> Vec<String> {
        union_roster(self.sheets.iter().map(|s| (&s.model, &s.config)))
    }

    /// 1b·1d 화면이 그릴 설정 보기(합집합 명단 기준). `viewer`를 주면 그 참여자 시점.
    pub fn view(&self, sheet: usize, viewer: Option<&str>) -> Result<ConfigView, String> {
        let s = self.slot(sheet)?;
        let mut view = config_view_with(&s.model, &s.config, viewer, &self.roster()).map_err(|e| e.to_string())?;
        view.baseline_rows = s.baseline.rows;
        view.baseline_cols = s.baseline.cols;
        Ok(view)
    }

    /// 1b 설정 편집. 공통 설정(제목·안내·마감, 직접 넣은 명단)은 첫 시트에 하고 다른 시트에 옮겨 적는다
    /// ([`ConfigEdit::reach`]).
    pub fn apply(&mut self, sheet: usize, edit: ConfigEdit) -> Result<(), String> {
        self.slot(sheet)?;
        match edit.reach() {
            EditReach::Sheet => {
                let s = &mut self.sheets[sheet];
                s.config.apply(&s.model, edit).map_err(|e| e.to_string())?;
            }
            EditReach::Common => {
                let s = &mut self.sheets[0];
                s.config.apply(&s.model, edit).map_err(|e| e.to_string())?;
            }
            EditReach::SheetAndCommon => {
                let s = &mut self.sheets[sheet];
                s.config.apply(&s.model, edit).map_err(|e| e.to_string())?;
                let action = s.config.permissions.deadline_action;
                self.sheets[0].config.permissions.deadline_action = action;
            }
        }
        sync_common(self.sheets);
        self.changed = true;
        Ok(())
    }

    /// 세션이 센 필수 진행률(합집합 명단 기준). 이 작업 안에서 바꾼 설정은 작업이 끝난 뒤 반영된다.
    pub fn progress(&self, sheet: usize) -> Option<&ProgressStats> {
        self.rt.get(sheet).map(|r| &r.progress)
    }

    /// 1d '담당자별': 그 시트에서 행 담당자가 없는 행(누구나 입력하는 행) 중 사람마다(합집합 명단 인덱스) 그 사람이
    /// 넣은 값이 지금 남아 있는 행(칸마다 마지막으로 바꾼 사람 기준, 오름차순). 명단 밖 이름(호스트·Excel 값 유지)은 뺀다.
    pub fn open_rows_by(&self, sheet: usize) -> Vec<Vec<u32>> {
        let (Some(slot), Some(rt)) = (self.sheets.get(sheet), self.rt.get(sheet)) else { return Vec::new() };
        let model = &slot.model;
        let mut out = vec![Vec::new(); rt.access.roster().len()];
        for (&i, name) in &rt.writer {
            let at = rt.cell_of(i, model.cols);
            if at.row < model.header_rows || rt.access.row_owner(at.row).is_some() {
                continue;
            }
            if model.cell(at).is_none_or(|c| c.v.is_empty()) {
                continue;
            }
            if let Some(rows) = rt.access.person(name).and_then(|p| out.get_mut(p)) {
                rows.push(at.row);
            }
        }
        for rows in &mut out {
            rows.sort_unstable();
            rows.dedup();
        }
        out
    }

    /// 셀 변경 기록(오래된 것부터).
    pub fn history(&self) -> &[ChangeEntry] {
        self.history
    }

    /// 엑셀 저장 재료(시트마다 기준·지금·설정·열 이름 + 모든 기록).
    pub fn save_parts(&self) -> SaveParts {
        SaveParts {
            sheets: self
                .sheets
                .iter()
                .map(|x| SaveSheet {
                    baseline: x.baseline.clone(),
                    current: x.model.clone(),
                    config: x.config.clone(),
                    labels: SheetLabels::of(&x.model).labels,
                })
                .collect(),
            history: self.history.to_vec(),
        }
    }

    /// 호스트가 칸 값을 바꾼다(되돌리기·이 값으로·'Excel 값 유지'). 참여자 입력처럼 버전·이력·진행률에 반영하고 알린다.
    /// 수식 칸이면 수식을 버리고 그 값으로 바꾼다(같은 값이어도).
    pub fn host_set(&mut self, user: &str, origin: BatchOrigin, sheet: usize, cells: Vec<(CellRef, CellValue)>) {
        if !cells.is_empty() && sheet < self.sheets.len() {
            self.actions.push(Action::Set { user: user.to_string(), origin, sheet, cells });
        }
    }

    /// 호스트가 취합을 마감했는가(마감 시각과 별개로 읽기 전용).
    pub fn closed(&self) -> bool {
        self.actions
            .iter()
            .rev()
            .find_map(|a| match a {
                Action::Close(c) => Some(*c),
                _ => None,
            })
            .unwrap_or(self.closed)
    }

    /// 취합 마감(참여자 입력을 막음) / 다시 열기.
    pub fn set_closed(&mut self, closed: bool) {
        self.actions.push(Action::Close(closed));
    }

    /// 스냅샷 목록(오래된 것부터).
    pub fn snapshots(&self) -> Vec<SnapshotMeta> {
        self.snapshots.iter().map(|(m, _)| m.clone()).collect()
    }

    /// 스냅샷의 셀 값(시트마다 행 우선). 스냅샷 뒤에 표 아래에 붙인 행은 빈 칸으로 채워 지금 시트 크기로 돌려준다.
    pub fn snapshot_values(&self, id: u32) -> Option<Vec<Vec<CellValue>>> {
        let mut sheets = snapshots::values_of(self.snapshots, id)?;
        for (values, slot) in sheets.iter_mut().zip(self.sheets.iter()) {
            let cols = slot.model.cols.max(1) as usize;
            if values.len() < slot.model.cells.len() && values.len() % cols == 0 {
                values.resize(slot.model.cells.len(), CellValue::Empty);
            }
        }
        Some(sheets)
    }

    /// 지금 시트로 스냅샷을 남긴다(작업이 끝난 뒤).
    pub fn take_snapshot(&mut self, label: &str) {
        self.actions.push(Action::Snapshot { kind: SnapshotKind::Manual, label: label.to_string() });
    }

    /// 스냅샷 시점으로 되돌린다: 먼저 '복원 직전' 스냅샷을 남기고, 시트마다 다른 칸을 스냅샷 값으로 바꾼다. 바꿀 칸 수를
    /// 돌려준다.
    pub fn restore(&mut self, id: u32, user: &str) -> Result<usize, String> {
        let sheets = self.snapshot_values(id).ok_or("스냅샷을 찾을 수 없습니다")?;
        if sheets.len() != self.sheets.len()
            || sheets.iter().zip(self.sheets.iter()).any(|(v, s)| v.len() != s.model.cells.len())
        {
            return Err("다른 시트의 스냅샷입니다".into());
        }
        let plans: Vec<Vec<(CellRef, CellValue)>> = sheets
            .iter()
            .zip(self.sheets.iter())
            .map(|(values, s)| {
                collect_core::history::diff(values, &s.model)
                    .into_iter()
                    .map(|at| (at, values[(at.row * s.model.cols + at.col) as usize].clone()))
                    .collect()
            })
            .collect();
        let n: usize = plans.iter().map(Vec::len).sum();
        if n > 0 {
            self.actions.push(Action::Snapshot { kind: SnapshotKind::BeforeRestore, label: String::new() });
            for (sheet, cells) in plans.into_iter().enumerate() {
                self.host_set(user, BatchOrigin::Restore, sheet, cells);
            }
        }
        Ok(n)
    }

    /// 공지를 보낸다.
    pub fn send_notice(&mut self, text: &str, important: bool, target: NoticeTarget) {
        self.actions.push(Action::Notice { text: text.to_string(), important, target });
    }

    /// 셀을 강조한다(노란 표시 + 말풍선). 범위는 그 시트의 모델 좌표.
    pub fn highlight(&mut self, sheet: usize, range: CellRange, note: &str, audience: HighlightAudience) {
        if sheet < self.sheets.len() {
            self.actions.push(Action::Highlight { sheet, range, note: note.to_string(), audience });
        }
    }

    pub fn clear_highlight(&mut self, id: u32) {
        self.actions.push(Action::ClearHighlight(id));
    }

    /// 호스트 채팅(전체 또는 한 사람).
    pub fn chat(&mut self, to: ChatTo, text: &str) {
        self.actions.push(Action::Chat { to, text: text.to_string() });
    }

    /// 참여자 채팅에 호스트 리액션을 달거나 뗀다(같은 종류를 다시 보내면 뗀다).
    pub fn chat_react(&mut self, id: u32, kind: ChatReaction) {
        self.actions.push(Action::ChatReact { id, kind });
    }

    /// 기록을 남길 저장소를 바꾼다(다시 불러와 확정할 때 새 파일).
    pub fn set_store(&mut self, store: Store) {
        self.actions.push(Action::Store(store));
    }

    /// 기록 파일의 메타 값(예: 서비스 포트, [`META_SERVER`]).
    pub fn set_meta(&mut self, key: &str, value: &str) {
        self.actions.push(Action::Meta(key.to_string(), value.to_string()));
    }
}

/// 호스트 작업이 끝난 뒤 부를 일(대개 결과를 돌려주기). 접속자 표·세션 정보·소통 상태를 내보낸 **뒤에** 부르므로,
/// 결과를 받은 쪽이 곧바로 [`SessionHandle::snapshot`]을 읽어도 바뀐 내용이 보인다.
type Completion = Box<dyn FnOnce() + Send>;
type HostJob = Box<dyn for<'a> FnOnce(&mut Shared<'a>) -> Completion + Send>;

pub(crate) enum Command {
    Connect {
        conn: ConnId,
        ip: IpAddr,
        outbox: mpsc::UnboundedSender<Outgoing>,
    },
    Client {
        conn: ConnId,
        msg: ClientMsg,
    },
    Disconnect {
        conn: ConnId,
    },
    Approve {
        name: String,
        range: Option<(u32, CellRange)>,
        note: String,
        reply: oneshot::Sender<Result<(), String>>,
    },
    Reject {
        name: String,
        reply: oneshot::Sender<Result<(), String>>,
    },
    Kick {
        name: String,
        reply: oneshot::Sender<Result<(), String>>,
    },
    Unlock {
        name: String,
        reply: oneshot::Sender<Result<(), String>>,
    },
    With(HostJob),
    SetIpNames(IpNames),
    SetRecalc(Option<mpsc::UnboundedSender<RecalcRequest>>),
    /// 다시 계산한 수식 칸 값. `epoch`가 있으면 그 판의 계산 사본에서 계산한 것(지금 판이 아니면 버린다).
    Formulas {
        values: Vec<(u32, CellRef, CellValue)>,
        epoch: Option<u64>,
    },
    CalcLayout(oneshot::Sender<CalcLayout>),
    /// 호스트가 Excel로 저장하는 중(그동안 행·열을 지우지 않는다 — 기준 시트가 저장하는 파일과 어긋나지 않게).
    SetSaving(bool),
    /// 참여자에게 보일 호스트 이름을 정해 둔다(`None`이면 이 PC의 IP·요청자로 정한다).
    SetHostName(Option<String>),
    /// 지금 상태를 모두 남긴 뒤 기록 파일의 사본을 만든다.
    Backup {
        to: PathBuf,
        reply: oneshot::Sender<Result<std::sync::mpsc::Receiver<collect_store::Result<()>>, String>>,
    },
    Tick,
    /// 마감 시각 점검(1초마다).
    DeadlineTick,
}

/// 세션에 명령을 보내는 손잡이(복제 가능). 마지막 손잡이가 사라지면 액터도 끝난다.
#[derive(Clone)]
pub struct SessionHandle {
    tx: mpsc::UnboundedSender<Command>,
    snapshot: watch::Receiver<HostSnapshot>,
    info: watch::Receiver<SessionInfo>,
    comms: watch::Receiver<CommsSnapshot>,
    host_feed: broadcast::Sender<HostEvent>,
}

const GONE: &str = "세션이 끝났습니다";

impl SessionHandle {
    pub(crate) fn send(&self, cmd: Command) {
        let _ = self.tx.send(cmd);
    }

    async fn ask<T>(&self, make: impl FnOnce(oneshot::Sender<T>) -> Command) -> Result<T, String> {
        let (tx, rx) = oneshot::channel();
        self.tx.send(make(tx)).map_err(|_| GONE.to_string())?;
        rx.await.map_err(|_| GONE.to_string())
    }

    /// 세션을 만든다(시트를 확정할 때). 손잡이가 모두 사라지면 끝난다.
    pub fn spawn(init: SessionInit) -> SessionHandle {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut state = State::new(init);
        let (snap_tx, snap_rx) = watch::channel(state.snapshot());
        let (info_tx, info_rx) = watch::channel(state.info());
        let (comms_tx, comms_rx) = watch::channel(state.comms_snapshot());
        let host_feed = state.host_feed.clone();

        // 주기 점검(자리 비움·잠금 만료·마감). 약한 참조라 손잡이가 모두 사라지면 함께 끝난다.
        let ticks: [(Duration, fn() -> Command); 2] =
            [(TICK, || Command::Tick), (DEADLINE_TICK, || Command::DeadlineTick)];
        for (period, cmd) in ticks {
            let weak = tx.downgrade();
            tokio::spawn(async move {
                let mut every = tokio::time::interval(period);
                every.tick().await;
                loop {
                    every.tick().await;
                    match weak.upgrade() {
                        Some(tx) if tx.send(cmd()).is_ok() => {}
                        _ => break,
                    }
                }
            });
        }

        tokio::spawn(async move {
            let mut comms_rev = state.comms.rev;
            while let Some(cmd) = rx.recv().await {
                let done = state.handle(cmd);
                let snap = state.snapshot();
                snap_tx.send_if_modified(|s| replace_if_changed(s, snap));
                let info = state.info();
                info_tx.send_if_modified(|s| replace_if_changed(s, info));
                if state.comms.rev != comms_rev {
                    comms_rev = state.comms.rev;
                    let _ = comms_tx.send(state.comms_snapshot());
                }
                if let Some(done) = done {
                    done();
                }
            }
        });

        SessionHandle { tx, snapshot: snap_rx, info: info_rx, comms: comms_rx, host_feed }
    }

    /// 세션 안에서 시트·설정을 읽거나 고친다. 고쳤으면 `Shared::changed`/`replace`를 부른다.
    pub async fn with<R, F>(&self, f: F) -> Result<R, String>
    where
        R: Send + 'static,
        F: for<'a> FnOnce(&mut Shared<'a>) -> R + Send + 'static,
    {
        self.ask(|reply| {
            Command::With(Box::new(move |s: &mut Shared<'_>| {
                let out = f(s);
                Box::new(move || {
                    let _ = reply.send(out);
                }) as Completion
            }))
        })
        .await
    }

    /// 승인을 기다리는 참여자를 들인다: 명단에 없는 이름이면 명단에 넣고(범위를 주면 그 시트에 범위 배정도) 들인다.
    /// 같은 이름이 다른 기기에서 접속 중이면 그 연결은 끊는다.
    pub async fn approve(&self, name: &str, range: Option<(u32, CellRange)>, note: &str) -> Result<(), String> {
        let (name, note) = (name.to_string(), note.to_string());
        self.ask(|reply| Command::Approve { name, range, note, reply }).await?
    }

    pub async fn reject(&self, name: &str) -> Result<(), String> {
        let name = name.to_string();
        self.ask(|reply| Command::Reject { name, reply }).await?
    }

    pub async fn kick(&self, name: &str) -> Result<(), String> {
        let name = name.to_string();
        self.ask(|reply| Command::Kick { name, reply }).await?
    }

    /// 그 사람이 잡고 있는 편집 잠금을 모두 푼다(1c '편집 잠금 해제').
    pub async fn unlock(&self, name: &str) -> Result<(), String> {
        let name = name.to_string();
        self.ask(|reply| Command::Unlock { name, reply }).await?
    }

    /// IP 명단을 바꾼다(호스트가 고치거나 JSON 파일을 불러옴). 명단에 새로 든 IP에서 기다리던 사람은 바로 들인다.
    pub fn set_ip_names(&self, names: IpNames) {
        self.send(Command::SetIpNames(names));
    }

    /// 수식 재계산을 맡을 곳(호스트의 Excel). `None`이면 끈다.
    pub fn set_recalc(&self, tx: Option<mpsc::UnboundedSender<RecalcRequest>>) {
        self.send(Command::SetRecalc(tx));
    }

    /// 재계산된 수식 칸 값(시트 순번, 모델 좌표). 바뀐 칸만 참여자에게 보낸다.
    pub fn formula_results(&self, values: Vec<(u32, CellRef, CellValue)>) {
        self.send(Command::Formulas { values, epoch: None });
    }

    /// 계산 사본 판 `epoch`에서 계산한 수식 칸 값. 그사이 행을 지워 판이 바뀌었으면 버린다.
    pub fn formula_results_at(&self, epoch: u64, values: Vec<(u32, CellRef, CellValue)>) {
        self.send(Command::Formulas { values, epoch: Some(epoch) });
    }

    /// 계산 사본을 열 때 맞출 것: 수식 칸, 원본에서 지울 행·펼칠 줄, 기록을 되짚을 값, 지금 판.
    pub async fn calc_layout(&self) -> Result<CalcLayout, String> {
        self.ask(Command::CalcLayout).await
    }

    /// 호스트가 Excel로 저장하는 중이다(`true`) / 끝났다. 그동안 행·열 지우기는 거절한다.
    pub fn set_saving(&self, on: bool) {
        self.send(Command::SetSaving(on));
    }

    /// 참여자에게 보일 호스트 이름을 정해 둔다. 에이전트 PC에서 서비스하면 그 PC의 IP로 IP 명단을 찾으면 안 되므로,
    /// 넘긴 호스트가 정한 이름을 쓴다. `None`이면 원래대로(이 PC의 IP가 IP 명단에 있으면 그 이름, 없으면 요청자).
    pub fn set_host_name(&self, name: Option<String>) {
        self.send(Command::SetHostName(name.filter(|n| !n.trim().is_empty())));
    }

    /// 엑셀 저장 재료를 한 번에 뜬다(시트마다 기준·지금·설정·열 이름 + 모든 기록).
    pub async fn save_parts(&self) -> Result<SaveParts, String> {
        self.with(|s| s.save_parts()).await
    }

    /// 지금 상태(시트·설정·기준·소통·토큰·기록)를 모두 남긴 기록 파일의 사본을 `to`에 만든다(다른 PC로 넘길 때).
    /// 대상 파일이 있으면 실패한다. 기록 파일이 없는 세션(메모리에만)이면 실패한다.
    pub async fn backup_store(&self, to: PathBuf) -> Result<(), String> {
        let rx = self.ask(|reply| Command::Backup { to, reply }).await??;
        tokio::task::spawn_blocking(move || rx.recv())
            .await
            .map_err(|e| e.to_string())?
            .map_err(|_| GONE.to_string())?
            .map_err(|e| e.to_string())
    }

    pub fn snapshot(&self) -> HostSnapshot {
        self.snapshot.borrow().clone()
    }

    /// 접속자 표가 바뀔 때마다 깨어나는 수신기.
    pub fn subscribe(&self) -> watch::Receiver<HostSnapshot> {
        self.snapshot.clone()
    }

    pub fn info(&self) -> SessionInfo {
        self.info.borrow().clone()
    }

    /// 공개 정보(업무명 등)가 바뀔 때마다 깨어나는 수신기.
    pub fn subscribe_info(&self) -> watch::Receiver<SessionInfo> {
        self.info.clone()
    }

    /// 같은 세션을 가리키는 손잡이인가.
    pub fn same(&self, other: &SessionHandle) -> bool {
        self.tx.same_channel(&other.tx)
    }

    /// 그 이름이 명단에 있는가(접속자 표로 본다 — 세션에 묻지 않으므로 막히지 않는다).
    pub fn has_person(&self, name: &str) -> bool {
        self.snapshot.borrow().participants.iter().any(|p| p.name == name)
    }

    /// 1i 소통 패널: 보낸 공지·강조, 모든 채팅.
    pub fn comms(&self) -> CommsSnapshot {
        self.comms.borrow().clone()
    }

    pub fn subscribe_comms(&self) -> watch::Receiver<CommsSnapshot> {
        self.comms.clone()
    }

    /// 1d 모니터링: 셀 변경 알림(받기 시작한 뒤의 것). 놓치면(`Lagged`) 시트를 다시 읽는다.
    pub fn host_events(&self) -> broadcast::Receiver<HostEvent> {
        self.host_feed.subscribe()
    }
}

fn replace_if_changed<T: PartialEq>(slot: &mut T, new: T) -> bool {
    if *slot == new {
        false
    } else {
        *slot = new;
        true
    }
}

struct Conn {
    outbox: mpsc::UnboundedSender<Outgoing>,
    ip: IpAddr,
    /// 입장한 이름. 입장 전이거나 다른 기기로 넘어가 끊기는 중이면 `None`.
    name: Option<String>,
}

struct Person {
    token: String,
    conn: Option<ConnId>,
    last_active: Instant,
    cursor: Option<SheetCell>,
    ip: Option<IpAddr>,
}

struct Pending {
    name: String,
    conn: ConnId,
}

struct Lock {
    owner: String,
    since: Instant,
    /// 잡은 사람의 연결이 끊긴 시각(유예 시간이 지나면 푼다).
    orphaned: Option<Instant>,
}

/// 시트 하나에서 세션이 계산하거나 쌓는 것(시트·설정은 [`SheetSlot`]).
struct SheetRt {
    access: Access,
    /// 필수 진행률. 시트·설정이 바뀔 때 다시 세고, 셀 수정은 증분으로 반영한다.
    progress: ProgressStats,
    labels: Vec<String>,
    /// '남의 행 숨김'일 때 사람마다 보이는 행(머리글 포함). 읽기 전용 정책이면 비어 있다.
    visible: HashMap<usize, Vec<bool>>,
    has_formulas: bool,
    // ---- 편집(edit.rs)
    versions: Vec<u32>,
    /// 칸(행 우선 인덱스) → 편집 잠금. 한 사람은 모든 시트를 통틀어 한 칸만 잠근다.
    locks: HashMap<usize, Lock>,
    /// 칸마다 마지막으로 바꾼 사람(충돌 판정: 내가 연달아 바꾼 것은 충돌이 아니다).
    writer: HashMap<usize, String>,
    /// 계산 사본에 있는 행·열 수(재계산을 켤 때의 기준 시트 — 사본은 원본을 복사한 것이다). 앱에서 붙인 행·열은 사본에
    /// 없어 저장할 때 계산한다.
    recalc_rows: u32,
    recalc_cols: u32,
    /// 사람마다 이 시트의 배정 요약(1c 접속자 표).
    assignment: Vec<String>,
}

impl SheetRt {
    fn new(slot: &SheetSlot) -> Self {
        Self {
            access: Access::compile(&slot.model, &slot.config),
            progress: ProgressStats::default(),
            labels: Vec::new(),
            visible: HashMap::new(),
            has_formulas: false,
            versions: vec![0; slot.model.cells.len()],
            locks: HashMap::new(),
            writer: HashMap::new(),
            recalc_rows: 0,
            recalc_cols: 0,
            assignment: Vec::new(),
        }
    }

    fn cell_of(&self, i: usize, cols: u32) -> CellRef {
        let cols = cols.max(1);
        CellRef::new(i as u32 / cols, i as u32 % cols)
    }
}

struct State {
    sheets: Vec<SheetSlot>,
    rt: Vec<SheetRt>,
    /// 모든 시트 명단의 합집합(사람 인덱스 = 색). 시트마다의 권한 엔진도 이 순서를 쓴다.
    roster: Vec<String>,
    /// 어느 시트에든 수식이 있다(수식은 다른 시트를 볼 수 있어, 어느 시트를 고쳐도 다시 계산한다).
    any_formulas: bool,
    deadline: Option<chrono::NaiveDateTime>,
    /// 마감 동작 '공유 닫기'를 한 마감 시각(같은 마감으로는 한 번만 닫는다 — 메타 [`META_DEADLINE_FIRED`]).
    deadline_fired: Option<String>,
    /// 호스트가 취합을 마감했다(저장 뒤 '읽기 전용으로 전환' 등).
    closed: bool,
    /// 마감 시각이 지났거나 호스트가 마감했다.
    read_only: bool,
    conns: HashMap<ConnId, Conn>,
    people: HashMap<String, Person>,
    pending: Vec<Pending>,
    last_presence: Vec<PresenceEntry>,
    /// 반영한 묶음(`이름 + NUL + 묶음 ID`). 다시 연결한 뒤 같은 묶음을 또 보내도 한 번만 반영한다.
    applied: HashSet<String>,
    log: Vec<ChangeEntry>,
    next_seq: u64,
    recalc: Option<mpsc::UnboundedSender<RecalcRequest>>,
    /// 계산 사본 판: 엑셀에서 불러온 행을 지울 때마다 는다([`RecalcRequest::epoch`]).
    recalc_epoch: u64,
    /// 호스트가 Excel로 저장하는 중([`SessionHandle::set_saving`]).
    saving: bool,
    /// 호스트가 관리하는 IP별 이름: 이 IP에서 오면 이름을 묻지 않고 그 이름으로 들어온다.
    ip_names: IpNames,
    /// 이 PC(호스트)의 IP. IP 명단에 있으면 그 이름이 호스트 이름이다.
    host_ips: Vec<IpAddr>,
    /// 정해 둔 호스트 이름([`SessionHandle::set_host_name`], 에이전트 PC에서 서비스할 때). 있으면 이것을 쓴다.
    host_name: Option<String>,
    // ---- 6단계
    comms: comms::Comms,
    snapshots: snapshots::Snapshots,
    store: Option<Store>,
    host_feed: broadcast::Sender<HostEvent>,
    persisted: persist::Persisted,
    /// 호스트가 남긴 메타([`Shared::set_meta`]). 시트를 다시 확정해 새 기록 파일이 오면 그 파일에도 쓴다.
    host_meta: BTreeMap<String, String>,
}

fn new_token() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// 이 PC의 IP(LAN 주소 + loopback).
fn local_ips() -> Vec<IpAddr> {
    let mut out: Vec<IpAddr> = crate::net::lan_addresses().iter().filter_map(|a| a.ip.parse().ok()).collect();
    out.push(IpAddr::V4(Ipv4Addr::LOCALHOST));
    out.push(IpAddr::V6(Ipv6Addr::LOCALHOST));
    out
}

fn parse_deadline(text: Option<&str>) -> Option<chrono::NaiveDateTime> {
    let t = text?.trim();
    chrono::NaiveDateTime::parse_from_str(t, "%Y-%m-%dT%H:%M")
        .or_else(|_| chrono::NaiveDateTime::parse_from_str(t, "%Y-%m-%dT%H:%M:%S"))
        .ok()
        .or_else(|| chrono::NaiveDate::parse_from_str(t, "%Y-%m-%d").ok()?.and_hms_opt(23, 59, 59))
}

impl State {
    fn new(init: SessionInit) -> Self {
        let SessionInit { sheets, store, restore } = init;
        let sheets: Vec<SheetSlot> = sheets.into_iter().map(|(m, c)| SheetSlot::new(m, c)).collect();
        let mut s = Self {
            rt: sheets.iter().map(SheetRt::new).collect(),
            sheets,
            roster: Vec::new(),
            any_formulas: false,
            deadline: None,
            deadline_fired: None,
            closed: false,
            read_only: false,
            conns: HashMap::new(),
            people: HashMap::new(),
            pending: Vec::new(),
            last_presence: Vec::new(),
            applied: HashSet::new(),
            log: Vec::new(),
            next_seq: 1,
            recalc: None,
            recalc_epoch: 0,
            saving: false,
            ip_names: IpNames::default(),
            host_ips: local_ips(),
            host_name: None,
            comms: comms::Comms::default(),
            snapshots: snapshots::Snapshots::default(),
            store,
            host_feed: broadcast::channel(HOST_FEED_CAPACITY).0,
            persisted: persist::Persisted::default(),
            host_meta: BTreeMap::new(),
        };
        match restore {
            Some(r) => {
                s.restore(r);
                s.deadline_fired = s.host_meta.get(META_DEADLINE_FIRED).cloned();
                s.recompile();
                s.persist_sheet();
            }
            None => {
                s.recompile();
                s.reset_snapshots();
                s.persist_all();
            }
        }
        s
    }

    fn feed(&self, event: HostEvent) {
        let _ = self.host_feed.send(event);
    }

    /// 클라이언트가 보낸 시트 순번(없는 시트면 `None`).
    fn sheet_index(&self, sheet: u32) -> Option<usize> {
        let i = sheet as usize;
        (i < self.sheets.len()).then_some(i)
    }

    /// 명령 하나를 처리한다. 호스트 작업이면 상태를 내보낸 뒤 부를 일을 돌려준다.
    fn handle(&mut self, cmd: Command) -> Option<Completion> {
        match cmd {
            Command::Connect { conn, ip, outbox } => {
                self.conns.insert(conn, Conn { outbox, ip, name: None });
                if let Some(name) = self.known_name(conn) {
                    self.send(conn, ServerMsg::Known { name });
                }
            }
            Command::Client { conn, msg } => self.client(conn, msg),
            Command::Disconnect { conn } => self.disconnect(conn),
            Command::Approve { name, range, note, reply } => {
                let _ = reply.send(self.approve(&name, range, note));
                self.persist(true); // 명단·범위 배정이 바뀌었다
                return None;
            }
            Command::Reject { name, reply } => {
                let _ = reply.send(self.reject(&name));
            }
            Command::Kick { name, reply } => {
                let _ = reply.send(self.kick(&name));
            }
            Command::Unlock { name, reply } => {
                let _ = reply.send(self.host_unlock(&name));
            }
            Command::With(job) => {
                let done = self.host_job(job);
                self.persist(true);
                return Some(done);
            }
            Command::SetIpNames(names) => self.set_ip_names(names),
            Command::SetRecalc(tx) => {
                self.recalc = tx;
                for (rt, slot) in self.rt.iter_mut().zip(&self.sheets) {
                    rt.recalc_rows = slot.baseline.rows;
                    rt.recalc_cols = slot.baseline.cols;
                }
            }
            Command::Formulas { values, epoch } => self.formulas(values, epoch),
            Command::CalcLayout(reply) => {
                let _ = reply.send(self.calc_layout());
            }
            Command::SetSaving(on) => self.saving = on,
            Command::SetHostName(name) => self.host_name = name,
            Command::Backup { to, reply } => {
                self.persist(true);
                let out = match &self.store {
                    Some(store) => store.backup_request(to).map_err(|e| e.to_string()),
                    None => Err("기록 파일이 없는 업무라 넘길 수 없습니다".into()),
                };
                let _ = reply.send(out);
                return None;
            }
            Command::Tick => {
                self.expire_locks();
                self.refresh_presence();
                self.auto_snapshot();
            }
            Command::DeadlineTick => self.check_deadline(),
        }
        self.persist(false);
        None
    }

    fn client(&mut self, conn: ConnId, msg: ClientMsg) {
        match msg {
            ClientMsg::Join { name, token, protocol } => self.join(conn, &name, token, protocol),
            ClientMsg::SwitchDevice { name } => self.switch_device(conn, &name),
            ClientMsg::Cursor { sheet, at } => self.cursor(conn, sheet, at),
            ClientMsg::Ping => {
                self.touch(conn);
                self.send(conn, ServerMsg::Pong { now: edit::now_ms() });
            }
            ClientMsg::Lock { sheet, at, layout } => self.lock(conn, sheet, at, layout),
            ClientMsg::Unlock { sheet, at } => self.unlock(conn, sheet, at),
            ClientMsg::SetCells { sheet, batch, cells, origin, layout } => {
                self.set_cells(conn, sheet, batch, cells, origin.unwrap_or_default(), layout)
            }
            ClientMsg::RowHistory { sheet, row } => self.row_history(conn, sheet, row),
            ClientMsg::Format { sheet, range, patch, layout } => self.format(conn, sheet, range, patch, layout),
            ClientMsg::AckNotice { id } => self.ack_notice(conn, id),
            ClientMsg::AckHighlight { id } => self.ack_highlight(conn, id),
            ClientMsg::Chat { to, text } => self.chat(conn, to, text),
            ClientMsg::ChatReact { id, kind } => self.chat_react(conn, id, kind),
            ClientMsg::AddRows { sheet, count } => {
                self.participant_structure(conn, sheet, StructureEdit::AddRows { count }, None)
            }
            ClientMsg::DeleteRows { sheet, from, count, layout } => {
                self.participant_structure(conn, sheet, StructureEdit::DeleteRows { from, count }, layout)
            }
            ClientMsg::CancelRows { sheet, from, count, on, layout } => {
                self.participant_structure(conn, sheet, StructureEdit::CancelRows { from, count, on }, layout)
            }
            ClientMsg::Resize { sheet, axis, from, count, size } => self.resize(conn, sheet, axis, from, count, size),
        }
    }

    // ------------------------------------------------------------ 보내기

    fn send(&self, conn: ConnId, msg: ServerMsg) {
        if let Some(c) = self.conns.get(&conn) {
            let _ = c.outbox.send(Outgoing::Msg(msg));
        }
    }

    fn close(&self, conn: ConnId) {
        if let Some(c) = self.conns.get(&conn) {
            let _ = c.outbox.send(Outgoing::Close);
        }
    }

    /// 입장한 연결 모두에게(`except` 빼고).
    fn broadcast(&self, msg: &ServerMsg, except: Option<ConnId>) {
        for (id, c) in &self.conns {
            if c.name.is_some() && Some(*id) != except {
                let _ = c.outbox.send(Outgoing::Msg(msg.clone()));
            }
        }
    }

    /// 명단에서 그 이름의 인덱스(색).
    fn person(&self, name: &str) -> Option<usize> {
        self.roster.iter().position(|p| p.trim() == name.trim())
    }

    /// 입장한 연결과 그 사람(명단 인덱스).
    fn attached(&self) -> Vec<(ConnId, usize)> {
        self.conns.iter().filter_map(|(id, c)| Some((*id, self.person(c.name.as_deref()?)?))).collect()
    }

    fn person_of(&self, conn: ConnId) -> Option<(String, usize)> {
        let name = self.conns.get(&conn)?.name.clone()?;
        let p = self.person(&name)?;
        Some((name, p))
    }

    /// 그 사람에게 그 시트의 그 행이 보이는가('남의 행 숨김'이면 자기 행과 머리글만).
    fn row_visible(&self, sheet: usize, person: usize, row: u32) -> bool {
        self.rt[sheet].visible.get(&person).is_none_or(|v| v.get(row as usize).copied().unwrap_or(false))
    }

    // ------------------------------------------------------------ 접속

    /// 연결한 IP가 IP 명단에 있으면 그 이름.
    fn known_name(&self, conn: ConnId) -> Option<String> {
        let c = self.conns.get(&conn)?;
        self.ip_names.get(c.ip).map(str::to_string)
    }

    fn join(&mut self, conn: ConnId, name: &str, token: Option<String>, protocol: u32) {
        if protocol != PROTOCOL_VERSION {
            self.send(conn, ServerMsg::Rejected { reason: "화면이 오래되었습니다. 새로 고침해 주세요".into() });
            self.close(conn);
            return;
        }
        if self.conns.get(&conn).is_none_or(|c| c.name.is_some()) {
            return; // 이미 입장한 연결
        }
        let token = token.filter(|t| t.len() <= MAX_TOKEN_CHARS);
        if let Some(known) = self.known_name(conn) {
            return self.join_known(conn, &known, token);
        }
        let name = name.trim();
        if name.is_empty() || name.chars().count() > MAX_NAME_CHARS {
            self.send(conn, ServerMsg::Rejected { reason: format!("이름을 확인해 주세요 (1~{MAX_NAME_CHARS}자)") });
            return;
        }
        // 승인받은 적 있는 브라우저의 재접속(토큰 일치): 남아 있던 연결이 있으면 이쪽으로 넘긴다.
        if self.person(name).is_some()
            && let (Some(token), Some(p)) = (&token, self.people.get(name))
            && p.token == *token
        {
            if let Some(old) = p.conn.filter(|&c| c != conn) {
                self.kick_conn(old, KickReason::SwitchedDevice);
            }
            self.attach(conn, name, Some(token.clone()));
            return;
        }
        // IP 명단 밖: 호스트 승인을 기다린다. 같은 이름으로 다시 요청하면 앞 요청을 대신한다.
        if let Some(i) = self.pending.iter().position(|p| p.name == name) {
            let old = self.pending.remove(i).conn;
            if old != conn {
                self.send(old, ServerMsg::Rejected { reason: "다른 창에서 다시 요청했습니다".into() });
                self.close(old);
            }
        }
        self.pending.retain(|p| p.conn != conn);
        self.pending.push(Pending { name: name.to_string(), conn });
        self.send(conn, ServerMsg::Pending { name: name.to_string() });
    }

    /// 명단에 이름을 더한다(공통 명단 = 첫 시트 설정).
    fn add_person(&mut self, name: &str) -> Result<(), String> {
        let first = &mut self.sheets[0];
        first.config.apply(&first.model, ConfigEdit::AddPerson { name: name.into() }).map_err(|e| e.to_string())
    }

    /// IP 명단에 있는 IP: 그 이름으로 바로 들어온다(명단에 없던 이름이면 명단에 더한다). 같은 이름이 다른 연결에서
    /// 접속 중이면 토큰이 맞을 때만 넘기고, 아니면 '이 기기로 전환'을 묻는다.
    fn join_known(&mut self, conn: ConnId, name: &str, token: Option<String>) {
        self.pending.retain(|p| p.conn != conn);
        if let Some(p) = self.people.get(name)
            && let Some(old) = p.conn.filter(|&c| c != conn)
        {
            if token.as_deref() != Some(p.token.as_str()) {
                self.send(conn, ServerMsg::DupName { name: name.to_string() });
                return;
            }
            self.kick_conn(old, KickReason::SwitchedDevice);
        }
        if self.person(name).is_none() {
            if let Err(e) = self.add_person(name) {
                self.send(conn, ServerMsg::Rejected { reason: e });
                return;
            }
            self.recompile();
            self.resync();
            self.persist(true);
        }
        let token = token.filter(|t| self.people.get(name).is_some_and(|p| p.token == *t));
        self.attach(conn, name, token);
    }

    /// IP 명단을 바꾼다. 새로 든 IP에서 기다리던(승인 대기·이름 입력 중) 연결은 그 이름으로 들인다.
    fn set_ip_names(&mut self, names: IpNames) {
        self.ip_names = names;
        let waiting: Vec<(ConnId, String)> = self
            .conns
            .iter()
            .filter(|(_, c)| c.name.is_none())
            .filter_map(|(id, c)| Some((*id, self.ip_names.get(c.ip)?.to_string())))
            .collect();
        for (conn, name) in waiting {
            if self.pending.iter().any(|p| p.conn == conn) {
                self.join_known(conn, &name, None);
            } else {
                self.send(conn, ServerMsg::Known { name });
            }
        }
        self.refresh_presence(); // 호스트 이름이 바뀌었을 수 있다
    }

    /// '이 기기로 전환': IP 명단의 이름으로 접속 중인 다른 연결을 끊고 이 연결로 들어온다.
    fn switch_device(&mut self, conn: ConnId, _name: &str) {
        let Some(name) = self.known_name(conn) else { return };
        if self.conns.get(&conn).is_none_or(|c| c.name.is_some()) {
            return;
        }
        if let Some(old) = self.people.get(&name).and_then(|p| p.conn).filter(|&c| c != conn) {
            self.kick_conn(old, KickReason::SwitchedDevice);
        }
        self.join_known(conn, &name, None);
    }

    fn kick_conn(&mut self, conn: ConnId, reason: KickReason) {
        self.send(conn, ServerMsg::Kicked { reason });
        self.close(conn);
        if let Some(c) = self.conns.get_mut(&conn)
            && let Some(name) = c.name.take()
            && let Some(p) = self.people.get_mut(&name)
            && p.conn == Some(conn)
        {
            p.conn = None;
        }
    }

    fn attach(&mut self, conn: ConnId, name: &str, token: Option<String>) {
        let Some(ip) = self.conns.get(&conn).map(|c| c.ip) else { return };
        let token = token.unwrap_or_else(new_token);
        let now = Instant::now();
        let p = self.people.entry(name.to_string()).or_insert_with(|| Person {
            token: token.clone(),
            conn: None,
            last_active: now,
            cursor: None,
            ip: None,
        });
        p.token = token;
        p.conn = Some(conn);
        p.last_active = now;
        p.ip = Some(ip);
        if let Some(c) = self.conns.get_mut(&conn) {
            c.name = Some(name.to_string());
        }
        self.pending.retain(|q| q.conn != conn);
        // 끊긴 사이 남겨 둔 잠금은 다시 이 사람 것으로 되살린다.
        for rt in &mut self.rt {
            for lock in rt.locks.values_mut() {
                if lock.owner == name {
                    lock.orphaned = None;
                }
            }
        }
        if let Some(w) = self.welcome(name) {
            self.send(conn, ServerMsg::Welcome(Box::new(w)));
        }
        self.refresh_presence();
    }

    fn disconnect(&mut self, conn: ConnId) {
        self.pending.retain(|p| p.conn != conn);
        if let Some(c) = self.conns.remove(&conn)
            && let Some(name) = c.name
            && let Some(p) = self.people.get_mut(&name)
            && p.conn == Some(conn)
        {
            p.conn = None;
            p.cursor = None;
            let now = Instant::now();
            for rt in &mut self.rt {
                for lock in rt.locks.values_mut() {
                    if lock.owner == name {
                        lock.orphaned = Some(now);
                    }
                }
            }
            self.refresh_presence();
        }
    }

    fn touch(&mut self, conn: ConnId) {
        let Some(name) = self.conns.get(&conn).and_then(|c| c.name.clone()) else { return };
        if let Some(p) = self.people.get_mut(&name) {
            p.last_active = Instant::now();
        }
        self.refresh_presence(); // 자리 비움 → 접속 중으로 돌아왔을 수 있다
    }

    fn cursor(&mut self, conn: ConnId, sheet: u32, at: Option<CellRef>) {
        let Some(name) = self.conns.get(&conn).and_then(|c| c.name.clone()) else { return };
        let Some(si) = self.sheet_index(sheet) else { return };
        let model = &self.sheets[si].model;
        let at = at.filter(|a| a.row < model.rows && a.col < model.cols);
        if let Some(p) = self.people.get_mut(&name) {
            p.cursor = at.map(|a| SheetCell::new(sheet, a));
        }
        self.touch(conn);
        // '남의 행 숨김'이면 남의 행에 있는 커서는 보이지 않는다.
        let msg = ServerMsg::Cursor { name, sheet, at };
        for (id, p) in self.attached() {
            if id != conn && at.is_none_or(|a| self.row_visible(si, p, a.row)) {
                self.send(id, msg.clone());
            }
        }
    }

    // ------------------------------------------------------------ 호스트 명령

    fn approve(&mut self, name: &str, range: Option<(u32, CellRange)>, note: String) -> Result<(), String> {
        let name = name.trim();
        let waiting = self.pending.iter().find(|p| p.name == name).map(|p| p.conn);
        let mut changed = false;
        if self.person(name).is_none() {
            self.add_person(name)?;
            changed = true;
        }
        if let Some((sheet, range)) = range {
            let si = self.sheet_index(sheet).ok_or(NO_SHEET)?;
            let slot = &mut self.sheets[si];
            let edit = ConfigEdit::AssignRange { range, person: name.into(), note };
            slot.config.apply(&slot.model, edit).map_err(|e| e.to_string())?;
            changed = true;
        }
        // 명단에 이미 있는 이름을 그대로 들일 때는 권한이 그대로라 모두에게 시트를 다시 보내지 않는다.
        if changed {
            self.recompile();
            self.resync();
        }
        if let Some(conn) = waiting {
            // 호스트가 이 기기를 들였다: 같은 이름으로 다른 기기에서 접속 중이면 그쪽은 끊는다.
            if let Some(old) = self.people.get(name).and_then(|p| p.conn).filter(|&c| c != conn) {
                self.kick_conn(old, KickReason::SwitchedDevice);
            }
            self.attach(conn, name, None);
        }
        Ok(())
    }

    fn reject(&mut self, name: &str) -> Result<(), String> {
        let i = self.pending.iter().position(|p| p.name == name).ok_or("승인 대기 중이 아닙니다")?;
        let conn = self.pending.remove(i).conn;
        self.send(conn, ServerMsg::Rejected { reason: "호스트가 참여를 거절했습니다".into() });
        self.close(conn);
        Ok(())
    }

    fn kick(&mut self, name: &str) -> Result<(), String> {
        let conn = self.people.get(name).and_then(|p| p.conn).ok_or("접속 중이 아닙니다")?;
        self.kick_conn(conn, KickReason::Host);
        self.release_locks_of(name);
        self.refresh_presence();
        Ok(())
    }

    fn host_job(&mut self, job: HostJob) -> Completion {
        let host_name = self.host_name();
        let (changed, replacement, actions, done) = {
            let mut shared = Shared {
                sheets: &mut self.sheets,
                rt: &self.rt,
                history: &self.log,
                snapshots: &self.snapshots.list,
                closed: self.closed,
                saving: self.saving,
                host_name,
                changed: false,
                replacement: None,
                actions: Vec::new(),
            };
            let done = job(&mut shared);
            (shared.changed, shared.replacement, shared.actions, done)
        };
        let replaced = replacement.is_some();
        if let Some(sheets) = replacement {
            // 다른 시트(들): 셀 버전·잠금·이력을 새로 시작한다.
            self.sheets = sheets.into_iter().map(|(m, c)| SheetSlot::new(m, c)).collect();
            self.rt = self.sheets.iter().map(SheetRt::new).collect();
            self.applied.clear();
            self.log.clear();
            self.next_seq = 1;
        }
        if changed {
            self.recompile();
            self.resync();
            self.feed(HostEvent::Reset);
        }
        if replaced {
            // 다른 시트: 강조한 칸은 의미가 없다. 스냅샷은 새 원본부터.
            let ids: Vec<u32> = self.comms_snapshot().highlights.iter().filter(|h| h.active).map(|h| h.id).collect();
            for id in ids {
                self.clear_highlight(id);
            }
        }
        // 새 저장소가 오면 원본 스냅샷을 그 파일에 남기도록 스냅샷을 다시 시작한다.
        let new_store = actions.iter().any(|a| matches!(a, Action::Store(_)));
        for action in actions {
            match action {
                Action::Set { user, origin, sheet, cells } => self.host_writes(&user, origin, sheet, cells),
                Action::Snapshot { kind, label } => {
                    self.take_snapshot(kind, &label);
                }
                Action::Close(closed) => {
                    if closed != self.closed {
                        self.closed = closed;
                        self.update_read_only();
                    }
                }
                Action::Notice { text, important, target } => self.send_notice(text, important, target),
                Action::Highlight { sheet, range, note, audience } => self.add_highlight(sheet, range, note, audience),
                Action::ClearHighlight(id) => self.clear_highlight(id),
                Action::Chat { to, text } => self.host_chat(to, text),
                Action::ChatReact { id, kind } => self.host_chat_react(id, kind),
                Action::Store(store) => {
                    store.append(&self.log);
                    self.store = Some(store);
                }
                Action::Meta(k, v) => {
                    if let Some(store) = &self.store {
                        store.meta(&k, &v);
                    }
                    self.host_meta.insert(k, v);
                }
                Action::Resized { sheet, axis, from, count, size } => {
                    let by = self.host_name();
                    self.resized(sheet, &by, axis, from, count, size);
                }
                Action::Structure { sheet, edit } => {
                    let by = self.host_name();
                    self.apply_structure(sheet, edit, &by, None);
                }
            }
        }
        if replaced || new_store {
            self.reset_snapshots();
        }
        if new_store {
            self.persist_all();
        } else if changed {
            self.persist_sheet();
        }
        done
    }

    /// 권한·진행률·보이는 행·마감을 다시 계산한다(시트·설정이 바뀜).
    fn recompile(&mut self) {
        sync_common(&mut self.sheets);
        self.roster = union_roster(self.sheets.iter().map(|s| (&s.model, &s.config)));
        let many = self.sheets.len() > 1;
        for (slot, rt) in self.sheets.iter().zip(self.rt.iter_mut()) {
            let model = &slot.model;
            rt.access = Access::compile_with(model, &slot.config, &self.roster);
            rt.progress = rt.access.progress(model);
            rt.labels = column_labels(model, &TableGuess { range: model.bounds(), header_rows: model.header_rows });
            rt.has_formulas = model.cells.iter().any(|c| c.f.is_some());
            rt.visible.clear();
            if slot.config.permissions.others_rows == OthersRows::Hidden {
                for p in 0..self.roster.len() {
                    let mut v = vec![false; model.rows as usize];
                    for r in 0..model.header_rows.min(model.rows) {
                        v[r as usize] = true;
                    }
                    for r in rt.access.rows_of(p) {
                        v[r as usize] = true;
                    }
                    rt.visible.insert(p, v);
                }
            }
            rt.assignment = (0..self.roster.len())
                .map(|p| {
                    let summary = assignment_summary(model, &slot.config, &rt.access, p);
                    match (many, summary.as_str()) {
                        (true, NO_ASSIGNMENT) => String::new(),
                        (true, _) => format!("{}: {summary}", model.sheet),
                        _ => summary,
                    }
                })
                .collect();
        }
        self.any_formulas = self.rt.iter().any(|rt| rt.has_formulas);
        self.deadline = parse_deadline(self.sheets[0].config.deadline.as_deref());
        // 마감을 지난 시각으로 옮겼거나 마감 동작을 바꿨으면 바로 알린다(편집 잠금도 푼다).
        self.update_read_only();
        // 더는 편집할 수 없는 칸의 잠금은 푼다.
        for (slot, rt) in self.sheets.iter().zip(self.rt.iter_mut()) {
            let cols = slot.model.cols;
            let read_only = self.read_only;
            let roster = &self.roster;
            let access = &rt.access;
            let stale: Vec<usize> = rt
                .locks
                .iter()
                .filter(|(i, l)| {
                    let at = CellRef::new(**i as u32 / cols.max(1), **i as u32 % cols.max(1));
                    read_only
                        || roster
                            .iter()
                            .position(|p| p.trim() == l.owner.trim())
                            .is_none_or(|p| !access.can_edit(Viewer::Person(p), at))
                })
                .map(|(i, _)| *i)
                .collect();
            for i in stale {
                rt.locks.remove(&i);
            }
        }
    }

    /// 접속 중인 사람 모두에게 새 시트를 다시 보낸다. 명단에서 빠진 사람은 끊는다.
    fn resync(&mut self) {
        let attached: Vec<(ConnId, String)> =
            self.conns.iter().filter_map(|(id, c)| c.name.clone().map(|n| (*id, n))).collect();
        for (conn, name) in attached {
            match self.welcome(&name) {
                Some(w) => self.send(conn, ServerMsg::Welcome(Box::new(w))),
                None => {
                    self.kick_conn(conn, KickReason::Removed);
                    self.release_locks_of(&name);
                }
            }
        }
        // 승인을 기다리던 사람은 명단에 그 이름이 생겨도 호스트가 승인해야 들어온다.
        self.refresh_presence();
    }

    fn deadline_action(&self) -> DeadlineAction {
        self.sheets[0].config.permissions.deadline_action
    }

    /// 마감 시각이 지났다(호스트 PC의 현지 시각으로 잰다).
    fn deadline_reached(&self) -> bool {
        self.deadline.is_some_and(|d| chrono::Local::now().naive_local() >= d)
    }

    /// 마감 시각이 지나 읽기 전용이다(마감 동작이 '읽기 전용으로 전환'일 때만).
    fn deadline_passed(&self) -> bool {
        self.deadline_action() == DeadlineAction::ReadOnly && self.deadline_reached()
    }

    /// 1초마다: 읽기 전용을 바꾸고, 마감 동작이 '공유 닫기'면 마감 시각이 지나는 순간 한 번 호스트 앱에 알린다.
    fn check_deadline(&mut self) {
        self.update_read_only();
        if self.deadline_action() != DeadlineAction::StopSharing || !self.deadline_reached() {
            return;
        }
        let Some(deadline) = self.sheets[0].config.deadline.clone() else { return };
        if self.deadline_fired.as_deref() == Some(deadline.as_str()) {
            return;
        }
        self.deadline_fired = Some(deadline.clone());
        if let Some(store) = &self.store {
            store.meta(META_DEADLINE_FIRED, &deadline);
        }
        self.host_meta.insert(META_DEADLINE_FIRED.to_string(), deadline);
        self.feed(HostEvent::DeadlineClose);
    }

    /// 마감 시각(유닉스 ms, 호스트 PC 시간대).
    fn deadline_at(&self) -> Option<i64> {
        use chrono::TimeZone;
        let d = self.deadline?;
        chrono::Local.from_local_datetime(&d).earliest().map(|t| t.timestamp_millis())
    }

    /// 마감 시각·호스트 마감에 따라 읽기 전용을 바꾸고, 바뀌었으면 모두에게 알린다(잡힌 편집 잠금은 푼다).
    fn update_read_only(&mut self) {
        let now = self.closed || self.deadline_passed();
        if now == self.read_only {
            return;
        }
        self.read_only = now;
        if now {
            let all: Vec<String> = self.rt.iter().flat_map(|rt| rt.locks.values().map(|l| l.owner.clone())).collect();
            for owner in all {
                self.release_locks_of(&owner);
            }
        }
        self.broadcast(&ServerMsg::ReadOnly { read_only: now, reason: self.read_only_reason() }, None);
    }

    fn read_only_reason(&self) -> Option<String> {
        if !self.read_only {
            None
        } else if self.closed {
            Some("호스트가 취합을 마감했습니다".into())
        } else {
            Some("마감 시각이 지났습니다".into())
        }
    }

    // ------------------------------------------------------------ 보기

    /// 그 사람의 필수 진행률(모든 시트 합).
    fn total_progress(&self, p: usize) -> PersonProgress {
        let mut out =
            PersonProgress { name: self.roster.get(p).cloned().unwrap_or_default(), rows: 0, required: 0, filled: 0 };
        for rt in &self.rt {
            if let Some(x) = rt.progress.by_person.get(p) {
                out.rows += x.rows;
                out.required += x.required;
                out.filled += x.filled;
            }
        }
        out
    }

    fn sheet_view(&self, sheet: usize, name: &str, p: usize) -> SheetView {
        let (slot, rt) = (&self.sheets[sheet], &self.rt[sheet]);
        let projection = project_with(&rt.access, &slot.model, &slot.config, p);
        let hidden = slot.config.permissions.others_rows == OthersRows::Hidden;
        let row_owner = rt.access.row_owners().iter().map(|&o| if hidden && o != p as i32 { -1 } else { o }).collect();
        let cols = slot.model.cols.max(1);
        let versions = rt
            .versions
            .iter()
            .enumerate()
            .filter(|(i, v)| **v > 0 && self.row_visible(sheet, p, *i as u32 / cols))
            .map(|(i, v)| (i as u32, *v))
            .collect();
        let locks = rt
            .locks
            .iter()
            .filter(|(i, l)| l.owner != name && self.row_visible(sheet, p, **i as u32 / cols))
            .map(|(i, l)| LockEntry { at: rt.cell_of(*i, cols), by: l.owner.clone() })
            .collect();
        SheetView {
            model: projection.model,
            flags: encode_flags(&projection.flags),
            versions,
            locks,
            columns: slot.config.columns.clone(),
            labels: rt.labels.clone(),
            permissions: slot.config.permissions,
            row_owner,
            progress: rt.progress.by_person.get(p).cloned().unwrap_or_default(),
            assigned: slot.config.has_assignment(),
        }
    }

    fn welcome(&self, name: &str) -> Option<Welcome> {
        let p = self.person(name)?;
        let token = self.people.get(name).map(|x| x.token.clone()).unwrap_or_default();
        Some(Welcome {
            protocol: PROTOCOL_VERSION,
            name: name.to_string(),
            token,
            color: p as i32,
            session: self.info(),
            server_now: edit::now_ms(),
            roster: self.roster.clone(),
            sheets: (0..self.sheets.len()).map(|i| self.sheet_view(i, name, p)).collect(),
            presence: self.presence(),
            notices: self.notices_for(name),
            highlights: self.highlights_for(name),
            chat: self.chat_for(name),
        })
    }

    fn status_of(&self, p: &Person) -> Option<PresenceStatus> {
        p.conn?;
        Some(if p.last_active.elapsed() >= AWAY_AFTER { PresenceStatus::Away } else { PresenceStatus::Online })
    }

    fn presence(&self) -> Vec<PresenceEntry> {
        let mut out = vec![PresenceEntry {
            name: self.host_name(),
            color: -1,
            status: PresenceStatus::Online,
            cursor: None,
            host: true,
        }];
        for (i, name) in self.roster.iter().enumerate() {
            let Some(p) = self.people.get(name) else { continue };
            let Some(status) = self.status_of(p) else { continue };
            out.push(PresenceEntry { name: name.clone(), color: i as i32, status, cursor: p.cursor, host: false });
        }
        out
    }

    /// 접속 상태가 바뀌었으면(커서 제외) 모두에게 알린다.
    fn refresh_presence(&mut self) {
        let now = self.presence();
        let key = |v: &[PresenceEntry]| v.iter().map(|e| (e.name.clone(), e.status)).collect::<Vec<_>>();
        if key(&now) != key(&self.last_presence) {
            self.broadcast(&ServerMsg::Presence { people: now.clone() }, None);
        }
        self.last_presence = now;
    }

    fn info(&self) -> SessionInfo {
        let config = &self.sheets[0].config;
        SessionInfo {
            title: config.title.clone(),
            description: config.description.clone(),
            requester: config.requester.clone(),
            deadline: config.deadline.clone(),
            deadline_at: self.deadline_at(),
            deadline_action: config.permissions.deadline_action,
            read_only: self.read_only,
            closed: self.closed,
            online: self.people.values().filter(|p| p.conn.is_some()).count() as u32,
            job: self.host_meta.get(META_JOB).cloned(),
        }
    }

    /// 그 사람이 잠근 칸(모든 시트 중 하나).
    fn editing_of(&self, name: &str) -> Option<SheetCell> {
        self.rt.iter().enumerate().find_map(|(s, rt)| {
            rt.locks
                .iter()
                .find(|(_, l)| l.owner == name)
                .map(|(i, _)| SheetCell::new(s as u32, rt.cell_of(*i, self.sheets[s].model.cols)))
        })
    }

    fn snapshot(&self) -> HostSnapshot {
        let participants: Vec<ParticipantRow> = self
            .roster
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let person = self.people.get(name);
                let status = match person.and_then(|p| self.status_of(p)) {
                    Some(PresenceStatus::Online) => ParticipantStatus::Online,
                    Some(PresenceStatus::Away) => ParticipantStatus::Away,
                    None => ParticipantStatus::Offline,
                };
                let progress = self.total_progress(i);
                let parts: Vec<&str> = self
                    .rt
                    .iter()
                    .filter_map(|rt| rt.assignment.get(i).map(String::as_str))
                    .filter(|a| !a.is_empty())
                    .collect();
                ParticipantRow {
                    name: name.clone(),
                    color: i as i32,
                    status,
                    idle_mins: person
                        .filter(|p| p.conn.is_some())
                        .map_or(0, |p| (p.last_active.elapsed().as_secs() / 60) as u32),
                    assignment: if parts.is_empty() { NO_ASSIGNMENT.into() } else { parts.join(" · ") },
                    cursor: person.and_then(|p| p.cursor),
                    editing: self.editing_of(name),
                    required: progress.required,
                    filled: progress.filled,
                    ip: person.and_then(|p| p.ip).map(|ip| ip.to_string()),
                }
            })
            .collect();
        HostSnapshot {
            online: participants.iter().filter(|p| p.status != ParticipantStatus::Offline).count() as u32,
            roster: participants.len() as u32,
            participants,
            pending: self
                .pending
                .iter()
                .map(|p| PendingRow {
                    name: p.name.clone(),
                    ip: self.conns.get(&p.conn).map(|c| c.ip.to_string()).unwrap_or_default(),
                })
                .collect(),
            required: self.rt.iter().map(|rt| rt.progress.required).sum(),
            filled: self.rt.iter().map(|rt| rt.progress.filled).sum(),
            read_only: self.read_only,
            closed: self.closed,
            host_name: self.host_name(),
            sheets: self
                .sheets
                .iter()
                .zip(&self.rt)
                .map(|(s, rt)| SheetProgress {
                    name: s.model.sheet.clone(),
                    required: rt.progress.required,
                    filled: rt.progress.filled,
                })
                .collect(),
        }
    }
}

const NO_ASSIGNMENT: &str = "배정 없음";

/// 배정 요약(시트 기준 행 번호·주소): "행 3–7, 13–17 + I3:I1002", 구간이 많으면 "행 3–7 외 39구간(200행)".
pub fn assignment_summary(model: &SheetModel, config: &CollectConfig, access: &Access, person: usize) -> String {
    let mut runs: Vec<(u32, u32)> = Vec::new();
    for (r, &owner) in access.row_owners().iter().enumerate() {
        if owner != person as i32 {
            continue;
        }
        let r = r as u32;
        match runs.last_mut() {
            Some((_, end)) if *end + 1 == r => *end = r,
            _ => runs.push((r, r)),
        }
    }
    let row = |r: u32| model.origin.row + r + 1;
    let span = |(a, b): &(u32, u32)| if a == b { format!("{}", row(*a)) } else { format!("{}–{}", row(*a), row(*b)) };
    let mut parts = Vec::new();
    if !runs.is_empty() {
        let total: u32 = runs.iter().map(|(a, b)| b - a + 1).sum();
        parts.push(if runs.len() <= 2 {
            format!("행 {}", runs.iter().map(span).collect::<Vec<_>>().join(", "))
        } else {
            format!("행 {} 외 {}구간({total}행)", span(&runs[0]), runs.len() - 1)
        });
    }
    let Some(name) = access.roster().get(person) else { return NO_ASSIGNMENT.into() };
    for a in config.assignment.ranges.iter().filter(|a| a.person.trim() == name) {
        let r = a.range;
        parts.push(CellRange::new(r.row + model.origin.row, r.col + model.origin.col, r.rows, r.cols).to_string());
    }
    if parts.is_empty() { NO_ASSIGNMENT.into() } else { parts.join(" + ") }
}

/// 시트 이름을 붙인 시트 주소(`인력계획!F7`). 시트가 하나뿐이면 이름 없이 `F7`.
pub fn sheet_address(sheets: &[SheetSlot], sheet: usize, at: CellRef) -> String {
    let Some(s) = sheets.get(sheet) else { return String::new() };
    let addr = CellRef::new(at.row + s.model.origin.row, at.col + s.model.origin.col).to_string();
    if sheets.len() > 1 { format!("{}{addr}", sheet_prefix(&s.model.sheet)) } else { addr }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod edit_tests;

#[cfg(test)]
mod save_tests;

#[cfg(test)]
mod comms_tests;

#[cfg(test)]
mod persist_tests;

#[cfg(test)]
mod sheets_tests;

#[cfg(test)]
mod structure_tests;
