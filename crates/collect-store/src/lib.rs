//! 취합 세션 저장소: 셀 변경 기록과 스냅샷을 SQLite 파일에 남긴다(7단계 세션 복구의 바탕).
//!
//! - 쓰기는 전용 스레드가 순서대로 한다(세션 액터는 보내기만 해서 멈추지 않는다). 쓰기에 실패해도 취합은 계속하고 로그만 남긴다.
//! - 스냅샷은 셀 값 배열(행 우선)을 JSON으로 만들어 zstd로 압축한다(1,000행 × 9열 ≈ 수 KB).
//! - 블롭(`blobs`): 시트·기준 시트·설정·소통 상태처럼 통째로 바꿔 쓰는 것. 시트 블롭에는 그때의 마지막 기록 번호를 함께
//!   남겨, 복구할 때 그 뒤 기록만 되짚는다(칸마다 시트 전체를 다시 쓰지 않는다).
//! - 읽기([`load`])는 세션 복구(7단계)에서, [`peek`]은 '이어서 하기' 안내에서 쓴다.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread::JoinHandle;

use collect_core::CellValue;
use collect_core::history::ChangeEntry;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("저장소 오류: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("저장소 파일 오류: {0}")]
    Io(#[from] std::io::Error),
    #[error("스냅샷을 풀 수 없습니다: {0}")]
    Decode(String),
    #[error("저장소 스레드가 끝났습니다")]
    Gone,
}

pub type Result<T> = std::result::Result<T, StoreError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum SnapshotKind {
    /// 불러온 시점의 원본.
    Original,
    /// 15분마다 자동(그사이 바뀐 것이 있을 때만).
    Auto,
    /// 호스트가 '지금 저장'.
    Manual,
    /// 스냅샷으로 복원하기 직전(복원도 되돌릴 수 있게).
    BeforeRestore,
}

impl SnapshotKind {
    fn as_str(self) -> &'static str {
        match self {
            SnapshotKind::Original => "original",
            SnapshotKind::Auto => "auto",
            SnapshotKind::Manual => "manual",
            SnapshotKind::BeforeRestore => "beforeRestore",
        }
    }

    fn parse(s: &str) -> Self {
        match s {
            "original" => SnapshotKind::Original,
            "manual" => SnapshotKind::Manual,
            "beforeRestore" => SnapshotKind::BeforeRestore,
            _ => SnapshotKind::Auto,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SnapshotMeta {
    pub id: u32,
    #[ts(type = "number")]
    pub at_ms: i64,
    pub kind: SnapshotKind,
    /// 이 스냅샷에 든 마지막 변경 기록 번호(그 뒤 기록 = 이후 변경).
    #[ts(type = "number")]
    pub seq: u64,
    pub label: String,
}

/// 셀 값 배열 → 압축 바이트.
pub fn pack(values: &[CellValue]) -> Vec<u8> {
    pack_json(values)
}

pub fn unpack(bytes: &[u8]) -> Result<Vec<CellValue>> {
    unpack_json(bytes)
}

/// 아무 값 → JSON → zstd.
pub fn pack_json<T: Serialize + ?Sized>(value: &T) -> Vec<u8> {
    let json = serde_json::to_vec(value).expect("직렬화");
    zstd::encode_all(json.as_slice(), 3).expect("zstd 압축")
}

pub fn unpack_json<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    let json = zstd::decode_all(bytes).map_err(|e| StoreError::Decode(e.to_string()))?;
    serde_json::from_slice(&json).map_err(|e| StoreError::Decode(e.to_string()))
}

enum Op {
    Changes(Vec<ChangeEntry>),
    Snapshot(SnapshotMeta, Vec<u8>),
    Meta(String, String),
    Blob(String, u64, Vec<u8>),
    /// 다른 시트로 바꿈: 기록·스냅샷을 비운다.
    Reset,
    Flush(mpsc::SyncSender<()>),
}

/// 쓰기 전용 손잡이. 버리면 남은 쓰기를 마치고 닫는다.
pub struct Store {
    path: PathBuf,
    tx: Option<mpsc::Sender<Op>>,
    join: Option<JoinHandle<()>>,
}

const SCHEMA: &str = "
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
CREATE TABLE IF NOT EXISTS changes (
    seq INTEGER PRIMARY KEY,
    at_ms INTEGER NOT NULL,
    user TEXT NOT NULL,
    batch TEXT NOT NULL,
    origin TEXT NOT NULL,
    kind TEXT NOT NULL,
    row INTEGER NOT NULL,
    col INTEGER NOT NULL,
    old TEXT NOT NULL,
    new TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS snapshots (
    id INTEGER PRIMARY KEY,
    at_ms INTEGER NOT NULL,
    kind TEXT NOT NULL,
    seq INTEGER NOT NULL,
    label TEXT NOT NULL,
    data BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS blobs (key TEXT PRIMARY KEY, seq INTEGER NOT NULL, data BLOB NOT NULL);
";

fn json<T: Serialize>(v: &T) -> String {
    serde_json::to_string(v).expect("직렬화")
}

fn write(conn: &mut Connection, op: Op) -> rusqlite::Result<()> {
    match op {
        Op::Changes(entries) => {
            let tx = conn.transaction()?;
            {
                let mut stmt = tx.prepare_cached(
                    "INSERT OR REPLACE INTO changes (seq, at_ms, user, batch, origin, kind, row, col, old, new)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                )?;
                for e in &entries {
                    stmt.execute(params![
                        e.seq as i64,
                        e.at_ms,
                        e.user,
                        e.batch,
                        json(&e.origin).trim_matches('"'),
                        json(&e.kind).trim_matches('"'),
                        e.row,
                        e.col,
                        json(&e.old),
                        json(&e.new),
                    ])?;
                }
            }
            tx.commit()
        }
        Op::Snapshot(m, data) => conn
            .execute(
                "INSERT OR REPLACE INTO snapshots (id, at_ms, kind, seq, label, data) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![m.id, m.at_ms, m.kind.as_str(), m.seq as i64, m.label, data],
            )
            .map(drop),
        Op::Meta(k, v) => {
            conn.execute("INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)", params![k, v]).map(drop)
        }
        Op::Blob(k, seq, data) => conn
            .execute("INSERT OR REPLACE INTO blobs (key, seq, data) VALUES (?1, ?2, ?3)", params![k, seq as i64, data])
            .map(drop),
        Op::Reset => conn.execute_batch("DELETE FROM changes; DELETE FROM snapshots; DELETE FROM blobs;"),
        Op::Flush(done) => {
            let _ = done.send(());
            Ok(())
        }
    }
}

impl Store {
    /// 파일을 만들거나 연다(폴더도 만든다).
    pub fn open(path: impl AsRef<Path>) -> Result<Store> {
        let path = path.as_ref().to_path_buf();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut conn = Connection::open(&path)?;
        conn.execute_batch(SCHEMA)?;
        let (tx, rx) = mpsc::channel::<Op>();
        let shown = path.display().to_string();
        let join = std::thread::Builder::new().name("collect-store".into()).spawn(move || {
            while let Ok(op) = rx.recv() {
                if let Err(e) = write(&mut conn, op) {
                    tracing::error!("저장소({shown}) 쓰기 실패: {e}");
                }
            }
        })?;
        Ok(Store { path, tx: Some(tx), join: Some(join) })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn send(&self, op: Op) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(op);
        }
    }

    pub fn append(&self, entries: &[ChangeEntry]) {
        if !entries.is_empty() {
            self.send(Op::Changes(entries.to_vec()));
        }
    }

    pub fn snapshot(&self, meta: &SnapshotMeta, data: &[u8]) {
        self.send(Op::Snapshot(meta.clone(), data.to_vec()));
    }

    pub fn meta(&self, key: &str, value: &str) {
        self.send(Op::Meta(key.into(), value.into()));
    }

    /// 통째로 바꿔 쓰는 값(압축한 바이트). `seq`는 그때의 마지막 기록 번호.
    pub fn blob(&self, key: &str, seq: u64, data: Vec<u8>) {
        self.send(Op::Blob(key.into(), seq, data));
    }

    pub fn reset(&self) {
        self.send(Op::Reset);
    }

    /// 그때까지 보낸 쓰기가 끝날 때까지 기다린다.
    pub fn flush(&self) -> Result<()> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.tx.as_ref().ok_or(StoreError::Gone)?.send(Op::Flush(tx)).map_err(|_| StoreError::Gone)?;
        rx.recv().map_err(|_| StoreError::Gone)
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        drop(self.tx.take());
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Store({})", self.path.display())
    }
}

/// 파일에 남은 것.
#[derive(Debug, Default)]
pub struct Saved {
    pub changes: Vec<ChangeEntry>,
    pub snapshots: Vec<(SnapshotMeta, Vec<u8>)>,
    pub meta: HashMap<String, String>,
    /// 키 → (기록 번호, 압축한 바이트).
    pub blobs: HashMap<String, (u64, Vec<u8>)>,
}

fn from_json<T: serde::de::DeserializeOwned>(s: &str) -> rusqlite::Result<T> {
    serde_json::from_str(s)
        .map_err(|e| rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e)))
}

pub fn load(path: impl AsRef<Path>) -> Result<Saved> {
    let conn = Connection::open(path)?;
    conn.execute_batch(SCHEMA)?;
    let mut out = Saved::default();
    let mut stmt =
        conn.prepare("SELECT seq, at_ms, user, batch, origin, kind, row, col, old, new FROM changes ORDER BY seq")?;
    let rows = stmt.query_map([], |r| {
        Ok(ChangeEntry {
            seq: r.get::<_, i64>(0)? as u64,
            at_ms: r.get(1)?,
            user: r.get(2)?,
            batch: r.get(3)?,
            origin: from_json(&format!("\"{}\"", r.get::<_, String>(4)?))?,
            kind: from_json(&format!("\"{}\"", r.get::<_, String>(5)?))?,
            row: r.get(6)?,
            col: r.get(7)?,
            old: from_json(&r.get::<_, String>(8)?)?,
            new: from_json(&r.get::<_, String>(9)?)?,
        })
    })?;
    for row in rows {
        out.changes.push(row?);
    }
    let mut stmt = conn.prepare("SELECT id, at_ms, kind, seq, label, data FROM snapshots ORDER BY id")?;
    let rows = stmt.query_map([], |r| {
        Ok((
            SnapshotMeta {
                id: r.get(0)?,
                at_ms: r.get(1)?,
                kind: SnapshotKind::parse(&r.get::<_, String>(2)?),
                seq: r.get::<_, i64>(3)? as u64,
                label: r.get(4)?,
            },
            r.get::<_, Vec<u8>>(5)?,
        ))
    })?;
    for row in rows {
        out.snapshots.push(row?);
    }
    out.meta = read_meta(&conn)?;
    let mut stmt = conn.prepare("SELECT key, seq, data FROM blobs")?;
    let rows =
        stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u64, r.get::<_, Vec<u8>>(2)?)))?;
    for row in rows {
        let (k, seq, data) = row?;
        out.blobs.insert(k, (seq, data));
    }
    Ok(out)
}

fn read_meta(conn: &Connection) -> rusqlite::Result<HashMap<String, String>> {
    let mut stmt = conn.prepare("SELECT key, value FROM meta")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    rows.collect()
}

/// '이어서 하기' 안내용 요약(기록 전체를 읽지 않는다).
#[derive(Debug, Clone, Default)]
pub struct Peek {
    pub meta: HashMap<String, String>,
    pub changes: u64,
    pub last_change_ms: Option<i64>,
    /// 시트 블롭이 있다(이어서 할 수 있다).
    pub resumable: bool,
}

pub fn peek(path: impl AsRef<Path>) -> Result<Peek> {
    let conn = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let has = |table: &str| -> rusqlite::Result<bool> {
        conn.query_row("SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = ?1", [table], |r| {
            r.get::<_, i64>(0)
        })
        .map(|n| n > 0)
    };
    if !has("meta")? || !has("changes")? || !has("blobs")? {
        return Ok(Peek::default());
    }
    let (changes, last) = conn.query_row("SELECT count(*), max(at_ms) FROM changes", [], |r| {
        Ok((r.get::<_, i64>(0)? as u64, r.get::<_, Option<i64>>(1)?))
    })?;
    let resumable = conn.query_row("SELECT count(*) FROM blobs WHERE key = 'sheet'", [], |r| r.get::<_, i64>(0))? > 0;
    Ok(Peek { meta: read_meta(&conn)?, changes, last_change_ms: last, resumable })
}

#[cfg(test)]
mod tests {
    use collect_core::history::{BatchOrigin, ChangeKind};

    use super::*;

    fn entry(seq: u64, new: CellValue) -> ChangeEntry {
        ChangeEntry {
            seq,
            at_ms: 1000 * seq as i64,
            user: "이서연".into(),
            batch: format!("b{seq}"),
            origin: BatchOrigin::Paste,
            kind: ChangeKind::Input,
            row: 2,
            col: 5,
            old: CellValue::Empty,
            new,
        }
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("collect-store-test-{}", std::process::id()));
        let path = dir.join(name);
        let _ = std::fs::remove_file(&path);
        path
    }

    #[test]
    fn pack_round_trip_is_small() {
        let values: Vec<CellValue> = (0..9000)
            .map(|i| if i % 3 == 0 { CellValue::Number(f64::from(i)) } else { CellValue::Text("교육훈련비".into()) })
            .collect();
        let packed = pack(&values);
        assert!(packed.len() < 20_000, "{} bytes", packed.len());
        assert_eq!(unpack(&packed).unwrap(), values);
        assert!(unpack(b"not zstd").is_err());
    }

    #[test]
    fn writes_are_persisted_and_reset() {
        let path = temp("a.sqlite");
        {
            let store = Store::open(&path).unwrap();
            store.append(&[entry(1, CellValue::Number(9800.0)), entry(2, CellValue::Text("간담회".into()))]);
            let meta = SnapshotMeta { id: 1, at_ms: 5, kind: SnapshotKind::Original, seq: 0, label: "원본".into() };
            store.snapshot(&meta, &pack(&[CellValue::Empty, CellValue::Number(1.0)]));
            store.meta("workbook", "budget.xlsx");
            store.flush().unwrap();
        } // drop: 남은 쓰기를 마치고 닫는다
        let saved = load(&path).unwrap();
        assert_eq!(saved.changes.len(), 2);
        assert_eq!(saved.changes[0], entry(1, CellValue::Number(9800.0)));
        assert_eq!(saved.changes[1].origin, BatchOrigin::Paste);
        assert_eq!(saved.snapshots[0].0.kind, SnapshotKind::Original);
        assert_eq!(unpack(&saved.snapshots[0].1).unwrap()[1], CellValue::Number(1.0));
        assert_eq!(saved.meta["workbook"], "budget.xlsx");

        let peeked = peek(&path).unwrap();
        assert_eq!((peeked.changes, peeked.last_change_ms, peeked.resumable), (2, Some(2000), false));
        let store = Store::open(&path).unwrap();
        store.blob("sheet", 2, pack_json(&vec![1, 2, 3]));
        store.flush().unwrap();
        let saved = load(&path).unwrap();
        let (seq, data) = &saved.blobs["sheet"];
        assert_eq!((*seq, unpack_json::<Vec<i32>>(data).unwrap()), (2, vec![1, 2, 3]));
        assert!(peek(&path).unwrap().resumable);
        store.reset();
        store.append(&[entry(1, CellValue::Number(1.0))]);
        store.flush().unwrap();
        drop(store);
        let saved = load(&path).unwrap();
        assert_eq!((saved.changes.len(), saved.snapshots.len()), (1, 0));
        let _ = std::fs::remove_file(&path);
    }
}
