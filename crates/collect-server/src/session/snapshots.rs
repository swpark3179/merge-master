//! 6단계 스냅샷(1h): 불러온 원본, 15분마다 자동(그사이 바뀐 것이 있을 때만), 수동, 복원 직전.
//!
//! 스냅샷은 셀 값 배열(행 우선)을 압축해 메모리에 두고, 저장소가 있으면 파일에도 남긴다.

use std::time::Duration;

use collect_core::CellValue;
use collect_store::{SnapshotKind, SnapshotMeta, pack, unpack};
use tokio::time::Instant;

use super::State;
use super::edit::now_ms;

/// 자동 스냅샷 간격.
pub const AUTO_SNAPSHOT: Duration = Duration::from_secs(15 * 60);

pub(super) struct Snapshots {
    pub(super) list: Vec<(SnapshotMeta, Vec<u8>)>,
    next_id: u32,
    last_auto: Instant,
}

impl Default for Snapshots {
    fn default() -> Self {
        Self { list: Vec::new(), next_id: 1, last_auto: Instant::now() }
    }
}

impl Snapshots {
    /// 파일에서 읽은 스냅샷으로 다시 시작한다(자동 스냅샷 간격은 지금부터).
    pub(super) fn restored(list: Vec<(SnapshotMeta, Vec<u8>)>) -> Self {
        let next_id = list.iter().map(|(m, _)| m.id).max().unwrap_or(0) + 1;
        Self { list, next_id, last_auto: Instant::now() }
    }
}

pub(super) fn values_of(list: &[(SnapshotMeta, Vec<u8>)], id: u32) -> Option<Vec<CellValue>> {
    list.iter().find(|(m, _)| m.id == id).and_then(|(_, data)| unpack(data).ok())
}

fn default_label(kind: SnapshotKind) -> &'static str {
    match kind {
        SnapshotKind::Original => "불러온 원본",
        SnapshotKind::Auto => "자동 (15분)",
        SnapshotKind::Manual => "수동",
        SnapshotKind::BeforeRestore => "복원 직전",
    }
}

impl State {
    /// 지금 시트 값으로 스냅샷을 남긴다.
    pub(super) fn take_snapshot(&mut self, kind: SnapshotKind, label: &str) -> u32 {
        let values: Vec<CellValue> = self.model.cells.iter().map(|c| c.v.clone()).collect();
        let data = pack(&values);
        let id = self.snapshots.next_id;
        self.snapshots.next_id += 1;
        let label = if label.trim().is_empty() { default_label(kind).to_string() } else { label.trim().to_string() };
        let meta = SnapshotMeta { id, at_ms: now_ms(), kind, seq: self.next_seq - 1, label };
        if let Some(store) = &self.store {
            store.snapshot(&meta, &data);
        }
        self.snapshots.list.push((meta, data));
        if kind == SnapshotKind::Auto {
            self.snapshots.last_auto = Instant::now();
        }
        // 복구할 때 되짚을 기록을 줄이려고 시트도 함께 남긴다.
        self.persist_sheet();
        id
    }

    /// 주기 점검: 15분이 지났고 마지막 스냅샷 뒤에 바뀐 것이 있으면 자동 스냅샷.
    pub(super) fn auto_snapshot(&mut self) {
        if self.snapshots.last_auto.elapsed() < AUTO_SNAPSHOT {
            return;
        }
        let last_seq = self.snapshots.list.last().map_or(0, |(m, _)| m.seq);
        if self.next_seq - 1 > last_seq {
            self.take_snapshot(SnapshotKind::Auto, "");
        } else {
            self.snapshots.last_auto = Instant::now();
        }
    }

    /// 새 시트: 스냅샷을 비우고 원본을 남긴다.
    pub(super) fn reset_snapshots(&mut self) {
        self.snapshots = Snapshots::default();
        self.take_snapshot(SnapshotKind::Original, "");
    }
}
