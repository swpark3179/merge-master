//! 서비스 하나의 기록(`services\<id>\record.json`): 넘겨받은 내용과 상태. 에이전트를 다시 켜면 이것으로 되살린다.

use std::path::{Path, PathBuf};

use agent_protocol::{HandoffManifest, ServiceState};
use serde::{Deserialize, Serialize};

use crate::settings::write_atomic;

pub const RECORD_FILE: &str = "record.json";
pub const STORE_FILE: &str = "session.sqlite";
/// 마감 때 남기는 취합 완료 데이터([`agent_protocol::save_data::SaveData`]). 엑셀로 저장하면(이 PC 또는 관리자 PC) 지운다.
pub const RESULT_FILE: &str = "result.mmsave";

/// 끝났을 때의 진행률(세션을 놓은 뒤에도 상태에 보인다).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub roster: u32,
    pub required: u32,
    pub filled: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    pub manifest: HandoffManifest,
    /// 에이전트가 정한 주소 경로.
    pub slug: String,
    pub state: ServiceState,
    /// 마감 때 저장할 파일(받을 때 정한 전체 경로 — 기본 저장 폴더를 바꿔도 그대로).
    pub output_path: PathBuf,
    /// 서비스 폴더 안의 원본 파일 이름(`source.xlsx`).
    pub source_file: String,
    pub created_at_ms: i64,
    /// 저장을 시도한 횟수(모두).
    #[serde(default)]
    pub attempts: u32,
    /// 이번 저장에서 남은 다시 시도 횟수.
    #[serde(default)]
    pub retries_left: u32,
    /// 다음 저장 시도 시각(`Saving`일 때).
    #[serde(default)]
    pub next_try_ms: Option<i64>,
    /// 호스트(또는 트레이)가 끝냈다(아니면 마감). 참여자에게 보일 까닭이 다르다.
    #[serde(default)]
    pub ended_by_host: bool,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub saved_path: Option<String>,
    #[serde(default)]
    pub saved_at_ms: Option<i64>,
    #[serde(default)]
    pub finished_at_ms: Option<i64>,
    #[serde(default)]
    pub last: Option<Progress>,
    /// 취합 완료 데이터([`RESULT_FILE`])를 남겼다(엑셀로 저장하면 지운다).
    #[serde(default)]
    pub result_file: bool,
    /// 엑셀 저장을 시작한 시각(끝나면 지운다 — 다시 켰을 때 남아 있으면 저장하던 도중 꺼진 것).
    #[serde(default)]
    pub excel_started_ms: Option<i64>,
    /// 관리자 PC(넘긴 호스트 앱)에서 저장했다(`saved_path`는 그 PC의 경로).
    #[serde(default)]
    pub saved_on_host: bool,
}

impl Record {
    pub fn load(dir: &Path) -> Result<Record, String> {
        let bytes = std::fs::read(dir.join(RECORD_FILE)).map_err(|e| format!("서비스 기록을 읽을 수 없습니다: {e}"))?;
        serde_json::from_slice(&bytes).map_err(|e| format!("서비스 기록이 손상되었습니다: {e}"))
    }

    pub fn save(&self, dir: &Path) -> Result<(), String> {
        write_atomic(&dir.join(RECORD_FILE), &serde_json::to_vec_pretty(self).expect("직렬화"))
            .map_err(|e| format!("서비스 기록을 남길 수 없습니다: {e}"))
    }
}
