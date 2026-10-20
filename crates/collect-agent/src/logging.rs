//! 로그: `<데이터 폴더>\logs\agent.log`(창이 없으니 파일로). 5MB를 넘으면 켤 때 `agent.1.log`로 돌린다.

use std::path::Path;

use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::writer::MakeWriterExt;

const ROTATE_BYTES: u64 = 5 * 1024 * 1024;

/// 로그를 시작한다. `console`이면 표준 오류에도 쓴다(`--headless`). 로그 파일을 열 수 없으면 표준 오류에만.
pub fn init(dir: &Path, console: bool) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let path = dir.join("agent.log");
    let file = std::fs::create_dir_all(dir).ok().and_then(|()| {
        if std::fs::metadata(&path).is_ok_and(|m| m.len() > ROTATE_BYTES) {
            let _ = std::fs::rename(&path, dir.join("agent.1.log"));
        }
        std::fs::OpenOptions::new().create(true).append(true).open(&path).ok()
    });
    let builder = tracing_subscriber::fmt().with_env_filter(filter).with_ansi(false);
    match (file, console) {
        (Some(f), true) => builder.with_writer(std::sync::Mutex::new(f).and(std::io::stderr)).init(),
        (Some(f), false) => builder.with_writer(std::sync::Mutex::new(f)).init(),
        (None, _) => builder.with_writer(std::io::stderr).init(),
    }
}
