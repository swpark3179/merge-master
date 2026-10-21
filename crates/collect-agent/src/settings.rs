//! 에이전트 설정(`<데이터 폴더>\agent.json`)과 폴더 위치.

use std::path::{Path, PathBuf};

use agent_protocol::{DEFAULT_MAX_DAYS, DEFAULT_PORT};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// 기본 저장 폴더: 호스트는 이 아래에서만 저장 위치를 고른다.
    pub base_dir: PathBuf,
    /// 참여자 접속과 관리 API를 받는 포트.
    pub port: u16,
    /// 마감은 넘겨받는 때부터 이 날 수 안이어야 한다.
    pub max_days: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self { base_dir: default_base_dir(), port: DEFAULT_PORT, max_days: DEFAULT_MAX_DAYS }
    }
}

impl Settings {
    /// 읽는다. 없거나 읽을 수 없으면 기본값으로 만들어 둔다(설정 파일 열기로 고칠 수 있게).
    pub fn load(path: &Path) -> Settings {
        match std::fs::read(path) {
            Ok(bytes) => match serde_json::from_slice::<Settings>(&bytes) {
                Ok(s) => return s.normalized(),
                Err(e) => tracing::warn!("설정 파일을 읽을 수 없어 기본값을 씁니다: {e}"),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => tracing::warn!("설정 파일을 열 수 없어 기본값을 씁니다: {e}"),
        }
        let s = Settings::default();
        if let Err(e) = s.save(path) {
            tracing::warn!("설정 파일을 만들 수 없습니다: {e}");
        }
        s
    }

    fn normalized(mut self) -> Settings {
        if self.base_dir.as_os_str().is_empty() {
            self.base_dir = default_base_dir();
        }
        if self.port == 0 {
            self.port = DEFAULT_PORT;
        }
        if self.max_days == 0 {
            self.max_days = DEFAULT_MAX_DAYS;
        }
        self
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        write_atomic(path, &serde_json::to_vec_pretty(self).expect("직렬화"))
    }
}

/// 임시 파일에 쓰고 바꿔 넣는다(쓰다가 꺼져도 앞 파일이 남는다).
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

/// [`write_atomic`]과 같되 바꿔 넣기 전에 디스크까지 쓴다(전원이 나가도 남아야 하는 취합 데이터).
pub(crate) fn write_synced(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    let mut f = std::fs::File::create(&tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(&tmp, path)
}

/// 사용자 홈(Windows는 `USERPROFILE`).
fn home() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from)
}

/// 기본 저장 폴더 기본값: 문서\엑셀 취합 결과.
pub fn default_base_dir() -> PathBuf {
    home().map(|h| h.join("Documents")).unwrap_or_else(|| PathBuf::from(".")).join("엑셀 취합 결과")
}

/// 데이터 폴더 기본값: `%LOCALAPPDATA%\merge-master-agent`(설정·서비스·로그·계산 사본).
pub fn default_data_dir() -> PathBuf {
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(local).join("merge-master-agent");
    }
    home().map(|h| h.join(".local").join("share")).unwrap_or_else(|| PathBuf::from(".")).join("merge-master-agent")
}

/// 이 PC 이름.
pub fn pc_name() -> String {
    ["COMPUTERNAME", "HOSTNAME"]
        .iter()
        .find_map(|k| std::env::var(k).ok().filter(|v| !v.trim().is_empty()))
        .unwrap_or_else(|| "에이전트 PC".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_is_created_with_defaults_and_bad_values_are_fixed() {
        let dir = std::env::temp_dir().join(format!("agent-settings-{}", std::process::id()));
        let path = dir.join("agent.json");
        let _ = std::fs::remove_file(&path);
        let s = Settings::load(&path);
        assert_eq!(s.port, DEFAULT_PORT);
        assert!(path.exists());
        std::fs::write(&path, r#"{"baseDir":"","port":0,"maxDays":7}"#).unwrap();
        let s = Settings::load(&path);
        assert_eq!((s.port, s.max_days), (DEFAULT_PORT, 7));
        assert!(!s.base_dir.as_os_str().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
