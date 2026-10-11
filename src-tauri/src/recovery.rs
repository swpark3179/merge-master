//! 7단계 세션 복구의 기록 파일: 업무마다 시트를 확정할 때 `sessions\YYYYMMDD-HHMMSS-mmm.sqlite`를 만들고, 앱을 켜면
//! 끝내지 않은 업무를 모두 되살린다([`crate::jobs::jobs_restore`]). 어느 파일을 되살리고 지울지는
//! [`collect_server::session::files`]가 메타만 보고 정한다.

use std::path::{Path, PathBuf};

use collect_server::session::files::{StoreFile, cleanup_plan};
use tauri::{AppHandle, Manager};

/// 남겨 둘 기록 파일 수(더 오래된 것은 앱을 켤 때 지운다 — 끝내지 않은 업무의 지금 파일은 오래되어도 남긴다).
const KEEP_FILES: usize = 20;

pub(crate) fn sessions_dir(app: &AppHandle) -> Option<PathBuf> {
    Some(app.path().app_local_data_dir().ok()?.join("sessions"))
}

/// 기록 파일(이름이 시각이라 이름순 = 시간순), 최근 것부터.
fn session_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|it| it.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "sqlite")).collect())
        .unwrap_or_default();
    files.sort();
    files.reverse();
    files
}

/// 기록 파일과 그 메타(최근 것부터). 읽을 수 없는 파일은 되살릴 수 없는 것으로 둔다.
pub(crate) fn store_files(dir: &Path) -> Vec<(PathBuf, StoreFile)> {
    session_files(dir)
        .into_iter()
        .map(|path| {
            let name = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            let file = match collect_store::peek(&path) {
                Ok(peek) => StoreFile { name, meta: peek.meta, resumable: peek.resumable },
                Err(_) => StoreFile { name, ..Default::default() },
            };
            (path, file)
        })
        .collect()
}

/// 새 기록 파일 이름(만든 시각, ms까지 — 업무 둘이 같은 초에 확정해도 겹치지 않게, 그래도 있으면 번호).
pub(crate) fn new_file(dir: &Path) -> PathBuf {
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S-%3f").to_string();
    std::iter::once(dir.join(format!("{stamp}.sqlite")))
        .chain((2..).map(|n| dir.join(format!("{stamp}-{n}.sqlite"))))
        .find(|p| !p.exists())
        .expect("끝없는 후보")
}

fn remove_with_sidecars(p: &Path) {
    let _ = std::fs::remove_file(p);
    for ext in ["-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{ext}", p.display()));
    }
}

/// 최근 [`KEEP_FILES`]개와 끝내지 않은 업무의 지금 파일만 남긴다.
pub(crate) fn cleanup(dir: &Path) {
    let files = store_files(dir);
    let metas: Vec<StoreFile> = files.iter().map(|(_, f)| f.clone()).collect();
    for i in cleanup_plan(&metas, KEEP_FILES) {
        remove_with_sidecars(&files[i].0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_only_recent_files() {
        let dir = std::env::temp_dir().join(format!("mm-sessions-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for i in 0..25 {
            let p = dir.join(format!("20261009-1200{i:02}.sqlite"));
            std::fs::write(&p, b"x").unwrap();
            std::fs::write(format!("{}-wal", p.display()), b"x").unwrap();
        }
        std::fs::write(dir.join("다른 파일.txt"), b"x").unwrap();
        cleanup(&dir);
        let left = session_files(&dir);
        assert_eq!(left.len(), KEEP_FILES);
        assert!(left[0].ends_with("20261009-120024.sqlite"), "최근 것부터");
        assert!(!dir.join("20261009-120000.sqlite-wal").exists(), "딸린 파일도 지운다");
        assert!(dir.join("다른 파일.txt").exists());
        let next = new_file(&dir);
        assert!(next.extension().is_some_and(|e| e == "sqlite") && !next.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
