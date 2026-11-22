//! 서비스 기록의 백업(`services\<id>\backups\`): 서비스 중에 1시간마다(그사이 입력이 있었을 때만) 기록 파일의 사본을 남기고 최근
//! [`KEEP`]개만 둔다. 기록 파일(`session.sqlite`)과 따로 있어, 그 파일이 손상되면 다시 켤 때 가장 최근 백업으로 되살리고, 트레이에서
//! 특정 백업을 엑셀로 저장할 수 있다.
//!
//! 파일 이름은 `backup-YYYYMMDD-HHMMSS.sqlite`(에이전트 PC 현지 시각 — 이름 순이 시간 순). 만들 때는 `.part`로 쓰고 다 쓰면 이름을
//! 바꾼다(도중에 꺼져 남은 반쪽 파일은 목록에 나오지 않고 다음 정리 때 지운다).

use std::path::{Path, PathBuf};

use chrono::NaiveDateTime;

/// 서비스 폴더 안의 백업 폴더 이름.
pub const DIR: &str = "backups";
/// 백업 간격.
pub const EVERY_MS: i64 = 3_600_000;
/// 남겨 둘 백업 수.
pub const KEEP: usize = 3;
/// 다 쓰기 전 이름의 꼬리.
pub const PART: &str = ".part";

const PREFIX: &str = "backup-";
const EXT: &str = ".sqlite";
const STAMP: &str = "%Y%m%d-%H%M%S";

/// 남아 있는 백업 하나.
#[derive(Debug, Clone, PartialEq)]
pub struct BackupFile {
    /// 파일 이름(트레이 메뉴가 고를 때 쓴다).
    pub name: String,
    /// 남긴 시각(에이전트 PC 현지 시각).
    pub at: NaiveDateTime,
    pub path: PathBuf,
}

impl BackupFile {
    /// 트레이에 보일 시각(`11/22 15:00`).
    pub fn label(&self) -> String {
        label(self.at)
    }
}

pub fn dir(svc_dir: &Path) -> PathBuf {
    svc_dir.join(DIR)
}

/// 그 시각의 백업 파일 이름.
pub fn file_name(at: NaiveDateTime) -> String {
    format!("{PREFIX}{}{EXT}", at.format(STAMP))
}

/// 백업 파일 이름 → 시각(형식이 다르면 `None`).
pub fn parse_name(name: &str) -> Option<NaiveDateTime> {
    let stamp = name.strip_prefix(PREFIX)?.strip_suffix(EXT)?;
    NaiveDateTime::parse_from_str(stamp, STAMP).ok()
}

pub fn label(at: NaiveDateTime) -> String {
    at.format("%m/%d %H:%M").to_string()
}

/// 다 쓴 백업들(최신 먼저). 폴더가 없으면 빈 목록.
pub fn list(svc_dir: &Path) -> Vec<BackupFile> {
    let Ok(entries) = std::fs::read_dir(dir(svc_dir)) else { return Vec::new() };
    let mut out: Vec<BackupFile> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| {
            let name = e.file_name().to_str()?.to_string();
            let at = parse_name(&name)?;
            Some(BackupFile { name, at, path: e.path() })
        })
        .collect();
    out.sort_by(|a, b| b.at.cmp(&a.at).then_with(|| b.name.cmp(&a.name)));
    out
}

/// 목록에 있는 이름의 백업(이름으로 폴더 밖을 가리킬 수 없다).
pub fn find(svc_dir: &Path, name: &str) -> Option<BackupFile> {
    list(svc_dir).into_iter().find(|b| b.name == name)
}

/// 최근 `keep`개만 남기고 지운다. 다 쓰지 못한 `.part`도 지운다.
pub fn prune(svc_dir: &Path, keep: usize) {
    for old in list(svc_dir).into_iter().skip(keep) {
        if let Err(e) = std::fs::remove_file(&old.path) {
            tracing::warn!("오래된 백업({})을 지울 수 없습니다: {e}", old.path.display());
        }
    }
    let Ok(entries) = std::fs::read_dir(dir(svc_dir)) else { return };
    for e in entries.flatten() {
        if e.file_name().to_str().is_some_and(|n| n.starts_with(PREFIX) && n.ends_with(PART)) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M:%S").unwrap()
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("agent-backup-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(DIR)).unwrap();
        dir
    }

    #[test]
    fn names_carry_the_time() {
        let t = at("2026-11-22T15:00:07");
        assert_eq!(file_name(t), "backup-20261122-150007.sqlite");
        assert_eq!(parse_name(&file_name(t)), Some(t));
        assert_eq!(label(t), "11/22 15:00");
        assert_eq!(parse_name("backup-20261122-150007.sqlite.part"), None);
        assert_eq!(parse_name("../backup-20261122-150007.sqlite"), None);
        assert_eq!(parse_name("session.sqlite"), None);
    }

    #[test]
    fn listing_skips_partial_and_foreign_files_and_prune_keeps_the_newest() {
        let svc = temp("list");
        let d = dir(&svc);
        for t in ["2026-11-22T12:00:00", "2026-11-22T14:00:00", "2026-11-22T13:00:00", "2026-11-22T15:00:00"] {
            std::fs::write(d.join(file_name(at(t))), b"x").unwrap();
        }
        std::fs::write(d.join(format!("{}{PART}", file_name(at("2026-11-22T16:00:00")))), b"x").unwrap();
        std::fs::write(d.join("note.txt"), b"x").unwrap();
        let names: Vec<String> = list(&svc).into_iter().map(|b| b.label()).collect();
        assert_eq!(names, ["11/22 15:00", "11/22 14:00", "11/22 13:00", "11/22 12:00"]);
        assert!(find(&svc, &file_name(at("2026-11-22T13:00:00"))).is_some());
        assert!(find(&svc, "note.txt").is_none());

        prune(&svc, KEEP);
        let names: Vec<String> = list(&svc).into_iter().map(|b| b.label()).collect();
        assert_eq!(names, ["11/22 15:00", "11/22 14:00", "11/22 13:00"]);
        assert!(!d.join(format!("{}{PART}", file_name(at("2026-11-22T16:00:00")))).exists());
        assert!(d.join("note.txt").exists(), "남의 파일은 건드리지 않는다");
        let _ = std::fs::remove_dir_all(&svc);
    }

    #[test]
    fn missing_folder_is_empty() {
        let svc = std::env::temp_dir().join(format!("agent-backup-{}-none", std::process::id()));
        assert!(list(&svc).is_empty());
        prune(&svc, KEEP);
    }
}
