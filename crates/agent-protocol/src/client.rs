//! 호스트 앱이 에이전트에 보내는 요청(HTTP/1.1, 요청마다 연결 하나). 사내망의 에이전트에 바로 붙는다(프록시를 쓰지 않는다).

use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::{
    AgentInfo, ApiError, Approve, Created, DEFAULT_PORT, Finish, FolderList, HEADER_AGENT, HEADER_TOKEN, PROTOCOL,
    Package, Reject, ServiceStatus, StatusItem, StatusQuery,
};

/// 연결·짧은 요청을 기다리는 시간.
const SHORT: Duration = Duration::from_secs(5);
/// 넘기기(큰 묶음)를 기다리는 시간.
const UPLOAD: Duration = Duration::from_secs(300);
/// 응답의 최대 크기.
const MAX_RESPONSE: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub enum ClientError {
    /// 에이전트에 닿지 않았거나 응답이 끊겼다(넘겼는지 모를 수 있다).
    Unreachable(String),
    /// 에이전트가 거절했다(상태 코드 + 에이전트의 문구).
    Api { status: u16, message: String },
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientError::Unreachable(m) => write!(f, "에이전트에 연결할 수 없습니다: {m}"),
            ClientError::Api { status: 426, message } => {
                write!(
                    f,
                    "{message} (Release 페이지의 merge-master-agent 실행 파일을 이 앱과 같은 버전으로 받아 주세요)"
                )
            }
            ClientError::Api { message, .. } => f.write_str(message),
        }
    }
}

impl From<ClientError> for String {
    fn from(e: ClientError) -> String {
        e.to_string()
    }
}

/// 에이전트 주소 하나.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentClient {
    host: String,
    port: u16,
}

impl AgentClient {
    /// `PC01`, `10.0.0.7`, `10.0.0.7:8090`, `http://10.0.0.7:8090/` 모두 받는다(포트가 없으면 8090).
    pub fn new(addr: &str) -> Result<AgentClient, String> {
        let t = addr.trim();
        let t = t.strip_prefix("http://").or_else(|| t.strip_prefix("HTTP://")).unwrap_or(t);
        if t.starts_with("https://") {
            return Err("에이전트 주소는 http://로 씁니다".into());
        }
        let t = t.split(['/', '?', '#']).next().unwrap_or("");
        let (host, port) = match t.strip_prefix('[') {
            Some(rest) => {
                let (h, tail) = rest.split_once(']').ok_or("에이전트 주소가 올바르지 않습니다")?;
                (h.to_string(), tail.strip_prefix(':').map(str::to_string))
            }
            None => match t.rsplit_once(':') {
                Some((h, p)) if !h.contains(':') => (h.to_string(), Some(p.to_string())),
                _ => (t.to_string(), None),
            },
        };
        if host.is_empty() || host.chars().any(|c| c.is_whitespace() || "@\\\"'<>".contains(c)) {
            return Err("에이전트 주소(PC 이름 또는 IP)를 입력해 주세요".into());
        }
        let port = match port.filter(|p| !p.is_empty()) {
            Some(p) => p.parse::<u16>().ok().filter(|&p| p > 0).ok_or("에이전트 포트가 올바르지 않습니다")?,
            None => DEFAULT_PORT,
        };
        Ok(AgentClient { host, port })
    }

    /// `http://host:port`(같은 에이전트를 같은 글자로 기억한다).
    pub fn base_url(&self) -> String {
        if self.host.contains(':') {
            format!("http://[{}]:{}", self.host, self.port)
        } else {
            format!("http://{}:{}", self.host, self.port)
        }
    }

    /// 그 서비스의 관리자 연결 주소(`ws://host:port/agent/services/{id}/host`).
    pub fn link_url(&self, id: &str) -> String {
        format!("ws://{}{}", self.base_url().trim_start_matches("http://"), crate::link::link_path(id))
    }

    pub async fn info(&self) -> Result<AgentInfo, ClientError> {
        self.call("GET", "/agent/info", None, Vec::new(), SHORT).await
    }

    pub async fn folders(&self, path: &[String]) -> Result<FolderList, ClientError> {
        let q = encode_query(&path.join("/"));
        self.call("GET", &format!("/agent/folders?path={q}"), None, Vec::new(), SHORT).await
    }

    pub async fn create(&self, pkg: &Package) -> Result<Created, ClientError> {
        self.call("POST", "/agent/services", None, pkg.encode(), UPLOAD).await
    }

    pub async fn status(&self, queries: &[StatusQuery]) -> Result<Vec<StatusItem>, ClientError> {
        self.call("POST", "/agent/status", None, json(queries), SHORT).await
    }

    pub async fn approve(&self, id: &str, token: &str, body: &Approve) -> Result<ServiceStatus, ClientError> {
        self.call("POST", &format!("/agent/services/{id}/approve"), Some(token), json(body), SHORT).await
    }

    pub async fn reject(&self, id: &str, token: &str, name: &str) -> Result<ServiceStatus, ClientError> {
        let body = Reject { name: name.to_string() };
        self.call("POST", &format!("/agent/services/{id}/reject"), Some(token), json(&body), SHORT).await
    }

    pub async fn finish(&self, id: &str, token: &str, save: bool) -> Result<ServiceStatus, ClientError> {
        self.call("POST", &format!("/agent/services/{id}/finish"), Some(token), json(&Finish { save }), SHORT).await
    }

    pub async fn retry(&self, id: &str, token: &str) -> Result<ServiceStatus, ClientError> {
        self.call("POST", &format!("/agent/services/{id}/retry"), Some(token), Vec::new(), SHORT).await
    }

    async fn call<T: DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        token: Option<&str>,
        body: Vec<u8>,
        wait: Duration,
    ) -> Result<T, ClientError> {
        let (status, bytes) = tokio::time::timeout(wait, self.send(method, path, token, &body))
            .await
            .map_err(|_| ClientError::Unreachable("응답이 없습니다(시간 초과)".into()))??;
        if !(200..300).contains(&status) {
            let message = serde_json::from_slice::<ApiError>(&bytes)
                .map(|e| e.message)
                .unwrap_or_else(|_| format!("에이전트 오류({status})"));
            return Err(ClientError::Api { status, message });
        }
        serde_json::from_slice(&bytes).map_err(|e| ClientError::Unreachable(format!("응답을 읽을 수 없습니다: {e}")))
    }

    async fn send(
        &self,
        method: &str,
        path: &str,
        token: Option<&str>,
        body: &[u8],
    ) -> Result<(u16, Vec<u8>), ClientError> {
        let io = |e: std::io::Error| ClientError::Unreachable(e.to_string());
        let mut stream = tokio::time::timeout(SHORT, tokio::net::TcpStream::connect((self.host.as_str(), self.port)))
            .await
            .map_err(|_| ClientError::Unreachable(format!("{} 에 닿지 않습니다(시간 초과)", self.base_url())))?
            .map_err(io)?;
        let host = self.base_url().trim_start_matches("http://").to_string();
        let mut head = format!(
            "{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n{HEADER_AGENT}: {PROTOCOL}\r\nContent-Length: {}\r\n",
            body.len()
        );
        if !body.is_empty() {
            let kind = if body.starts_with(b"MMAGENT") { "application/octet-stream" } else { "application/json" };
            head.push_str(&format!("Content-Type: {kind}\r\n"));
        }
        if let Some(t) = token {
            head.push_str(&format!("{HEADER_TOKEN}: {t}\r\n"));
        }
        head.push_str("\r\n");
        stream.write_all(head.as_bytes()).await.map_err(io)?;
        stream.write_all(body).await.map_err(io)?;
        stream.flush().await.map_err(io)?;
        let mut out = Vec::new();
        let mut chunk = [0u8; 64 * 1024];
        loop {
            let n = stream.read(&mut chunk).await.map_err(io)?;
            if n == 0 {
                break;
            }
            out.extend_from_slice(&chunk[..n]);
            if out.len() > MAX_RESPONSE {
                return Err(ClientError::Unreachable("응답이 너무 큽니다".into()));
            }
        }
        parse_response(&out).ok_or_else(|| ClientError::Unreachable("응답이 올바르지 않습니다".into()))
    }
}

fn json<T: Serialize + ?Sized>(v: &T) -> Vec<u8> {
    serde_json::to_vec(v).expect("직렬화")
}

/// 쿼리 값 하나를 퍼센트 인코딩한다(UTF-8).
fn encode_query(text: &str) -> String {
    let mut out = String::new();
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// 상태 코드와 본문(Content-Length·chunked·닫힘까지).
fn parse_response(raw: &[u8]) -> Option<(u16, Vec<u8>)> {
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = std::str::from_utf8(&raw[..split]).ok()?;
    let mut lines = head.split("\r\n");
    let status = lines.next()?.split_whitespace().nth(1)?.parse().ok()?;
    let mut length = None;
    let mut chunked = false;
    for line in lines {
        let Some((k, v)) = line.split_once(':') else { continue };
        match k.trim().to_ascii_lowercase().as_str() {
            "content-length" => length = v.trim().parse::<usize>().ok(),
            "transfer-encoding" => chunked = v.to_ascii_lowercase().contains("chunked"),
            _ => {}
        }
    }
    let body = &raw[split + 4..];
    let body = if chunked {
        dechunk(body)?
    } else if let Some(n) = length {
        body.get(..n)?.to_vec()
    } else {
        body.to_vec()
    };
    Some((status, body))
}

fn dechunk(mut b: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let line = b.windows(2).position(|w| w == b"\r\n")?;
        let size = std::str::from_utf8(&b[..line]).ok()?.split(';').next()?.trim();
        let n = usize::from_str_radix(size, 16).ok()?;
        if n == 0 {
            return Some(out);
        }
        let start = line + 2;
        out.extend_from_slice(b.get(start..start + n)?);
        b = b.get(start + n + 2..)?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses() {
        let c = |s: &str| AgentClient::new(s).map(|c| c.base_url());
        assert_eq!(c("PC01").unwrap(), "http://PC01:8090");
        assert_eq!(c(" 10.0.0.7 ").unwrap(), "http://10.0.0.7:8090");
        assert_eq!(c("10.0.0.7:9000").unwrap(), "http://10.0.0.7:9000");
        assert_eq!(c("http://10.0.0.7:8090/q4").unwrap(), "http://10.0.0.7:8090");
        assert_eq!(c("[::1]:8090").unwrap(), "http://[::1]:8090");
        assert!(c("").is_err());
        assert!(c("10.0.0.7:abc").is_err());
        assert!(c("https://x").is_err());
        assert!(c("a b").is_err());
    }

    #[test]
    fn responses() {
        let r = b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\n{}";
        assert_eq!(parse_response(r), Some((200, b"{}".to_vec())));
        let r = b"HTTP/1.1 400 Bad\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n2\r\nde\r\n0\r\n\r\n";
        assert_eq!(parse_response(r), Some((400, b"abcde".to_vec())));
        assert_eq!(parse_response(b"garbage"), None);
        assert_eq!(encode_query("2026/새 폴더"), "2026%2F%EC%83%88%20%ED%8F%B4%EB%8D%94");
    }
}
