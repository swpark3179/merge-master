//! 기본 저장 폴더 안의 경로: 호스트가 보낸 경로 조각을 검사하고, 기본 폴더 밖(`..`, 드라이브, 정션)으로 나가지 못하게 한다.

use std::path::{Path, PathBuf};

use agent_protocol::{FolderList, MAX_PATH_CHARS, check_name, extension};

const WORKBOOK_EXTS: [&str; 4] = ["xlsx", "xlsm", "xlsb", "xls"];

/// `a/b`(또는 `a\b`) → 조각. 빈 조각은 버리고, 조각마다 이름으로 쓸 수 있는지 본다.
pub fn split(rel: &str) -> Result<Vec<String>, String> {
    let segs: Vec<String> = rel.split(['/', '\\']).filter(|s| !s.is_empty()).map(str::to_string).collect();
    check_segments(&segs)?;
    Ok(segs)
}

pub fn check_segments(segs: &[String]) -> Result<(), String> {
    segs.iter().try_for_each(|s| check_name(s))
}

/// 기본 폴더 + 조각. 결과가 기본 폴더 밖이면(정션·바로 가기 폴더로 나감) 거절한다.
pub fn join(base: &Path, segs: &[String]) -> Result<PathBuf, String> {
    check_segments(segs)?;
    let path = segs.iter().fold(base.to_path_buf(), |p, s| p.join(s));
    ensure_inside(base, &path)?;
    Ok(path)
}

/// 이미 있는 가장 가까운 상위 폴더.
fn existing_ancestor(path: &Path) -> Option<&Path> {
    path.ancestors().find(|p| p.exists())
}

/// 조각에는 `..`가 없으므로, 이미 있는 가장 가까운 상위 폴더를 실제 경로로 풀어(정션·링크를 따라가) 기본 폴더 안인지 본다.
fn ensure_inside(base: &Path, path: &Path) -> Result<(), String> {
    // 기본 폴더가 아직 없으면 그 아래도 없다(밖으로 나갈 길이 없다).
    let Ok(base) = base.canonicalize() else { return Ok(()) };
    let Some(near) = existing_ancestor(path) else { return Ok(()) };
    let near = near.canonicalize().map_err(|e| format!("폴더를 확인할 수 없습니다: {e}"))?;
    if near.starts_with(&base) { Ok(()) } else { Err("기본 저장 폴더 밖입니다".into()) }
}

/// Excel이 다루는 길이 안인지.
pub fn check_length(path: &Path) -> Result<(), String> {
    let n = path.to_string_lossy().chars().count();
    if n > MAX_PATH_CHARS {
        return Err(format!(
            "저장 경로가 너무 깁니다({n}자, Excel은 {MAX_PATH_CHARS}자까지). 폴더나 파일 이름을 줄여 주세요"
        ));
    }
    Ok(())
}

/// `dir`(아직 없으면 가장 가까운 상위 폴더)에 파일을 만들 수 있는지 시험한다.
pub fn writable(dir: &Path) -> Result<(), String> {
    let near = existing_ancestor(dir).ok_or_else(|| format!("{}에 쓸 수 없습니다", dir.display()))?;
    let probe = near.join(format!(".merge-master-agent-{}.tmp", std::process::id()));
    std::fs::write(&probe, b"").map_err(|e| format!("{}에 쓸 수 없습니다: {e}", near.display()))?;
    let _ = std::fs::remove_file(&probe);
    Ok(())
}

/// 같은 이름의 파일이 있으면 `이름_2.확장자`, `_3`…(덮어쓰지 않는다).
pub fn unique_file(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }
    let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let ext = path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
    (2..).map(|n| dir.join(format!("{stem}_{n}{ext}"))).find(|p| !p.exists()).expect("빈 이름이 있다")
}

/// 기본 폴더 아래 폴더 하나의 하위 폴더·엑셀 파일(이름 순).
pub fn list(base: &Path, segs: &[String]) -> Result<FolderList, String> {
    let dir = join(base, segs)?;
    let mut folders = Vec::new();
    let mut workbooks = Vec::new();
    let exists = dir.is_dir();
    if exists {
        let entries = std::fs::read_dir(&dir).map_err(|e| format!("폴더를 읽을 수 없습니다: {e}"))?;
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.') || name.starts_with("~$") {
                continue; // 숨은 파일·Excel 잠금 파일
            }
            let Ok(kind) = e.file_type() else { continue };
            let is_dir = kind.is_dir() || (kind.is_symlink() && e.path().is_dir());
            if is_dir {
                folders.push(name);
            } else if WORKBOOK_EXTS.contains(&extension(&name).as_str()) {
                workbooks.push(name);
            }
        }
    }
    let key = |s: &String| s.to_lowercase();
    folders.sort_by_key(key);
    workbooks.sort_by_key(key);
    Ok(FolderList { path: segs.to_vec(), display: dir.display().to_string(), exists, folders, workbooks })
}

/// 넘겨받을 수 있는 원본 형식인가.
pub fn workbook_ext(name: &str) -> Option<String> {
    let ext = extension(name);
    WORKBOOK_EXTS.contains(&ext.as_str()).then_some(ext)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("agent-paths-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn segments_cannot_leave_the_base() {
        assert_eq!(split("a/b\\c").unwrap(), ["a", "b", "c"]);
        assert_eq!(split("").unwrap(), Vec::<String>::new());
        assert!(split("../x").is_err());
        assert!(split("a/../../x").is_err());
        assert!(split("C:").is_err());
        assert!(split("a/con").is_err());
    }

    #[test]
    fn listing_and_missing_folders() {
        let base = temp("list");
        std::fs::create_dir_all(base.join("2026/예산")).unwrap();
        std::fs::write(base.join("2026/a.xlsx"), b"").unwrap();
        std::fs::write(base.join("2026/~$a.xlsx"), b"").unwrap();
        std::fs::write(base.join("2026/note.txt"), b"").unwrap();
        let l = list(&base, &["2026".into()]).unwrap();
        assert!(l.exists);
        assert_eq!(l.folders, ["예산"]);
        assert_eq!(l.workbooks, ["a.xlsx"]);
        let l = list(&base, &["새 폴더".into(), "안".into()]).unwrap();
        assert!(!l.exists && l.folders.is_empty());
        writable(&base.join("새 폴더/안")).unwrap();
        let _ = std::fs::remove_dir_all(&base);
    }

    #[cfg(unix)]
    #[test]
    fn linked_folders_outside_are_refused() {
        let base = temp("link");
        let outside = temp("outside");
        std::os::unix::fs::symlink(&outside, base.join("밖")).unwrap();
        assert!(join(&base, &["밖".into()]).is_err());
        assert!(join(&base, &["밖".into(), "더".into()]).is_err());
        assert!(join(&base, &["안".into()]).is_ok());
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[test]
    fn unique_names_do_not_overwrite() {
        let base = temp("unique");
        let p = base.join("취합.xlsx");
        assert_eq!(unique_file(&p), p);
        std::fs::write(&p, b"").unwrap();
        assert_eq!(unique_file(&p), base.join("취합_2.xlsx"));
        std::fs::write(base.join("취합_2.xlsx"), b"").unwrap();
        assert_eq!(unique_file(&p), base.join("취합_3.xlsx"));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn long_paths_are_refused() {
        assert!(check_length(Path::new(&"a".repeat(219))).is_err());
        assert!(check_length(Path::new(&"a".repeat(218))).is_ok());
        assert_eq!(workbook_ext("Budget.XLSM").as_deref(), Some("xlsm"));
        assert_eq!(workbook_ext("budget.csv"), None);
    }
}
