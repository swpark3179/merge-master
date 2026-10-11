//! 기록 파일 고르기: 앱을 켤 때 되살릴 업무와 지울 옛 파일. 호스트 앱이 디렉터리를 읽어 넘기고(파일 이름은 만든 시각이라
//! 이름순 = 시간순), 여기서는 메타만 보고 정한다.
//!
//! 업무 하나는 시트를 다시 확정할 때마다 새 파일을 만든다. 같은 업무의 파일은 메타 [`META_JOB`]이 같고, 그중 가장 최근 것이
//! 지금 상태다.

use std::collections::{HashMap, HashSet};

use super::persist::{META_DISCARDED, META_ENDED, META_JOB};

/// 기록 파일 하나(최근 것부터 넘긴다).
#[derive(Debug, Clone, Default)]
pub struct StoreFile {
    /// 확장자를 뺀 파일 이름(`20261011-093012-123`).
    pub name: String,
    pub meta: HashMap<String, String>,
    /// 시트가 남아 있어 되살릴 수 있다.
    pub resumable: bool,
}

/// 되살릴 업무.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumePick {
    /// 넘긴 목록에서의 자리.
    pub index: usize,
    pub job: String,
    /// 업무 id가 없는 옛(여러 업무 전) 파일. 되살린 뒤 id를 적어 둔다.
    pub legacy: bool,
}

fn flag(file: &StoreFile, key: &str) -> bool {
    file.meta.get(key).is_some_and(|v| v == "1")
}

fn live(file: &StoreFile) -> bool {
    file.resumable && !flag(file, META_ENDED) && !flag(file, META_DISCARDED)
}

/// 옛 파일을 되살릴 때 줄 업무 id.
pub fn legacy_job_id(name: &str) -> String {
    format!("legacy-{name}")
}

/// 되살릴 업무들(최근 것부터): 업무마다 가장 최근 파일이 끝나지(버려지지) 않았으면 그 파일.
///
/// 업무 id가 없는 옛 파일은 디렉터리에서 **가장 최근 파일일 때만** 되살린다(여러 업무로 올린 직후 한 번 — 그 뒤로는
/// 새 업무 파일이 앞에 오므로, 앞서 버리거나 바꾼 옛 취합이 줄줄이 되살아나지 않는다).
pub fn resume_plan(files: &[StoreFile]) -> Vec<ResumePick> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for (index, file) in files.iter().enumerate() {
        match file.meta.get(META_JOB).filter(|j| !j.is_empty()) {
            Some(job) => {
                if seen.insert(job.clone()) && live(file) {
                    out.push(ResumePick { index, job: job.clone(), legacy: false });
                }
            }
            None if index == 0 && live(file) => {
                out.push(ResumePick { index, job: legacy_job_id(&file.name), legacy: true });
            }
            None => {}
        }
    }
    out
}

/// 지울 파일(넘긴 목록의 자리): 최근 `keep`개 밖의 것. 단, 끝나지 않은 업무의 지금 파일은 오래되어도 남긴다.
pub fn cleanup_plan(files: &[StoreFile], keep: usize) -> Vec<usize> {
    let protected: HashSet<usize> = resume_plan(files).into_iter().map(|p| p.index).collect();
    (keep..files.len()).filter(|i| !protected.contains(i)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str, job: Option<&str>, extra: &[(&str, &str)]) -> StoreFile {
        let mut meta: HashMap<String, String> = extra.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        if let Some(job) = job {
            meta.insert(META_JOB.into(), job.into());
        }
        StoreFile { name: name.into(), meta, resumable: true }
    }

    #[test]
    fn newest_file_per_live_job() {
        let files = vec![
            file("6", Some("b"), &[]),
            file("5", Some("a"), &[]),
            file("4", Some("b"), &[]),                  // b의 옛 확정
            file("3", Some("c"), &[(META_ENDED, "1")]), // 끝낸 업무
            file("2", Some("c"), &[]),                  // 끝낸 업무의 옛 파일도 되살리지 않는다
            file("1", Some("d"), &[(META_DISCARDED, "1")]),
            file("0", None, &[]), // 맨 앞이 아닌 옛 파일
        ];
        let picks = resume_plan(&files);
        assert_eq!(
            picks,
            [
                ResumePick { index: 0, job: "b".into(), legacy: false },
                ResumePick { index: 1, job: "a".into(), legacy: false }
            ]
        );
    }

    #[test]
    fn unresumable_latest_hides_the_job() {
        let mut latest = file("2", Some("a"), &[]);
        latest.resumable = false;
        let files = vec![latest, file("1", Some("a"), &[])];
        assert!(resume_plan(&files).is_empty(), "앞 확정의 파일로 돌아가지 않는다");
    }

    #[test]
    fn legacy_file_only_when_newest() {
        let files = vec![file("20261009-120000", None, &[]), file("20261008-120000", None, &[])];
        assert_eq!(resume_plan(&files), [ResumePick { index: 0, job: "legacy-20261009-120000".into(), legacy: true }]);
        let discarded = vec![file("x", None, &[(META_DISCARDED, "1")]), file("y", None, &[])];
        assert!(resume_plan(&discarded).is_empty());
        // 되살린 옛 파일은 id가 적히므로 다음에는 보통 업무로 고른다.
        let stamped = vec![file("x", Some("legacy-x"), &[]), file("w", None, &[])];
        assert_eq!(resume_plan(&stamped)[0].job, "legacy-x");
        assert_eq!(resume_plan(&stamped).len(), 1);
    }

    #[test]
    fn cleanup_keeps_recent_and_live_jobs() {
        let mut files: Vec<StoreFile> = (0..6).map(|i| file(&format!("{}", 9 - i), Some("new"), &[])).collect();
        files.push(file("3", Some("old"), &[])); // 오래 진행 중인 업무의 하나뿐인 파일
        files.push(file("2", Some("gone"), &[(META_ENDED, "1")]));
        files.push(file("1", None, &[]));
        assert_eq!(cleanup_plan(&files, 4), [4, 5, 7, 8]);
        assert!(cleanup_plan(&files, 20).is_empty());
    }
}
