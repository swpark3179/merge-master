//! 호스트 앱 ↔ 에이전트 관리 API(`/agent/*`)의 형식.
//!
//! 에이전트는 다른 PC에서 창 없이(트레이) 도는 프로그램이다. 호스트 앱은 공유를 시작하기 전에 업무를 에이전트로 넘기고
//! ([`Package`]: 넘길 내용 + 기록 파일 + 원본 통합문서), 에이전트가 그 PC에서 참여자 서버를 열어 마감 일시까지 서비스한다.
//! 마감이 되면 에이전트가 엑셀을 저장하고 그 서비스를 끝낸다.
//!
//! - 모든 요청에는 헤더 [`HEADER_AGENT`]`: `[`PROTOCOL`]이 있어야 한다(다르면 426). 브라우저 요청(`Origin` 헤더)은 받지 않는다.
//! - 서비스마다 호스트가 만든 관리 키(`token`)가 있어야 그 서비스를 승인·끝낼 수 있다(헤더 [`HEADER_TOKEN`]). 다른
//!   관리자가 실수로 끝내지 않게 하는 것이고 보안 경계는 아니다(같은 사내망 누구나 넘길 수 있다).
//! - 오류는 상태 코드 + [`ApiError`](한국어 문구).

#[cfg(feature = "client")]
pub mod client;

use std::collections::BTreeMap;

use chrono::NaiveDateTime;
use collect_core::export::ExportOptions;
use collect_server::RecalcStatus;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// 관리 API 판. 호스트 앱과 에이전트가 같아야 한다(다르면 에이전트를 바꾸라고 알린다).
pub const PROTOCOL: u32 = 1;
/// 에이전트의 기본 포트(참여자 접속과 관리 API를 같이 받는다).
pub const DEFAULT_PORT: u16 = 8090;
pub const HEADER_AGENT: &str = "x-mm-agent";
pub const HEADER_TOKEN: &str = "x-mm-token";
/// 넘길 묶음의 최대 크기.
pub const MAX_PACKAGE: usize = 512 * 1024 * 1024;
/// 묶음 안 넘길 내용(JSON)의 최대 크기.
const MAX_MANIFEST: usize = 4 * 1024 * 1024;
/// Excel이 다루는 경로의 최대 길이(글자).
pub const MAX_PATH_CHARS: usize = 218;
/// 마감은 지금부터 이만큼(분) 뒤여야 한다(에이전트가 받을 때). 호스트 화면은 5분 뒤부터 고르게 한다.
pub const MIN_LEAD_MINUTES: i64 = 1;
/// 호스트 화면이 고르게 하는 가장 이른 마감(지금부터 분).
pub const HOST_LEAD_MINUTES: i64 = 5;
/// 기본 최대 서비스 기간(일).
pub const DEFAULT_MAX_DAYS: u32 = 60;

/// `GET /agent/info`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AgentInfo {
    /// 에이전트 PC 이름.
    pub name: String,
    pub version: String,
    pub protocol: u32,
    /// 기본 저장 폴더(이 아래에만 저장한다).
    pub base_dir: String,
    /// 기본 저장 폴더가 있고 쓸 수 있다.
    pub base_ok: bool,
    pub port: u16,
    /// 이 에이전트의 주소들(`http://IP:포트/`).
    pub urls: Vec<String>,
    /// 에이전트 PC 시각(마감은 이 시계로 잰다).
    #[ts(type = "number")]
    pub now_ms: i64,
    pub utc_offset_minutes: i32,
    /// Excel이 설치되어 있다(마감 때 저장·수식 계산에 쓴다).
    pub excel: bool,
    /// 진행 중인 서비스 수.
    pub services: u32,
    /// 마감은 지금부터 이 날 수 안이어야 한다.
    pub max_days: u32,
}

/// `GET /agent/folders?path=a/b`: 기본 저장 폴더 아래 폴더 하나.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct FolderList {
    /// 기본 저장 폴더에서의 경로 조각.
    pub path: Vec<String>,
    /// 전체 경로(표시용).
    pub display: String,
    /// 그 폴더가 있다(없으면 저장할 때 만든다).
    pub exists: bool,
    pub folders: Vec<String>,
    /// 엑셀 파일 이름(같은 이름을 피할 때 참고).
    pub workbooks: Vec<String>,
}

/// 마감 때 저장할 곳: 기본 저장 폴더 아래 경로 조각 + 파일 이름(원본과 같은 확장자).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct OutputTarget {
    pub dir: Vec<String>,
    pub file_name: String,
}

/// 넘길 내용([`Package`] 안의 JSON).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HandoffManifest {
    /// 서비스 id와 관리 키: 호스트가 만든다(같은 것을 다시 보내면 이미 받은 서비스를 돌려준다).
    pub id: String,
    pub token: String,
    /// 호스트 앱의 업무 id.
    pub job_id: String,
    /// 넘긴 PC 이름·앱 버전(표시·로그용).
    pub host_pc: String,
    pub host_version: String,
    /// 참여자에게 보일 호스트 이름.
    #[serde(default)]
    pub host_name: Option<String>,
    pub title: String,
    /// 바라는 주소 경로(겹치면 에이전트가 `-2`, `-3`…을 붙인다).
    pub slug: String,
    /// 마감(`YYYY-MM-DDTHH:MM`, 에이전트 PC 시각).
    pub deadline: String,
    pub output: OutputTarget,
    pub export: ExportOptions,
    /// 원본 통합문서 파일 이름(확장자를 따른다).
    pub workbook_name: String,
    /// Excel 시트 이름(세션 안 순번 순서).
    pub sheets: Vec<String>,
    /// 호스트의 IP 명단(`{"IP": "이름"}`).
    #[serde(default)]
    pub ip_names: BTreeMap<String, String>,
}

/// `POST /agent/services` 결과.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Created {
    pub id: String,
    /// 에이전트가 정한 주소 경로.
    pub slug: String,
    pub urls: Vec<String>,
    pub list_urls: Vec<String>,
    /// 마감 때 저장할 파일(에이전트 PC 경로). 그때 같은 이름이 있으면 `_2`를 붙인다.
    pub output_path: String,
    pub deadline: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ServiceState {
    /// 서비스 중.
    Running,
    /// 마감이 되어(또는 '지금 저장하고 끝내기') 저장하는 중. 실패하면 1분 뒤 다시 한다.
    Saving,
    /// 저장하고 끝냈다.
    Done,
    /// 저장하지 못했다(다시 저장할 수 있다, 공유는 닫혔다).
    Failed,
    /// 저장 없이 끝냈다.
    Cancelled,
}

impl ServiceState {
    /// 끝난 서비스(다시 열지 않는다).
    pub fn finished(self) -> bool {
        matches!(self, ServiceState::Done | ServiceState::Cancelled)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PendingPerson {
    pub name: String,
    pub ip: String,
    /// 자동 승인인데도 기다리는 까닭(같은 이름이 접속 중, 호스트가 끊은 이름).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub reason: Option<collect_server::protocol::PendingReason>,
}

/// 서비스 하나의 상태(호스트 앱 '에이전트에서 진행 중').
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ServiceStatus {
    pub id: String,
    pub title: String,
    pub slug: String,
    pub state: ServiceState,
    pub urls: Vec<String>,
    pub list_urls: Vec<String>,
    pub deadline: String,
    #[ts(type = "number")]
    pub deadline_at_ms: i64,
    /// 에이전트 PC 시각.
    #[ts(type = "number")]
    pub now_ms: i64,
    pub online: u32,
    pub roster: u32,
    pub pending: Vec<PendingPerson>,
    pub required: u32,
    pub filled: u32,
    pub read_only: bool,
    pub recalc: RecalcStatus,
    pub output_path: String,
    #[ts(optional)]
    pub saved_path: Option<String>,
    #[ts(optional, type = "number")]
    pub saved_at_ms: Option<i64>,
    /// 저장을 시도한 횟수(실패 포함).
    pub attempts: u32,
    #[ts(optional)]
    pub last_error: Option<String>,
    /// 마지막 채팅 시각(채팅은 호스트 앱에서 볼 수 없다 — 있었다는 것만 알린다).
    #[ts(optional, type = "number")]
    pub last_chat_ms: Option<i64>,
    #[ts(type = "number")]
    pub created_at_ms: i64,
}

/// `POST /agent/status` 본문의 한 줄.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusQuery {
    pub id: String,
    pub token: String,
}

/// `POST /agent/status` 결과의 한 줄: 상태, 아니면 찾지 못한 까닭.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusItem {
    pub id: String,
    #[serde(default)]
    pub status: Option<ServiceStatus>,
    /// 에이전트에 없는 서비스다(지웠거나 다른 에이전트).
    #[serde(default)]
    pub missing: bool,
    /// 관리 키가 맞지 않는다.
    #[serde(default)]
    pub forbidden: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Approve {
    pub name: String,
    #[serde(default)]
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reject {
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Finish {
    /// 저장하고 끝낸다(아니면 저장 없이).
    pub save: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiError {
    pub message: String,
}

/// 넘길 묶음: 넘길 내용 + 기록 파일(SQLite) + 원본 통합문서.
#[derive(Debug, Clone, PartialEq)]
pub struct Package {
    pub manifest: HandoffManifest,
    pub store: Vec<u8>,
    pub workbook: Vec<u8>,
}

const MAGIC: &[u8; 8] = b"MMAGENT\0";
const FORMAT: u32 = 1;

impl Package {
    /// `MMAGENT\0` · 형식(u32) · 길이(u32) + 넘길 내용 JSON · 길이(u64) + 기록 파일 · 길이(u64) + 통합문서 (모두 little endian).
    pub fn encode(&self) -> Vec<u8> {
        let manifest = serde_json::to_vec(&self.manifest).expect("직렬화");
        let mut out = Vec::with_capacity(32 + manifest.len() + self.store.len() + self.workbook.len());
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&FORMAT.to_le_bytes());
        out.extend_from_slice(&(manifest.len() as u32).to_le_bytes());
        out.extend_from_slice(&manifest);
        out.extend_from_slice(&(self.store.len() as u64).to_le_bytes());
        out.extend_from_slice(&self.store);
        out.extend_from_slice(&(self.workbook.len() as u64).to_le_bytes());
        out.extend_from_slice(&self.workbook);
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Package, String> {
        let bad = || "넘길 묶음이 손상되었습니다".to_string();
        if bytes.len() > MAX_PACKAGE {
            return Err("넘길 묶음이 너무 큽니다".into());
        }
        let mut r = Reader { bytes, at: 0 };
        if r.take(MAGIC.len()).ok_or_else(bad)? != MAGIC {
            return Err(bad());
        }
        let format = r.u32().ok_or_else(bad)?;
        if format != FORMAT {
            return Err(format!(
                "알 수 없는 묶음 형식({format})입니다. 호스트 앱과 에이전트를 같은 버전으로 맞춰 주세요"
            ));
        }
        let len = r.u32().ok_or_else(bad)? as usize;
        if len > MAX_MANIFEST {
            return Err(bad());
        }
        let manifest: HandoffManifest = serde_json::from_slice(r.take(len).ok_or_else(bad)?)
            .map_err(|e| format!("넘길 내용을 읽을 수 없습니다: {e}"))?;
        let len = usize::try_from(r.u64().ok_or_else(bad)?).map_err(|_| bad())?;
        let store = r.take(len).ok_or_else(bad)?.to_vec();
        let len = usize::try_from(r.u64().ok_or_else(bad)?).map_err(|_| bad())?;
        let workbook = r.take(len).ok_or_else(bad)?.to_vec();
        if r.at != bytes.len() {
            return Err(bad());
        }
        Ok(Package { manifest, store, workbook })
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.at.checked_add(n)?;
        let out = self.bytes.get(self.at..end)?;
        self.at = end;
        Some(out)
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
}

/// 마감 글자(`YYYY-MM-DDTHH:MM`)를 읽고, `now`(마감을 잴 PC의 시각)부터 `lead_minutes`분 뒤 ~ `max_days`일 안인지 본다.
pub fn check_deadline(
    text: &str,
    now: NaiveDateTime,
    lead_minutes: i64,
    max_days: u32,
) -> Result<NaiveDateTime, String> {
    let at = NaiveDateTime::parse_from_str(text.trim(), "%Y-%m-%dT%H:%M")
        .map_err(|_| "마감 일시를 날짜·시·분까지 정해 주세요".to_string())?;
    if at < now + chrono::Duration::minutes(lead_minutes) {
        return Err(format!("마감은 지금부터 {lead_minutes}분 뒤보다 늦어야 합니다(에이전트 PC 시각 기준)"));
    }
    if at > now + chrono::Duration::days(i64::from(max_days)) {
        return Err(format!("마감은 {max_days}일 안으로 정해 주세요"));
    }
    Ok(at)
}

/// Windows가 파일·폴더 이름으로 받지 않는 이름.
const RESERVED_NAMES: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1", "LPT2",
    "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// 폴더·파일 이름 하나로 쓸 수 있는가(경로 구분자·`..`·Windows 금지 글자·예약 이름·끝 점/공백 없음).
pub fn check_name(name: &str) -> Result<(), String> {
    let shown = if name.is_empty() { "(빈 이름)" } else { name };
    if name.trim().is_empty() || name == "." || name == ".." {
        return Err(format!("‘{shown}’은(는) 이름으로 쓸 수 없습니다"));
    }
    if name.chars().count() > 120 {
        return Err("이름이 너무 깁니다(120자까지)".into());
    }
    if let Some(c) = name.chars().find(|c| r#"<>:"/\|?*"#.contains(*c) || c.is_control()) {
        return Err(format!("이름에 ‘{}’ 글자는 쓸 수 없습니다", c.escape_default()));
    }
    if name.ends_with('.') || name.ends_with(' ') || name.starts_with(' ') {
        return Err(format!("‘{name}’: 이름 앞뒤에 공백이나 끝에 점을 둘 수 없습니다"));
    }
    let stem = name.split('.').next().unwrap_or("").trim().to_ascii_uppercase();
    if RESERVED_NAMES.contains(&stem.as_str()) {
        return Err(format!("‘{name}’은(는) Windows가 쓰는 이름이라 쓸 수 없습니다"));
    }
    Ok(())
}

/// 파일 이름의 확장자(소문자, 없으면 빈 글자).
pub fn extension(name: &str) -> String {
    std::path::Path::new(name).extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> HandoffManifest {
        HandoffManifest {
            id: "0f3c".into(),
            token: "t".into(),
            job_id: "j".into(),
            host_pc: "PC01".into(),
            host_version: "1.7.4".into(),
            host_name: Some("박지훈".into()),
            title: "예산".into(),
            slug: "q4".into(),
            deadline: "2026-10-30T18:00".into(),
            output: OutputTarget { dir: vec!["2026".into(), "예산".into()], file_name: "budget_취합.xlsx".into() },
            export: ExportOptions { mark: None, styles: true, history: true, cancelled: true },
            workbook_name: "budget.xlsx".into(),
            sheets: vec!["예산신청서".into()],
            ip_names: BTreeMap::from([("10.0.0.5".into(), "이서연".into())]),
        }
    }

    #[test]
    fn package_round_trip() {
        let p = Package { manifest: manifest(), store: vec![1, 2, 3], workbook: vec![9; 1000] };
        let bytes = p.encode();
        assert_eq!(Package::decode(&bytes).unwrap(), p);
    }

    #[test]
    fn broken_packages_are_rejected() {
        let p = Package { manifest: manifest(), store: vec![1, 2, 3], workbook: vec![4] };
        let bytes = p.encode();
        assert!(Package::decode(&bytes[..bytes.len() - 1]).is_err(), "잘림");
        let mut extra = bytes.clone();
        extra.push(0);
        assert!(Package::decode(&extra).is_err(), "뒤에 더 붙음");
        assert!(Package::decode(b"NOTAGENT").is_err());
        let mut other = bytes.clone();
        other[8] = 9; // 형식
        assert!(Package::decode(&other).unwrap_err().contains("형식"));
        // 길이가 터무니없이 큰 경우(넘침 없이 거절)
        let mut huge = bytes[..16].to_vec();
        huge[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(Package::decode(&huge).is_err());
    }

    #[test]
    fn deadline_must_be_ahead_and_bounded() {
        let now = NaiveDateTime::parse_from_str("2026-10-20T10:00", "%Y-%m-%dT%H:%M").unwrap();
        assert!(check_deadline("2026-10-20T10:05", now, 1, 60).is_ok());
        assert!(check_deadline("2026-10-20T10:00", now, 1, 60).is_err(), "지금");
        assert!(check_deadline("2026-10-19T18:00", now, 1, 60).is_err(), "지남");
        assert!(check_deadline("2026-12-30T18:00", now, 1, 60).unwrap_err().contains("60일"));
        assert!(check_deadline("2026-10-20", now, 1, 60).is_err(), "시·분까지");
        assert!(check_deadline("", now, 1, 60).is_err());
    }

    #[test]
    fn names_for_windows() {
        for ok in ["2026", "예산 취합", "budget_취합_20261030.xlsx", "a.b"] {
            assert!(check_name(ok).is_ok(), "{ok}");
        }
        for bad in ["", " ", ".", "..", "a/b", r"a\b", "C:", "con", "NUL.txt", "끝.", "끝 ", "a?b", "a\u{1}"] {
            assert!(check_name(bad).is_err(), "{bad:?}");
        }
        assert_eq!(extension("Budget.XLSM"), "xlsm");
        assert_eq!(extension("통합 문서1"), "");
    }

    #[test]
    fn status_items_tolerate_missing_fields() {
        let item: StatusItem = serde_json::from_str(r#"{"id":"x","missing":true}"#).unwrap();
        assert!(item.missing && item.status.is_none() && !item.forbidden);
    }
}
