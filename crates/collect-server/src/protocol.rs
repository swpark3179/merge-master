//! 참여자 WebSocket 프로토콜 (JSON, `type`으로 구분).
//!
//! - 접속: IP 명단(그 IP면 그 이름으로 바로 입장), 명단 밖 IP는 이름 + 호스트 승인, 기기 전환, 재접속 토큰
//! - 프레즌스: 접속 상태·커서(3단계)
//! - 편집: 셀 잠금, 셀 수정 묶음(기준 버전으로 충돌 감지), 진행률, 마감 읽기 전용, 서식, 행 이력(4단계)
//! - 소통: 공지(읽음 확인), 셀 강조(확인), 채팅(전체·호스트와 1:1)(6단계)
//! - 행 추가·삭제: 참여자가 표 아래에 행을 붙이고(참여자 권한 '행 추가') 자기가 붙인 행을 지운다('행 삭제')
//! - 취소 행: 행을 취소 표시한다(행 전체 회색 + 취소선, 필수로 세지 않는다)
//! - 크기 조절: 열 너비·행 높이(호스트, 참여자 권한 '크기 조절'이면 참여자도). 모두에게 보인다
//! - 시각: 남은 시간은 호스트 PC 시계 기준(`welcome.serverNow`·`pong.now`로 차이를 잡고 `session.deadlineAt`과 비교)

pub use collect_core::access::PersonProgress;
use collect_core::config::{ColumnRule, DeadlineAction, Permissions};
use collect_core::format::{Axis, StylePatch};
pub use collect_core::history::{BatchOrigin, ChangeEntry, ChangeKind};
use collect_core::{CellRange, CellRef, CellStyle, CellValue, SheetCell, SheetModel};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// 2: 4단계 편집 메시지 추가. 3: 6단계 공지·강조·채팅, 묶음 출처. 4: IP 명단 입장(`known`), 행 추가.
/// 5: 여러 시트(칸·범위가 든 메시지마다 `sheet` = 세션 안 시트 순번, `welcome.sheets`).
/// 6: 열 너비·행 높이(`resize` → 모두에게 `resized`).
/// 7: 행 삭제·취소 행(`deleteRows`·`cancelRows`, 편집의 `layout`), 호스트 시각(`serverNow`·`deadlineAt`), 마감 동작.
/// 8: 채팅 리액션(`chatReact` → 보이는 사람에게 `chatReactions`, `ChatMessage.reactions`).
/// 9: 수식 칸을 값으로 바꾸기(수식이 사라지면 새 시트 `welcome`), 엑셀에서 불러온 행 지우기(내가 추가했거나 내게 배정된
/// 행), 에이전트 PC의 자동 승인(`session.autoApprove`, 승인 대기의 까닭 `pending.reason`).
pub const PROTOCOL_VERSION: u32 = 9;

/// 채팅 받는 쪽. 참여자는 전체나 호스트에게, 호스트는 전체나 한 사람에게 보낸다.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum ChatTo {
    All,
    Host,
    Person { name: String },
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ChatMessage {
    pub id: u32,
    #[ts(type = "number")]
    pub at_ms: i64,
    pub from: String,
    /// 호스트가 보냄.
    pub host: bool,
    pub to: ChatTo,
    pub text: String,
    /// 단 순서. 옛 기록 파일에는 없다.
    #[serde(default)]
    pub reactions: Vec<ChatReactionMark>,
}

/// 채팅 리액션(아이콘은 화면이 정한다).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ChatReaction {
    Ok,
    Yes,
    No,
    Thanks,
    Good,
    Sad,
}

/// 리액션 하나. 호스트는 이름이 바뀔 수 있어 `host`로 가린다(`by`는 달 때의 이름).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ChatReactionMark {
    pub kind: ChatReaction,
    pub by: String,
    pub host: bool,
}

/// 참여자가 받는 공지.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct NoticeView {
    pub id: u32,
    #[ts(type = "number")]
    pub at_ms: i64,
    pub from: String,
    pub text: String,
    /// 확인할 때까지 화면 위에 고정한다.
    pub important: bool,
    /// 이 사람이 확인했다.
    pub read: bool,
}

/// 참여자가 받는 셀 강조(노란 표시 + 말풍선).
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HighlightView {
    pub id: u32,
    #[ts(type = "number")]
    pub at_ms: i64,
    pub from: String,
    pub sheet: u32,
    /// 그 시트의 모델 좌표.
    pub range: CellRange,
    pub note: String,
    /// 이 사람이 확인했다.
    pub acked: bool,
}

/// 셀 하나의 수정. `base`는 클라이언트가 마지막으로 본 그 칸의 버전(충돌 감지용).
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, TS)]
#[ts(export)]
pub struct CellEdit {
    pub row: u32,
    pub col: u32,
    #[ts(as = "Option<CellValue>", optional)]
    #[serde(default)]
    pub value: CellValue,
    pub base: u32,
}

#[derive(Debug, Clone, Deserialize, Serialize, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(export)]
pub enum ClientMsg {
    /// 이름으로 참여. 전에 받은 `token`이 있으면 같이 보내 재접속한다. IP 명단에 있는 IP면 `name`은 무시하고
    /// 명단의 이름으로 들어온다.
    Join {
        name: String,
        #[serde(default)]
        #[ts(optional)]
        token: Option<String>,
        #[serde(default)]
        protocol: u32,
    },
    /// `dupName`을 받은 뒤 "이 기기로 전환"을 누름: 다른 기기의 접속을 끊고 이 기기로 들어온다.
    SwitchDevice {
        name: String,
    },
    /// 내 커서 위치(그 시트의 모델 좌표). 다른 참여자에게 이름표로 보인다.
    Cursor {
        #[serde(default)]
        sheet: u32,
        #[serde(default)]
        #[ts(optional)]
        at: Option<CellRef>,
    },
    Ping,
    /// 칸 편집을 시작한다(다른 사람에게는 잠긴 칸으로 보인다). 결과는 `lockResult`. 한 사람은 모든 시트를 통틀어
    /// 한 칸만 잠근다.
    Lock {
        #[serde(default)]
        sheet: u32,
        at: CellRef,
        /// 클라이언트가 본 시트의 구조 판(`model.layout`). 다르면 거절한다(없으면 검사하지 않는다).
        #[serde(default)]
        #[ts(optional)]
        layout: Option<u32>,
    },
    /// 편집을 취소했다.
    Unlock {
        #[serde(default)]
        sheet: u32,
        at: CellRef,
    },
    /// 셀 수정 묶음(한 시트 안). 범위 지우기·붙여넣기는 한 묶음으로 보낸다(히스토리에서 한 번에 되돌림).
    /// `origin`은 히스토리·활동 피드 표시용(붙여넣기·범위 지우기·실행 취소·내 변경 되돌리기).
    SetCells {
        #[serde(default)]
        sheet: u32,
        batch: String,
        cells: Vec<CellEdit>,
        #[serde(default)]
        #[ts(optional)]
        origin: Option<BatchOrigin>,
        /// 묶음을 만들 때 본 시트의 구조 판. 그 뒤 앱에서 붙인 행·열을 지워 좌표가 밀렸으면 묶음 전체를 거절한다.
        #[serde(default)]
        #[ts(optional)]
        layout: Option<u32>,
    },
    /// 행 변경 이력(1g '이 행 변경 이력').
    RowHistory {
        #[serde(default)]
        sheet: u32,
        row: u32,
    },
    /// 서식 바꾸기(그 시트의 참여자 권한이 '배경색만'·'허용'일 때만).
    Format {
        #[serde(default)]
        sheet: u32,
        range: CellRange,
        patch: StylePatch,
        #[serde(default)]
        #[ts(optional)]
        layout: Option<u32>,
    },
    /// 공지를 확인했다.
    AckNotice {
        id: u32,
    },
    /// 강조한 셀을 확인했다.
    AckHighlight {
        id: u32,
    },
    Chat {
        to: ChatTo,
        text: String,
    },
    /// 남의 채팅에 리액션을 달거나 뗀다(같은 종류를 다시 보내면 뗀다).
    ChatReact {
        id: u32,
        kind: ChatReaction,
    },
    /// 그 시트 표 아래에 빈 행을 붙인다(그 시트의 참여자 권한 '행 추가'일 때만). 결과는 모두에게 새 시트(`welcome`)
    /// + `rowsAdded`.
    AddRows {
        #[serde(default)]
        sheet: u32,
        count: u32,
    },
    /// 내가 추가했거나 내게 배정된 행 `from`부터 `count`개를 지운다(그 시트의 참여자 권한 '행 삭제'일 때만 — 엑셀에서
    /// 불러온 행이면 저장할 때 Excel에서도 지운다). 결과는 모두에게 새 시트(`welcome`, 구조 판이 오른다) + `rowsDeleted`.
    DeleteRows {
        #[serde(default)]
        sheet: u32,
        from: u32,
        count: u32,
        #[serde(default)]
        #[ts(optional)]
        layout: Option<u32>,
    },
    /// 행 `from`부터 `count`개를 취소 표시하거나(`on`) 푼다(내가 입력할 수 있는 칸이 있는 행만). 결과는 모두에게 새 시트.
    CancelRows {
        #[serde(default)]
        sheet: u32,
        from: u32,
        count: u32,
        on: bool,
        #[serde(default)]
        #[ts(optional)]
        layout: Option<u32>,
    },
    /// `from`부터 `count`개 열 너비(행 높이)를 `size`(pt)로(그 시트의 참여자 권한 '크기 조절'일 때만, 행은 내게 보이는 행만).
    /// 결과는 모두에게 `resized`.
    Resize {
        #[serde(default)]
        sheet: u32,
        axis: Axis,
        from: u32,
        count: u32,
        size: f32,
    },
}

/// 접속 전에도 공개하는 취합 정보(1e 접속 화면).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SessionInfo {
    pub title: String,
    pub description: String,
    pub requester: String,
    /// 마감(호스트 PC의 현지 시각 `YYYY-MM-DDTHH:MM` — 화면에 그대로 보인다).
    #[ts(optional)]
    pub deadline: Option<String>,
    /// 마감 시각(유닉스 ms, 호스트 PC 시계·시간대로 계산). 남은 시간은 이것과 호스트 시각(`serverNow`)으로 센다.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "number")]
    pub deadline_at: Option<i64>,
    /// 마감 시각이 되면 읽기 전용으로 바꾸는지, 공유를 닫는지.
    #[serde(default)]
    pub deadline_action: DeadlineAction,
    /// 마감이 지나 입력할 수 없다.
    pub read_only: bool,
    /// 호스트가 취합을 마감했다(마감 시각과 별개).
    #[serde(default)]
    pub closed: bool,
    pub online: u32,
    /// 호스트 앱의 업무 id. 같은 주소 경로를 새 업무가 다시 쓰면 바뀐다(참여자가 앞 업무의 대기열·토큰을 버린다).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub job: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum PresenceStatus {
    Online,
    /// 3분 넘게 입력·이동이 없음.
    Away,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PresenceEntry {
    pub name: String,
    /// 명단 순서(색 고르기용). 호스트는 -1.
    pub color: i32,
    pub status: PresenceStatus,
    #[ts(optional)]
    pub cursor: Option<SheetCell>,
    pub host: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LockEntry {
    pub at: CellRef,
    pub by: String,
}

/// 시트 하나를 이 사람 기준으로(`welcome.sheets`, 불러온 순서). 시트 이름은 `model.sheet`.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SheetView {
    /// 이 사람에게 보내는 시트(남의 행 숨김이면 남의 행 값이 비어 있다).
    pub model: SheetModel,
    /// 셀 상태 비트(base64, 이 사람 기준 MINE 포함).
    pub flags: String,
    /// 한 번이라도 바뀐 칸의 버전: `[셀 인덱스(행 우선), 버전]`. 없는 칸은 0.
    pub versions: Vec<(u32, u32)>,
    pub locks: Vec<LockEntry>,
    pub columns: Vec<ColumnRule>,
    pub labels: Vec<String>,
    /// 이 시트의 참여자 권한(서식·남의 행·필터·행 추가). '마감 후 읽기 전용'은 모든 시트가 같다.
    pub permissions: Permissions,
    /// 행 담당자(명단 인덱스, 없으면 -1). 남의 행 숨김이면 내 행 말고는 -1.
    pub row_owner: Vec<i32>,
    /// 이 시트에서 내 필수 진행률.
    pub progress: PersonProgress,
    /// 호스트가 담당자를 배정했다(행 배정 기준 열이나 범위 배정). 아니면 '내 담당만' 필터를 보이지 않는다.
    #[serde(default)]
    pub assigned: bool,
}

/// 입장 허락. 설정이 바뀌면(호스트가 1b에서 고치거나 배정을 바꿈) 같은 내용으로 다시 보낸다.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Welcome {
    pub protocol: u32,
    pub name: String,
    /// 재접속용. 브라우저에 저장했다가 `join`에 같이 보낸다.
    pub token: String,
    pub color: i32,
    pub session: SessionInfo,
    /// 보낼 때의 호스트 PC 시각(유닉스 ms). 참여자는 자기 시계와의 차이를 잡아 마감까지 남은 시간을 호스트 기준으로 센다.
    #[ts(type = "number")]
    pub server_now: i64,
    /// 명단(모든 시트 명단의 합집합, 순서 = 색).
    pub roster: Vec<String>,
    /// 시트마다(불러온 순서, 하나 이상).
    pub sheets: Vec<SheetView>,
    pub presence: Vec<PresenceEntry>,
    /// 이 사람에게 온 공지(오래된 것부터).
    pub notices: Vec<NoticeView>,
    /// 이 사람에게 보이는 셀 강조(해제되지 않은 것).
    pub highlights: Vec<HighlightView>,
    /// 이 사람에게 보이는 채팅(최근 것까지).
    pub chat: Vec<ChatMessage>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum KickReason {
    /// 같은 이름으로 다른 기기(창)에서 들어옴.
    SwitchedDevice,
    /// 호스트가 연결을 끊음.
    Host,
    /// 호스트가 이 이름을 명단에서 뺌.
    Removed,
}

/// 적용된 칸(서버가 정규화한 값과 새 버전).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CellUpdate {
    pub row: u32,
    pub col: u32,
    #[ts(as = "Option<CellValue>", optional)]
    #[serde(default)]
    pub value: CellValue,
    pub ver: u32,
}

/// 받아들이지 않은 칸과 이유. 화면은 그 칸을 서버 값(`value`, `ver`)으로 되돌린다.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Rejection {
    pub row: u32,
    pub col: u32,
    pub reason: String,
    #[ts(as = "Option<CellValue>", optional)]
    #[serde(default)]
    pub value: CellValue,
    pub ver: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct StyleChange {
    pub row: u32,
    pub col: u32,
    pub s: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(export)]
pub enum ServerMsg {
    Welcome(Box<Welcome>),
    /// 연결한 IP가 호스트의 IP 명단에 있다: 이름을 묻지 않고 이 이름으로 들어온다(`join`을 보내면 된다).
    Known {
        name: String,
    },
    /// IP 명단 밖에서 접속: 호스트 승인을 기다린다.
    Pending {
        name: String,
    },
    /// 같은 이름이 다른 기기에서 접속 중.
    DupName {
        name: String,
    },
    Rejected {
        reason: String,
    },
    Kicked {
        reason: KickReason,
    },
    Presence {
        people: Vec<PresenceEntry>,
    },
    Cursor {
        name: String,
        sheet: u32,
        #[ts(optional)]
        at: Option<CellRef>,
    },
    /// `lock` 결과. 거절이면 `by`(먼저 잡은 사람) 또는 `reason`.
    LockResult {
        sheet: u32,
        at: CellRef,
        ok: bool,
        #[ts(optional)]
        by: Option<String>,
        #[ts(optional)]
        reason: Option<String>,
    },
    /// 다른 사람의 잠금이 바뀜(`by`가 없으면 풀림).
    LockChanged {
        sheet: u32,
        at: CellRef,
        #[ts(optional)]
        by: Option<String>,
    },
    /// 내 `setCells` 결과. `conflicts`는 내가 본 뒤 다른 사람이 바꾼 칸(내 값으로 덮어씀).
    Applied {
        sheet: u32,
        batch: String,
        cells: Vec<CellUpdate>,
        conflicts: Vec<CellRef>,
        rejected: Vec<Rejection>,
    },
    /// 다른 사람(또는 수식 재계산)이 바꾼 칸. `by`가 빈 문자열이면 수식 결과.
    Cells {
        sheet: u32,
        by: String,
        cells: Vec<CellUpdate>,
    },
    /// 그 시트에서 내 필수 진행률이 바뀜.
    Progress {
        sheet: u32,
        progress: PersonProgress,
    },
    /// 마감으로 읽기 전용이 되었거나 풀림. `reason`: 마감 시각이 지남 / 호스트가 마감함.
    ReadOnly {
        #[serde(rename = "readOnly")]
        read_only: bool,
        #[serde(default)]
        #[ts(optional)]
        reason: Option<String>,
    },
    /// 서식이 바뀜: 서식 표 전체와 바뀐 칸.
    Styles {
        sheet: u32,
        styles: Vec<CellStyle>,
        changes: Vec<StyleChange>,
    },
    History {
        sheet: u32,
        row: u32,
        entries: Vec<ChangeEntry>,
    },
    Notice(NoticeView),
    Highlight(HighlightView),
    HighlightCleared {
        id: u32,
    },
    Chat(ChatMessage),
    /// 그 채팅의 리액션이 바뀌었다(전체 목록).
    ChatReactions {
        id: u32,
        reactions: Vec<ChatReactionMark>,
    },
    /// 누군가 표 아래에 행을 붙였다(새 시트는 `welcome`으로 먼저 온다). 모델 좌표.
    RowsAdded {
        by: String,
        sheet: u32,
        range: CellRange,
    },
    /// 누군가(호스트 포함) 행을 지웠다(새 시트는 `welcome`으로 먼저 온다). 지운 뒤의 행은 위로 당겨졌다.
    RowsDeleted {
        by: String,
        sheet: u32,
        from: u32,
        count: u32,
    },
    /// 누군가(호스트 포함) 열 너비·행 높이를 바꿨다: `from`부터 `count`개 줄이 `size`(pt, 서버가 맞춘 값). 숨긴 줄
    /// (크기 0 — 남의 행 숨김으로 숨은 행 포함)은 그대로 둔다.
    Resized {
        by: String,
        sheet: u32,
        axis: Axis,
        from: u32,
        count: u32,
        size: f32,
    },
    /// 이 연결이 곧 닫힌다(호스트가 서비스·업무 공개를 멈춤). `ended`면 호스트가 업무를 끝냈다(다시 열리지 않는다).
    Closing {
        #[serde(default)]
        ended: bool,
        /// 마감 시각이 되어 공유를 닫았다(호스트가 다시 열면 이어서).
        #[serde(default)]
        deadline: bool,
    },
    /// `ping`의 답. `now`는 호스트 PC 시각(유닉스 ms).
    Pong {
        #[ts(type = "number")]
        now: i64,
    },
    Error {
        message: String,
    },
}
