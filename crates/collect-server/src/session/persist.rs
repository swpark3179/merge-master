//! 7단계 세션 복구: 지금 상태를 기록 파일(저장소)에 남기고, 파일에서 세션을 되살린다.
//!
//! - 칸 값: 변경 기록(`changes`)에 칸마다 남는다. 시트 전체(서식·병합·값)는 블롭 `sheet`로 가끔(시트를 확정할 때, 설정·서식이
//!   바뀔 때, 스냅샷을 남길 때) 남기고, 복구할 때는 그 뒤 기록만 되짚는다.
//! - 블롭 `baseline`(저장할 때 비교하는 기준 시트), `config`(취합 설정), `comms`(공지·강조·채팅), `people`(입장한 사람의
//!   재접속 토큰 — 앱을 다시 켜도 승인받은 브라우저는 다시 승인받지 않는다)는 바뀔 때마다 통째로.
//! - 메타 `closed`(취합 마감), `server`(서비스 포트·경로·실행 여부: 다시 켜면 같은 포트로 연다).

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use collect_core::config::CollectConfig;
use collect_core::history::ChangeEntry;
use collect_core::{CellRef, SheetModel};
use collect_store::{SnapshotMeta, Store, pack_json, unpack_json};
use serde::{Deserialize, Serialize};

use super::comms::CommsData;
use super::{SessionInit, State};

pub const BLOB_SHEET: &str = "sheet";
pub const BLOB_BASELINE: &str = "baseline";
pub const BLOB_CONFIG: &str = "config";
pub const BLOB_COMMS: &str = "comms";
pub const BLOB_PEOPLE: &str = "people";
pub const META_CLOSED: &str = "closed";
pub const META_SERVER: &str = "server";
/// 호스트 앱의 업무 id(다시 확정해 새 파일이 생겨도 같은 업무면 같다). 7단계 전 파일에는 없다.
pub const META_JOB: &str = "job";
/// 호스트가 업무를 끝냈다(앱을 다시 켜도 되살리지 않는다).
pub const META_ENDED: &str = "ended";
/// 7단계 '버리고 새로 시작'(되살리지 않는다).
pub const META_DISCARDED: &str = "discarded";
/// 마감 동작 '공유 닫기'를 한 마감 시각(앱을 다시 켜거나 다시 공유해도 같은 마감으로는 다시 닫지 않는다).
pub const META_DEADLINE_FIRED: &str = "deadlineFired";

/// 서비스 상태(메타 `server`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerMeta {
    pub port: u16,
    pub slug: String,
    pub running: bool,
}

/// 메타 `sheets`: 불러온 시트 이름(JSON 배열, 순서 = 세션 안 시트 순번). 여러 시트 전 파일은 메타 `sheet` 하나.
pub const META_SHEETS: &str = "sheets";

/// 마지막으로 파일에 쓴 것(같으면 다시 쓰지 않는다).
#[derive(Default)]
pub(super) struct Persisted {
    configs: Option<Vec<CollectConfig>>,
    baselines: Option<Vec<SheetModel>>,
    comms_rev: Option<u64>,
    closed: Option<bool>,
    tokens: Option<BTreeMap<String, String>>,
}

/// 파일에서 읽어 세션에 넘기는 것(시트·설정 밖의 상태).
pub struct Restore {
    pub(super) log: Vec<ChangeEntry>,
    pub(super) snapshots: Vec<(SnapshotMeta, Vec<u8>)>,
    /// 시트마다 기준 시트.
    pub(super) baselines: Vec<SheetModel>,
    pub(super) comms: CommsData,
    pub(super) closed: bool,
    /// 호스트가 남긴 메타(서비스 상태).
    pub(super) host_meta: BTreeMap<String, String>,
    /// 입장한 사람의 재접속 토큰(이름 → 토큰).
    pub(super) tokens: BTreeMap<String, String>,
}

/// [`recover`] 결과.
pub struct Recovered {
    pub init: SessionInit,
    /// 원본 통합문서(`FullName`).
    pub workbook_id: String,
    /// 불러온 시트 이름(세션 안 순번 순서).
    pub sheets: Vec<String>,
    pub server: Option<ServerMeta>,
    pub meta: HashMap<String, String>,
    pub changes: usize,
}

/// 기록 파일 메타의 시트 이름들(여러 시트 전 파일은 `sheet` 하나).
pub fn meta_sheets(meta: &HashMap<String, String>) -> Vec<String> {
    meta.get(META_SHEETS)
        .and_then(|v| serde_json::from_str::<Vec<String>>(v).ok())
        .or_else(|| meta.get("sheet").map(|s| vec![s.clone()]))
        .unwrap_or_default()
}

pub(super) fn applied_key(user: &str, batch: &str) -> String {
    format!("{user}\u{0}{batch}")
}

impl State {
    fn last_seq(&self) -> u64 {
        self.next_seq - 1
    }

    fn tokens(&self) -> BTreeMap<String, String> {
        self.people.iter().map(|(name, p)| (name.clone(), p.token.clone())).collect()
    }

    fn configs(&self) -> Vec<CollectConfig> {
        self.sheets.iter().map(|s| s.config.clone()).collect()
    }

    fn baselines(&self) -> Vec<SheetModel> {
        self.sheets.iter().map(|s| s.baseline.clone()).collect()
    }

    /// 시트 전체(모든 시트)를 남긴다(그 뒤 기록만 되짚으면 된다).
    pub(super) fn persist_sheet(&self) {
        if let Some(store) = &self.store {
            let models: Vec<&SheetModel> = self.sheets.iter().map(|s| &s.model).collect();
            store.blob(BLOB_SHEET, self.last_seq(), pack_json(&models));
        }
    }

    /// 바뀐 것만 남긴다. `full`이면(호스트 작업 뒤) 설정·기준 시트·마감도 본다.
    pub(super) fn persist(&mut self, full: bool) {
        let Some(store) = &self.store else { return };
        if self.persisted.comms_rev != Some(self.comms.rev) {
            store.blob(BLOB_COMMS, self.last_seq(), pack_json(&self.comms.to_data()));
            self.persisted.comms_rev = Some(self.comms.rev);
        }
        // 토큰은 입장할 때만 바뀐다(사람 수만큼만 비교).
        let tokens_changed = match &self.persisted.tokens {
            None => !self.people.is_empty(),
            Some(t) => t.len() != self.people.len() || self.people.iter().any(|(n, p)| t.get(n) != Some(&p.token)),
        };
        if tokens_changed {
            let tokens = self.tokens();
            store.blob(BLOB_PEOPLE, self.last_seq(), pack_json(&tokens));
            self.persisted.tokens = Some(tokens);
        }
        if !full {
            return;
        }
        let same_configs =
            self.persisted.configs.as_ref().is_some_and(|c| {
                c.len() == self.sheets.len() && c.iter().zip(&self.sheets).all(|(a, s)| *a == s.config)
            });
        if !same_configs {
            let configs = self.configs();
            store.blob(BLOB_CONFIG, self.last_seq(), pack_json(&configs));
            self.persisted.configs = Some(configs);
        }
        let same_baselines =
            self.persisted.baselines.as_ref().is_some_and(|b| {
                b.len() == self.sheets.len() && b.iter().zip(&self.sheets).all(|(a, s)| *a == s.baseline)
            });
        if !same_baselines {
            let baselines = self.baselines();
            store.blob(BLOB_BASELINE, self.last_seq(), pack_json(&baselines));
            self.persisted.baselines = Some(baselines);
        }
        if self.persisted.closed != Some(self.closed) {
            store.meta(META_CLOSED, if self.closed { "1" } else { "0" });
            self.persisted.closed = Some(self.closed);
        }
    }

    /// 새 저장소: 처음부터 다 남긴다(서비스 중에 시트를 다시 확정하면 서비스 상태도 새 파일에).
    pub(super) fn persist_all(&mut self) {
        self.persisted = Persisted::default();
        self.persist_sheet();
        self.persist(true);
        if let Some(store) = &self.store {
            for (k, v) in &self.host_meta {
                store.meta(k, v);
            }
        }
    }

    /// 파일에서 읽은 상태를 세션에 넣는다(버전·마지막으로 바꾼 사람·반영한 묶음은 기록에서 다시 만든다).
    pub(super) fn restore(&mut self, r: Restore) {
        for (slot, baseline) in self.sheets.iter_mut().zip(r.baselines) {
            slot.baseline = baseline;
        }
        self.closed = r.closed;
        self.host_meta = r.host_meta;
        let now = tokio::time::Instant::now();
        for (name, token) in r.tokens {
            let person = super::Person { token, conn: None, last_active: now, cursor: None, ip: None };
            self.people.insert(name, person);
        }
        for e in &r.log {
            // 지운 행·열의 기록은 지금 칸이 아니다(이미 반영한 묶음으로만 센다).
            if let Some((slot, rt)) =
                self.sheets.get(e.sheet as usize).zip(self.rt.get_mut(e.sheet as usize)).filter(|_| !e.gone)
            {
                let i = (e.row * slot.model.cols.max(1) + e.col) as usize;
                if let Some(v) = rt.versions.get_mut(i) {
                    *v += 1;
                }
                rt.writer.insert(i, e.user.clone());
            }
            self.applied.insert(applied_key(&e.user, &e.batch));
        }
        self.next_seq = r.log.last().map_or(1, |e| e.seq + 1);
        self.log = r.log;
        self.snapshots = super::snapshots::Snapshots::restored(r.snapshots);
        self.comms = super::comms::Comms::from_data(r.comms);
        // 이미 파일에 있는 것은 다시 쓰지 않는다(시트는 되짚은 결과로 새로 남긴다).
        self.persisted = Persisted {
            configs: Some(self.configs()),
            baselines: Some(self.baselines()),
            comms_rev: Some(self.comms.rev),
            closed: Some(self.closed),
            tokens: Some(self.tokens()),
        };
    }
}

fn blob<T: serde::de::DeserializeOwned>(
    blobs: &HashMap<String, (u64, Vec<u8>)>,
    key: &str,
) -> Result<(u64, T), String> {
    let (seq, data) = blobs.get(key).ok_or_else(|| format!("기록 파일에 {key}이(가) 없습니다"))?;
    Ok((*seq, unpack_json(data).map_err(|e| e.to_string())?))
}

/// 시트마다 남긴 블롭(배열). 여러 시트 전 파일은 하나(객체)였다.
fn blob_many<T: serde::de::DeserializeOwned>(
    blobs: &HashMap<String, (u64, Vec<u8>)>,
    key: &str,
) -> Result<(u64, Vec<T>), String> {
    let (seq, value): (u64, serde_json::Value) = blob(blobs, key)?;
    let out = if value.is_array() {
        serde_json::from_value(value).map_err(|e| e.to_string())?
    } else {
        vec![serde_json::from_value(value).map_err(|e| e.to_string())?]
    };
    Ok((seq, out))
}

/// 기록 파일에서 세션을 되살린다(같은 파일에 이어서 기록한다).
pub fn recover(path: &Path) -> Result<Recovered, String> {
    let saved = collect_store::load(path).map_err(|e| e.to_string())?;
    let (sheet_seq, mut models): (u64, Vec<SheetModel>) = blob_many(&saved.blobs, BLOB_SHEET)?;
    let (_, configs): (u64, Vec<CollectConfig>) = blob_many(&saved.blobs, BLOB_CONFIG)?;
    let baselines: Vec<SheetModel> =
        blob_many(&saved.blobs, BLOB_BASELINE).map(|(_, b)| b).unwrap_or_else(|_| models.clone());
    if models.is_empty() || configs.len() != models.len() || baselines.len() != models.len() {
        return Err("기록 파일이 손상되었습니다(시트 수가 맞지 않습니다)".into());
    }
    let comms: CommsData = blob(&saved.blobs, BLOB_COMMS).map(|(_, c)| c).unwrap_or_default();
    let tokens: BTreeMap<String, String> = blob(&saved.blobs, BLOB_PEOPLE).map(|(_, t)| t).unwrap_or_default();
    // 시트를 남긴 뒤의 기록을 그 시트에 되짚는다.
    for e in saved.changes.iter().filter(|e| e.seq > sheet_seq && !e.gone) {
        if let Some(cell) = models.get_mut(e.sheet as usize).and_then(|m| m.cell_mut(CellRef::new(e.row, e.col))) {
            cell.v = e.new.clone();
        }
    }
    let meta = saved.meta;
    let closed = meta.get(META_CLOSED).is_some_and(|v| v == "1");
    let server = meta.get(META_SERVER).and_then(|s| serde_json::from_str(s).ok());
    let changes = saved.changes.len();
    let store = Store::open(path).map_err(|e| e.to_string())?;
    // 호스트가 남긴 메타 중 다음 기록 파일(다시 확정)로 옮겨 갈 것.
    let host_meta = [META_SERVER, META_JOB, META_DEADLINE_FIRED]
        .into_iter()
        .filter_map(|k| Some((k.to_string(), meta.get(k)?.clone())))
        .collect();
    let restore =
        Restore { log: saved.changes, snapshots: saved.snapshots, baselines, comms, closed, host_meta, tokens };
    let mut sheets = meta_sheets(&meta);
    if sheets.len() != models.len() {
        sheets = models.iter().map(|m| m.sheet.clone()).collect();
    }
    Ok(Recovered {
        init: SessionInit {
            sheets: models.into_iter().zip(configs).collect(),
            store: Some(store),
            restore: Some(restore),
        },
        workbook_id: meta.get("workbook").cloned().unwrap_or_default(),
        sheets,
        server,
        meta,
        changes,
    })
}
