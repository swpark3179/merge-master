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
use collect_core::format::Axis;
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
    #[error("기록 사본을 만들 수 없습니다: {0}")]
    Backup(String),
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

/// 여러 시트의 셀 값(시트마다 행 우선) → 압축 바이트. `{"sheets": [[…], …]}` 모양이라 옛 한 시트 배열과 헷갈리지 않는다.
pub fn pack_sheets(values: &[Vec<CellValue>]) -> Vec<u8> {
    #[derive(Serialize)]
    struct Sheets<'a> {
        sheets: &'a [Vec<CellValue>],
    }
    pack_json(&Sheets { sheets: values })
}

/// [`pack_sheets`]의 반대. 여러 시트 전의 스냅샷(한 시트 값 배열)은 시트 하나로 읽는다.
pub fn unpack_sheets(bytes: &[u8]) -> Result<Vec<Vec<CellValue>>> {
    #[derive(Deserialize)]
    struct Sheets {
        sheets: Vec<Vec<CellValue>>,
    }
    let decode = |e: serde_json::Error| StoreError::Decode(e.to_string());
    let value: serde_json::Value = unpack_json(bytes)?;
    if value.is_object() {
        Ok(serde_json::from_value::<Sheets>(value).map_err(decode)?.sheets)
    } else {
        Ok(vec![serde_json::from_value(value).map_err(decode)?])
    }
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
    /// 앱에서 붙인 행·열을 지움: 그 줄의 기록은 `gone`, 뒤의 기록은 당긴다.
    Remap {
        sheet: u32,
        axis: Axis,
        from: u32,
        count: u32,
    },
    /// 스냅샷 값을 바꿔 쓴다(표 구조가 바뀜).
    ReplaceSnapshot(u32, Vec<u8>),
    /// 다른 시트로 바꿈: 기록·스냅샷을 비운다.
    Reset,
    /// 한 번에(전부 또는 하나도) 반영할 쓰기들([`Store::begin_group`]).
    Group(Vec<Op>),
    Flush(mpsc::SyncSender<()>),
    /// 그때까지의 쓰기를 모두 반영한 사본을 한 파일로(`VACUUM INTO`).
    Backup(PathBuf, mpsc::SyncSender<Result<()>>),
}

/// 쓰기 전용 손잡이. 버리면 남은 쓰기를 마치고 닫는다.
pub struct Store {
    path: PathBuf,
    tx: Option<mpsc::Sender<Op>>,
    join: Option<JoinHandle<()>>,
    /// 묶는 중인 쓰기([`Store::begin_group`] ~ [`Store::end_group`]).
    group: std::sync::Mutex<Option<Vec<Op>>>,
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
    sheet INTEGER NOT NULL DEFAULT 0,
    row INTEGER NOT NULL,
    col INTEGER NOT NULL,
    old TEXT NOT NULL,
    new TEXT NOT NULL,
    gone INTEGER NOT NULL DEFAULT 0
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

/// 옛 파일에 없는 변경 기록 열을 더한다: 여러 시트를 함께 취합하기 전 파일은 `sheet`(옛 기록은 모두 첫 시트), 행·열
/// 지우기 전 파일은 `gone`(모두 살아 있는 기록).
fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    let has = |name: &str| -> rusqlite::Result<bool> {
        conn.query_row("SELECT count(*) FROM pragma_table_info('changes') WHERE name = ?1", [name], |r| {
            r.get::<_, i64>(0)
        })
        .map(|n| n > 0)
    };
    if !has("sheet")? {
        conn.execute_batch("ALTER TABLE changes ADD COLUMN sheet INTEGER NOT NULL DEFAULT 0;")?;
    }
    if !has("gone")? {
        conn.execute_batch("ALTER TABLE changes ADD COLUMN gone INTEGER NOT NULL DEFAULT 0;")?;
    }
    Ok(())
}

fn json<T: Serialize>(v: &T) -> String {
    serde_json::to_string(v).expect("직렬화")
}

/// `f`를 한 번에(전부 또는 하나도) 반영한다. 묶음 안에서 불러도 된다(SAVEPOINT는 겹쳐 쓸 수 있다).
fn atomic(conn: &Connection, f: impl FnOnce(&Connection) -> rusqlite::Result<()>) -> rusqlite::Result<()> {
    conn.execute_batch("SAVEPOINT op")?;
    match f(conn) {
        Ok(()) => conn.execute_batch("RELEASE op"),
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK TO op; RELEASE op");
            Err(e)
        }
    }
}

fn write(conn: &Connection, op: Op) -> rusqlite::Result<()> {
    match op {
        Op::Changes(entries) => atomic(conn, |tx| {
            {
                let mut stmt = tx.prepare_cached(
                    "INSERT OR REPLACE INTO changes (seq, at_ms, user, batch, origin, kind, row, col, old, new, sheet, gone)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
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
                        e.sheet,
                        e.gone,
                    ])?;
                }
            }
            Ok(())
        }),
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
        Op::Remap { sheet, axis, from, count } => {
            let line = match axis {
                Axis::Row => "row",
                Axis::Col => "col",
            };
            let end = from.saturating_add(count);
            atomic(conn, |tx| {
                tx.execute(
                    &format!(
                        "UPDATE changes SET gone = 1 WHERE sheet = ?1 AND gone = 0 AND {line} >= ?2 AND {line} < ?3"
                    ),
                    params![sheet, from, end],
                )?;
                tx.execute(
                    &format!("UPDATE changes SET {line} = {line} - ?3 WHERE sheet = ?1 AND gone = 0 AND {line} >= ?2"),
                    params![sheet, end, count],
                )?;
                Ok(())
            })
        }
        Op::ReplaceSnapshot(id, data) => {
            conn.execute("UPDATE snapshots SET data = ?2 WHERE id = ?1", params![id, data]).map(drop)
        }
        Op::Reset => conn.execute_batch("DELETE FROM changes; DELETE FROM snapshots; DELETE FROM blobs;"),
        Op::Group(ops) => atomic(conn, |c| ops.into_iter().try_for_each(|op| write(c, op))),
        Op::Flush(done) => {
            let _ = done.send(());
            Ok(())
        }
        Op::Backup(to, done) => {
            let _ = done.send(backup(conn, &to));
            Ok(())
        }
    }
}

/// 이 연결(쓰기 스레드)로 사본을 만든다: 큐에서 앞선 쓰기는 이미 반영됐고, WAL까지 합친 한 파일이 된다.
fn backup(conn: &Connection, to: &Path) -> Result<()> {
    if to.exists() {
        return Err(StoreError::Backup(format!("{}이(가) 이미 있습니다", to.display())));
    }
    if let Some(dir) = to.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let target = to.to_str().ok_or_else(|| StoreError::Backup("경로에 쓸 수 없는 글자가 있습니다".into()))?;
    conn.execute("VACUUM INTO ?1", [target])?;
    Ok(())
}

impl Store {
    /// 파일을 만들거나 연다(폴더도 만든다).
    pub fn open(path: impl AsRef<Path>) -> Result<Store> {
        let path = path.as_ref().to_path_buf();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let conn = Connection::open(&path)?;
        conn.execute_batch(SCHEMA)?;
        migrate(&conn)?;
        let (tx, rx) = mpsc::channel::<Op>();
        let shown = path.display().to_string();
        let join = std::thread::Builder::new().name("collect-store".into()).spawn(move || {
            while let Ok(op) = rx.recv() {
                if let Err(e) = write(&conn, op) {
                    tracing::error!("저장소({shown}) 쓰기 실패: {e}");
                }
            }
        })?;
        Ok(Store { path, tx: Some(tx), join: Some(join), group: Default::default() })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn send(&self, op: Op) {
        if let Some(group) = self.group.lock().unwrap().as_mut() {
            group.push(op);
            return;
        }
        if let Some(tx) = &self.tx {
            let _ = tx.send(op);
        }
    }

    /// 이제부터 [`Store::end_group`]까지의 쓰기를 모아 한 번에(전부 또는 하나도) 반영한다. 표 구조를 바꿀 때 기록·스냅샷·
    /// 시트·기준 시트가 서로 어긋난 채 남지 않게 한다(그사이 꺼지면 바꾸기 전 그대로). 겹쳐 부르면 바깥 묶음 하나다.
    pub fn begin_group(&self) {
        self.group.lock().unwrap().get_or_insert_with(Vec::new);
    }

    /// 모은 쓰기를 한 번에 보낸다.
    pub fn end_group(&self) {
        let ops = self.group.lock().unwrap().take();
        if let Some(ops) = ops.filter(|o| !o.is_empty()) {
            self.send(Op::Group(ops));
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

    /// 앱에서 붙인 행(열)을 `from`부터 `count`개 지웠다: 그 줄의 기록은 지운 것으로 두고(`gone`) 뒤의 기록은 당긴다.
    /// 세션이 메모리의 기록에 한 것([`collect_core::structure::remap_entries`])과 같다.
    pub fn remap(&self, sheet: u32, axis: Axis, from: u32, count: u32) {
        self.send(Op::Remap { sheet, axis, from, count });
    }

    /// 스냅샷 값을 바꿔 쓴다(표 구조가 바뀌어 지금 시트 모양에 맞춘 값).
    pub fn replace_snapshot(&self, id: u32, data: Vec<u8>) {
        self.send(Op::ReplaceSnapshot(id, data));
    }

    /// 그때까지 보낸 쓰기를 모두 반영한 사본을 `to`에 한 파일로 만들라고 보낸다(대상 파일이 있으면 실패).
    /// 받는 쪽은 다른 PC로 옮길 기록 파일(에이전트로 넘기기)을 기다린다. 다른 쓰기를 막지 않으려고 기다림은 맡긴다.
    pub fn backup_request(&self, to: impl Into<PathBuf>) -> Result<mpsc::Receiver<Result<()>>> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.tx.as_ref().ok_or(StoreError::Gone)?.send(Op::Backup(to.into(), tx)).map_err(|_| StoreError::Gone)?;
        Ok(rx)
    }

    /// [`Store::backup_request`]하고 끝날 때까지 기다린다.
    pub fn backup_to(&self, to: impl Into<PathBuf>) -> Result<()> {
        self.backup_request(to)?.recv().map_err(|_| StoreError::Gone)?
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
    migrate(&conn)?;
    let mut out = Saved::default();
    let mut stmt = conn.prepare(
        "SELECT seq, at_ms, user, batch, origin, kind, row, col, old, new, sheet, gone FROM changes ORDER BY seq",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(ChangeEntry {
            seq: r.get::<_, i64>(0)? as u64,
            at_ms: r.get(1)?,
            user: r.get(2)?,
            batch: r.get(3)?,
            origin: from_json(&format!("\"{}\"", r.get::<_, String>(4)?))?,
            kind: from_json(&format!("\"{}\"", r.get::<_, String>(5)?))?,
            sheet: r.get(10)?,
            row: r.get(6)?,
            col: r.get(7)?,
            old: from_json(&r.get::<_, String>(8)?)?,
            new: from_json(&r.get::<_, String>(9)?)?,
            gone: r.get(11)?,
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
            sheet: 0,
            row: 2,
            col: 5,
            old: CellValue::Empty,
            new,
            gone: false,
        }
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("collect-store-test-{}", std::process::id()));
        let path = dir.join(name);
        let _ = std::fs::remove_file(&path);
        path
    }

    #[test]
    fn backup_has_every_write_queued_before_it() {
        let path = temp("backup-src.sqlite");
        let copy = temp("backup-copy.sqlite");
        let store = Store::open(&path).unwrap();
        store.append(&[entry(1, CellValue::Number(1.0)), entry(2, CellValue::Number(2.0))]);
        store.meta("job", "j-1");
        store.blob("sheet", 2, b"blob".to_vec());
        store.backup_to(&copy).unwrap();
        // 같은 대상에 다시 만들면 덮지 않고 실패한다.
        assert!(matches!(store.backup_to(&copy), Err(StoreError::Backup(_))));
        store.append(&[entry(3, CellValue::Number(3.0))]);
        drop(store);
        let saved = load(&copy).unwrap();
        assert_eq!(saved.changes.len(), 2, "사본 뒤의 쓰기는 들지 않는다");
        assert_eq!(saved.meta.get("job").map(String::as_str), Some("j-1"));
        assert_eq!(saved.blobs.get("sheet").map(|(seq, d)| (*seq, d.as_slice())), Some((2, b"blob".as_slice())));
        assert!(!copy.with_extension("sqlite-wal").exists(), "사본은 한 파일");
        let _ = std::fs::remove_file(&copy);
    }

    #[test]
    fn grouped_writes_land_together_or_not_at_all() {
        let path = temp("group.sqlite");
        let store = Store::open(&path).unwrap();
        store.append(&[entry(1, CellValue::Number(1.0))]);
        store.begin_group();
        store.remap(0, Axis::Row, 2, 1);
        store.blob("sheet", 1, b"after".to_vec());
        store.begin_group(); // 겹쳐 불러도 하나
        store.blob("baseline", 1, b"after".to_vec());
        store.flush().unwrap();
        assert!(load(&path).unwrap().blobs.is_empty(), "묶음이 끝나기 전에는 쓰지 않는다");
        store.end_group();
        store.end_group(); // 빈 묶음은 보내지 않는다
        store.flush().unwrap();
        let saved = load(&path).unwrap();
        assert!(saved.changes[0].gone);
        assert_eq!(saved.blobs.len(), 2);
        // 묶음 안에서 하나가 실패하면 앞의 쓰기도 되돌린다.
        conn_write_fails_whole_group(&path);
        drop(store);
    }

    fn conn_write_fails_whole_group(path: &Path) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch("CREATE TRIGGER no_meta BEFORE INSERT ON meta BEGIN SELECT RAISE(ABORT, 'no'); END;")
            .unwrap();
        let ops = vec![
            Op::Blob("x".into(), 9, b"x".to_vec()),
            Op::Remap { sheet: 0, axis: Axis::Row, from: 0, count: 1 },
            Op::Meta("job".into(), "j".into()), // 실패
        ];
        assert!(write(&conn, Op::Group(ops)).is_err());
        let n: i64 = conn.query_row("SELECT count(*) FROM blobs WHERE key = 'x'", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 0);
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
    fn sheet_snapshots_read_old_and_new_shapes() {
        let a = vec![CellValue::Number(5.0), CellValue::Empty];
        let b = vec![CellValue::Text("x".into())];
        assert_eq!(unpack_sheets(&pack_sheets(&[a.clone(), b.clone()])).unwrap(), [a.clone(), b]);
        // 여러 시트 전의 스냅샷은 시트 하나.
        assert_eq!(unpack_sheets(&pack(&a)).unwrap(), [a]);
        // 숫자 하나뿐인 시트를 감싼 새 모양도 옛 모양(오류 값 `{"err": 5}`)으로 잘못 읽지 않는다.
        let one = vec![vec![CellValue::Number(5.0)]];
        assert_eq!(unpack_sheets(&pack_sheets(&one)).unwrap(), one);
        assert_eq!(unpack_sheets(&pack(&[])).unwrap(), [Vec::<CellValue>::new()]);
    }

    #[test]
    fn old_files_get_a_sheet_column() {
        let path = temp("old.sqlite");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        {
            // 여러 시트 전의 스키마(변경 기록에 sheet 열이 없다).
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE changes (seq INTEGER PRIMARY KEY, at_ms INTEGER NOT NULL, user TEXT NOT NULL,
                    batch TEXT NOT NULL, origin TEXT NOT NULL, kind TEXT NOT NULL, row INTEGER NOT NULL,
                    col INTEGER NOT NULL, old TEXT NOT NULL, new TEXT NOT NULL);
                 INSERT INTO changes VALUES (1, 1000, '이서연', 'b1', 'paste', 'input', 2, 5, 'null', '9800');",
            )
            .unwrap();
        }
        let saved = load(&path).unwrap();
        assert_eq!(saved.changes, [entry(1, CellValue::Number(9800.0))]);
        let store = Store::open(&path).unwrap();
        let mut e = entry(2, CellValue::Number(1.0));
        e.sheet = 1;
        store.append(std::slice::from_ref(&e));
        store.flush().unwrap();
        drop(store);
        assert_eq!(load(&path).unwrap().changes[1], e);
        let _ = std::fs::remove_file(&path);
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

    #[test]
    fn removed_lines_mark_history_gone_and_pull_the_rest() {
        let path = temp("remap.sqlite");
        let at = |seq, row, col| ChangeEntry { row, col, ..entry(seq, CellValue::Number(seq as f64)) };
        {
            let store = Store::open(&path).unwrap();
            store.append(&[at(1, 3, 0), at(2, 4, 0), at(3, 5, 0), ChangeEntry { sheet: 1, ..at(4, 5, 0) }]);
            store.remap(0, Axis::Row, 4, 1);
            store.remap(0, Axis::Col, 0, 1);
            let meta = SnapshotMeta { id: 1, at_ms: 5, kind: SnapshotKind::Manual, seq: 3, label: String::new() };
            store.snapshot(&meta, &pack(&[CellValue::Empty]));
            store.replace_snapshot(1, pack(&[CellValue::Number(2.0)]));
            store.flush().unwrap();
        }
        let saved = load(&path).unwrap();
        let shape: Vec<(u32, u32, bool)> = saved.changes.iter().map(|e| (e.row, e.col, e.gone)).collect();
        assert_eq!(shape, [(3, 0, true), (4, 0, true), (4, 0, true), (5, 0, false)]);
        assert_eq!(unpack(&saved.snapshots[0].1).unwrap(), [CellValue::Number(2.0)]);
        let _ = std::fs::remove_file(&path);
    }
}
