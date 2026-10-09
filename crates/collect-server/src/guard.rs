//! 7단계 보안: WebSocket Origin 검사, 연결마다 메시지 빈도 제한, HTTP 보안 헤더.
//!
//! 참여자는 이름만으로 들어오므로(사내 LAN 전제) 인증은 없다. 여기서는 다른 웹사이트가 참여자 브라우저를 통해
//! 이 서버에 붙는 것(교차 사이트 WebSocket)과, 한 연결이 메시지를 쏟아부어 세션을 막는 것을 막는다.

use std::time::Instant;

use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::Response;

/// 연결당 메시지 빈도: 평소 초당 이만큼, 순간적으로는 `BURST`까지.
pub const RATE_PER_SEC: f64 = 50.0;
pub const BURST: f64 = 200.0;
/// 빈도를 넘겨 버린 메시지가 이만큼 쌓이면 연결을 끊는다.
pub const MAX_DROPPED: u32 = 500;

/// 토큰 버킷.
pub struct Bucket {
    tokens: f64,
    last: Instant,
    pub dropped: u32,
}

impl Default for Bucket {
    fn default() -> Self {
        Self { tokens: BURST, last: Instant::now(), dropped: 0 }
    }
}

impl Bucket {
    /// 메시지 하나를 받아도 되는가. 안 되면 버린 수를 센다.
    pub fn take(&mut self, now: Instant) -> bool {
        let dt = now.saturating_duration_since(self.last).as_secs_f64();
        self.last = now;
        self.tokens = (self.tokens + dt * RATE_PER_SEC).min(BURST);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            self.dropped += 1;
            false
        }
    }
}

fn host_part(authority: &str) -> &str {
    // "[::1]:8080" → "[::1]", "127.0.0.1:8080" → "127.0.0.1"
    if let Some(end) = authority.find(']') {
        return &authority[..=end];
    }
    authority.rsplit_once(':').map_or(authority, |(h, _)| h)
}

fn is_loopback(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "[::1]")
}

/// WebSocket을 열어도 되는 Origin인가: Origin이 없거나(브라우저 밖 도구), 접속한 주소와 같은 곳에서 열린 페이지거나,
/// 이 PC의 개발 서버(localhost)에서 열린 페이지일 때.
pub fn origin_allowed(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) else { return true };
    let Some(authority) = origin.strip_prefix("http://").or_else(|| origin.strip_prefix("https://")) else {
        return false;
    };
    let host = headers.get(header::HOST).and_then(|v| v.to_str().ok()).unwrap_or_default();
    authority.eq_ignore_ascii_case(host) || is_loopback(&host_part(authority).to_ascii_lowercase())
}

/// 모든 응답에 붙이는 보안 헤더(참여자 화면은 같은 서버의 스크립트·WebSocket만 쓴다).
pub async fn security_headers(mut res: Response) -> Response {
    let h = res.headers_mut();
    let set = |h: &mut HeaderMap, k: header::HeaderName, v: &'static str| {
        h.entry(k).or_insert(HeaderValue::from_static(v));
    };
    set(h, header::X_CONTENT_TYPE_OPTIONS, "nosniff");
    set(h, header::X_FRAME_OPTIONS, "DENY");
    set(h, header::REFERRER_POLICY, "no-referrer");
    set(
        h,
        header::CONTENT_SECURITY_POLICY,
        "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; \
         connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'",
    );
    res
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn headers(origin: Option<&str>, host: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(header::HOST, host.parse().unwrap());
        if let Some(o) = origin {
            h.insert(header::ORIGIN, o.parse().unwrap());
        }
        h
    }

    #[test]
    fn origins() {
        assert!(origin_allowed(&headers(None, "192.168.0.24:8080")));
        assert!(origin_allowed(&headers(Some("http://192.168.0.24:8080"), "192.168.0.24:8080")));
        assert!(origin_allowed(&headers(Some("http://localhost:5173"), "127.0.0.1:8080")), "개발 서버(Vite 프록시)");
        assert!(origin_allowed(&headers(Some("http://[::1]:5173"), "127.0.0.1:8080")));
        assert!(!origin_allowed(&headers(Some("http://evil.example"), "192.168.0.24:8080")));
        assert!(!origin_allowed(&headers(Some("http://192.168.0.99:8080"), "192.168.0.24:8080")));
        assert!(!origin_allowed(&headers(Some("null"), "192.168.0.24:8080")));
    }

    #[test]
    fn bucket_allows_bursts_then_limits() {
        let mut b = Bucket::default();
        let t0 = Instant::now();
        let ok = (0..300).filter(|_| b.take(t0)).count();
        assert_eq!(ok, BURST as usize);
        assert_eq!(b.dropped, 100);
        // 1초 뒤에는 초당 몫만큼 다시 받는다.
        let ok = (0..100).filter(|_| b.take(t0 + Duration::from_secs(1))).count();
        assert_eq!(ok, RATE_PER_SEC as usize);
    }
}
