//! 7단계 세션 복구: 지금 상태를 기록 파일(저장소)에 남기고, 파일에서 세션을 되살린다.
//!
//! - 칸 값: 변경 기록(`changes`)에 칸마다 남는다. 시트 전체(서식·병합·값)는 블롭 `sheet`로 가끔(시트를 확정할 때, 설정·서식이
//!   바뀔 때, 스냅샷을 남길 때) 남기고, 복구할 때는 그 뒤 기록만 되짚는다.
//! - 블롭 `baseline`(저장할 때 비교하는 기준 시트), `config`(취합 설정), `comms`(공지·강조·채팅)는 바뀔 때마다 통째로.
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
pub const META_CLOSED: &str = "closed";
pub const META_SERVER: &str = "server";

/// 서비스 상태(메타 `server`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerMeta {
    pub port: u16,
    pub slug: String,
    pub running: bool,
}

/// 마지막으로 파일에 쓴 것(같으면 다시 쓰지 않는다).
#[derive(Default)]
pub(super) struct Persisted {
    config: Option<CollectConfig>,
    baseline: Option<SheetModel>,
    comms_rev: Option<u64>,
    closed: Option<bool>,
}

/// 파일에서 읽어 세션에 넘기는 것(시트·설정 밖의 상태).
pub struct Restore {
    pub(super) log: Vec<ChangeEntry>,
    pub(super) snapshots: Vec<(SnapshotMeta, Vec<u8>)>,
    pub(super) baseline: SheetModel,
    pub(super) comms: CommsData,
    pub(super) closed: bool,
    /// 호스트가 남긴 메타(서비스 상태).
    pub(super) host_meta: BTreeMap<String, String>,
}

/// [`recover`] 결과.
pub struct Recovered {
    pub init: SessionInit,
    /// 원본 통합문서(`FullName`)와 시트.
    pub workbook_id: String,
    pub sheet: String,
    pub server: Option<ServerMeta>,
    pub meta: HashMap<String, String>,
    pub changes: usize,
}

pub(super) fn applied_key(user: &str, batch: &str) -> String {
    format!("{user}\u{0}{batch}")
}

impl State {
    fn last_seq(&self) -> u64 {
        self.next_seq - 1
    }

    /// 시트 전체를 남긴다(그 뒤 기록만 되짚으면 된다).
    pub(super) fn persist_sheet(&self) {
        if let Some(store) = &self.store {
            store.blob(BLOB_SHEET, self.last_seq(), pack_json(&self.model));
        }
    }

    /// 바뀐 것만 남긴다. `full`이면(호스트 작업 뒤) 설정·기준 시트·마감도 본다.
    pub(super) fn persist(&mut self, full: bool) {
        let Some(store) = &self.store else { return };
        if self.persisted.comms_rev != Some(self.comms.rev) {
            store.blob(BLOB_COMMS, self.last_seq(), pack_json(&self.comms.to_data()));
            self.persisted.comms_rev = Some(self.comms.rev);
        }
        if !full {
            return;
        }
        if self.persisted.config.as_ref() != Some(&self.config) {
            store.blob(BLOB_CONFIG, self.last_seq(), pack_json(&self.config));
            self.persisted.config = Some(self.config.clone());
        }
        if self.persisted.baseline.as_ref() != Some(&self.baseline) {
            store.blob(BLOB_BASELINE, self.last_seq(), pack_json(&self.baseline));
            self.persisted.baseline = Some(self.baseline.clone());
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
        let cols = self.model.cols.max(1);
        self.baseline = r.baseline;
        self.closed = r.closed;
        self.host_meta = r.host_meta;
        for e in &r.log {
            let i = (e.row * cols + e.col) as usize;
            if let Some(v) = self.versions.get_mut(i) {
                *v += 1;
            }
            self.writer.insert(i, e.user.clone());
            self.applied.insert(applied_key(&e.user, &e.batch));
        }
        self.next_seq = r.log.last().map_or(1, |e| e.seq + 1);
        self.log = r.log;
        self.snapshots = super::snapshots::Snapshots::restored(r.snapshots);
        self.comms = super::comms::Comms::from_data(r.comms);
        // 이미 파일에 있는 것은 다시 쓰지 않는다(시트는 되짚은 결과로 새로 남긴다).
        self.persisted = Persisted {
            config: Some(self.config.clone()),
            baseline: Some(self.baseline.clone()),
            comms_rev: Some(self.comms.rev),
            closed: Some(self.closed),
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

/// 기록 파일에서 세션을 되살린다(같은 파일에 이어서 기록한다).
pub fn recover(path: &Path) -> Result<Recovered, String> {
    let saved = collect_store::load(path).map_err(|e| e.to_string())?;
    let (sheet_seq, mut model): (u64, SheetModel) = blob(&saved.blobs, BLOB_SHEET)?;
    let (_, config): (u64, CollectConfig) = blob(&saved.blobs, BLOB_CONFIG)?;
    let baseline: SheetModel = blob(&saved.blobs, BLOB_BASELINE).map(|(_, b)| b).unwrap_or_else(|_| model.clone());
    let comms: CommsData = blob(&saved.blobs, BLOB_COMMS).map(|(_, c)| c).unwrap_or_default();
    // 시트를 남긴 뒤의 기록을 되짚는다.
    for e in saved.changes.iter().filter(|e| e.seq > sheet_seq) {
        if let Some(cell) = model.cell_mut(CellRef::new(e.row, e.col)) {
            cell.v = e.new.clone();
        }
    }
    let meta = saved.meta;
    let closed = meta.get(META_CLOSED).is_some_and(|v| v == "1");
    let server = meta.get(META_SERVER).and_then(|s| serde_json::from_str(s).ok());
    let changes = saved.changes.len();
    let store = Store::open(path).map_err(|e| e.to_string())?;
    let host_meta = meta.get(META_SERVER).map(|v| (META_SERVER.to_string(), v.clone())).into_iter().collect();
    let restore = Restore { log: saved.changes, snapshots: saved.snapshots, baseline, comms, closed, host_meta };
    Ok(Recovered {
        init: SessionInit { model, config, store: Some(store), restore: Some(restore) },
        workbook_id: meta.get("workbook").cloned().unwrap_or_default(),
        sheet: meta.get("sheet").cloned().unwrap_or_default(),
        server,
        meta,
        changes,
    })
}
