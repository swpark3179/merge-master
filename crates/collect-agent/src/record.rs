//! 서비스 하나의 기록(`services\<id>\record.json`): 넘겨받은 내용과 상태. 에이전트를 다시 켜면 이것으로 되살린다.

use std::path::{Path, PathBuf};

use agent_protocol::{HandoffManifest, ServiceState};
use serde::{Deserialize, Serialize};

use crate::settings::write_atomic;

pub const RECORD_FILE: &str = "record.json";
pub const STORE_FILE: &str = "session.sqlite";

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
