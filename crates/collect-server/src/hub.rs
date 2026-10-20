//! 참여자 서버 하나(포트 하나)에 공개한 여러 취합 업무(세션). 업무는 주소 경로(slug)로 가린다.
//!
//! 호스트가 업무를 공개하면 [`Hub::publish`]로 넣고, 공개를 멈추거나 업무를 끝내면 [`Hub::unpublish`]로 뺀다. 빼면 그 업무에
//! 붙어 있던 WebSocket만 `closing`을 받고 닫힌다(다른 업무의 연결은 그대로). 세션 액터는 자기가 공개되었는지 모른다.

use std::net::IpAddr;
use std::sync::{Arc, RwLock};

use axum::http::StatusCode;
use serde::Serialize;
use tokio::sync::watch;
use ts_rs::TS;

use crate::{IpNames, SessionHandle};

/// 주소 경로로 쓸 수 없는 이름(서버가 쓰는 경로·흔한 파일 이름).
pub const RESERVED_SLUGS: &[&str] =
    &["api", "ws", "assets", "__dev", "agent", "index.html", "favicon.ico", "robots.txt"];
pub const MAX_SLUG_CHARS: usize = 32;

/// 주소 경로: 영문·숫자·`-`·`_` 1~32자, 예약어가 아닐 것(대소문자 무시).
pub fn validate_slug(slug: &str) -> Result<(), String> {
    if slug.is_empty() || slug.len() > MAX_SLUG_CHARS {
        return Err(format!("주소 경로는 1~{MAX_SLUG_CHARS}자로 정해 주세요"));
    }
    if !slug.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err("주소 경로에는 영문·숫자·-·_만 쓸 수 있습니다".into());
    }
    if RESERVED_SLUGS.iter().any(|r| r.eq_ignore_ascii_case(slug)) {
        return Err(format!("‘{slug}’은(는) 주소 경로로 쓸 수 없습니다"));
    }
    Ok(())
}

/// 참여자 업무 목록(`GET /api/sessions`)의 한 줄.
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SessionListItem {
    pub slug: String,
    /// 업무명(취합 제목).
    pub title: String,
    pub requester: String,
    pub description: String,
    #[ts(optional)]
    pub deadline: Option<String>,
    pub read_only: bool,
    pub closed: bool,
    pub online: u32,
    /// 이 PC(IP 명단의 이름)가 그 업무 명단에 있다.
    pub mine: bool,
}

/// 공개가 끝난 까닭(그 업무의 WebSocket에 알린다).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gone {
    /// 공개만 멈췄다(다시 공개하면 이어서).
    Paused,
    /// 호스트가 업무를 끝냈다.
    Ended,
    /// 마감 시각이 되어 공유를 닫았다(마감 동작 '공유 닫기', 다시 공개하면 이어서).
    Deadline,
}

struct Entry {
    slug: String,
    session: SessionHandle,
    gone: watch::Sender<Option<Gone>>,
}

#[derive(Default)]
struct Inner {
    /// 공개한 순서.
    entries: RwLock<Vec<Entry>>,
    /// IP별 이름(업무 목록의 '내 담당' 표시용 — 입장 판단은 세션마다 한다).
    ip_names: RwLock<IpNames>,
}

/// 공개한 업무들. 복제하면 같은 목록을 가리킨다.
#[derive(Clone, Default)]
pub struct Hub(Arc<Inner>);

/// WebSocket·API가 고른 업무.
pub(crate) struct Target {
    pub session: SessionHandle,
    pub gone: watch::Receiver<Option<Gone>>,
}

impl Hub {
    pub fn new() -> Self {
        Self::default()
    }

    /// 업무를 이 주소 경로로 공개한다. 같은 세션을 같은 경로로 다시 공개하면 그대로 둔다.
    pub fn publish(&self, slug: &str, session: SessionHandle) -> Result<(), String> {
        validate_slug(slug)?;
        let mut entries = self.0.entries.write().unwrap();
        if let Some(e) = entries.iter().find(|e| e.session.same(&session)) {
            return if e.slug.eq_ignore_ascii_case(slug) {
                Ok(())
            } else {
                Err(format!("이 업무는 이미 ‘{}’ 주소로 공개 중입니다", e.slug))
            };
        }
        if entries.iter().any(|e| e.slug.eq_ignore_ascii_case(slug)) {
            return Err(format!("‘{slug}’ 주소는 다른 업무가 쓰고 있습니다"));
        }
        entries.push(Entry { slug: slug.to_string(), session, gone: watch::channel(None).0 });
        Ok(())
    }

    /// 공개를 멈춘다. 그 업무에 붙어 있던 연결은 `closing`(`ended`면 업무를 끝냈다고)을 받고 닫힌다.
    pub fn unpublish(&self, slug: &str, ended: bool) -> Option<SessionHandle> {
        self.unpublish_as(slug, if ended { Gone::Ended } else { Gone::Paused })
    }

    /// [`unpublish`](Self::unpublish)와 같지만 까닭을 고른다(마감 시각에 공유 닫기).
    pub fn unpublish_as(&self, slug: &str, why: Gone) -> Option<SessionHandle> {
        let mut entries = self.0.entries.write().unwrap();
        let i = entries.iter().position(|e| e.slug.eq_ignore_ascii_case(slug))?;
        let entry = entries.remove(i);
        // 받는 연결이 없어도 값은 남겨 둔다(그사이 붙은 연결도 곧바로 본다).
        entry.gone.send_replace(Some(why));
        Some(entry.session)
    }

    /// 그 세션을 공개한 주소 경로.
    pub fn slug_of(&self, session: &SessionHandle) -> Option<String> {
        self.0.entries.read().unwrap().iter().find(|e| e.session.same(session)).map(|e| e.slug.clone())
    }

    pub fn get(&self, slug: &str) -> Option<SessionHandle> {
        self.0.entries.read().unwrap().iter().find(|e| e.slug.eq_ignore_ascii_case(slug)).map(|e| e.session.clone())
    }

    pub fn is_empty(&self) -> bool {
        self.0.entries.read().unwrap().is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.entries.read().unwrap().len()
    }

    /// 공개 중인 업무(주소 경로, 세션) — 공개한 순서.
    pub fn sessions(&self) -> Vec<(String, SessionHandle)> {
        self.0.entries.read().unwrap().iter().map(|e| (e.slug.clone(), e.session.clone())).collect()
    }

    /// 업무 목록의 '내 담당'에 쓸 IP 명단(입장 판단에 쓰는 명단은 세션마다 따로 넣는다).
    pub fn set_ip_names(&self, names: IpNames) {
        *self.0.ip_names.write().unwrap() = names;
    }

    /// 참여자 업무 목록. 세션의 공개 정보·접속자 표(watch)만 읽으므로 호스트 작업이 길어도 막히지 않는다.
    pub fn list(&self, viewer: IpAddr) -> Vec<SessionListItem> {
        let name = self.0.ip_names.read().unwrap().get(viewer).map(str::to_string);
        self.0
            .entries
            .read()
            .unwrap()
            .iter()
            .map(|e| {
                let info = e.session.info();
                SessionListItem {
                    slug: e.slug.clone(),
                    title: info.title,
                    requester: info.requester,
                    description: info.description,
                    deadline: info.deadline,
                    read_only: info.read_only,
                    closed: info.closed,
                    online: info.online,
                    mine: name.as_deref().is_some_and(|n| e.session.has_person(n)),
                }
            })
            .collect()
    }

    /// 요청이 가리키는 업무: 주소 경로(`s`)가 있으면 그 업무, 없으면 공개 중인 업무가 하나뿐일 때 그 업무
    /// (주소 경로를 모르는 옛 화면·도구도 쓸 수 있게).
    pub(crate) fn resolve(&self, slug: Option<&str>) -> Result<Target, (StatusCode, &'static str)> {
        let entries = self.0.entries.read().unwrap();
        let entry = match slug.map(str::trim).filter(|s| !s.is_empty()) {
            Some(s) => entries
                .iter()
                .find(|e| e.slug.eq_ignore_ascii_case(s))
                .ok_or((StatusCode::NOT_FOUND, "공개 중인 업무가 아닙니다"))?,
            None => match entries.as_slice() {
                [only] => only,
                [] => return Err((StatusCode::NOT_FOUND, "공개 중인 업무가 없습니다")),
                _ => return Err((StatusCode::BAD_REQUEST, "업무 주소(s)가 필요합니다")),
            },
        };
        // 목록을 잡은 채로 수신기를 만들어, 바로 뒤의 공개 중지도 놓치지 않는다.
        Ok(Target { session: entry.session.clone(), gone: entry.gone.subscribe() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use collect_core::SheetModel;
    use collect_core::config::{CollectConfig, suggested_key_col};

    fn session(title: &str) -> SessionHandle {
        let model: SheetModel = serde_json::from_slice(include_bytes!("../../../fixtures/budget.sheet.json")).unwrap();
        let mut config = CollectConfig::suggest(&model);
        config.assignment.key_col = suggested_key_col(&model);
        config.title = title.into();
        SessionHandle::spawn(crate::SessionInit::new(model, config))
    }

    #[test]
    fn slugs_are_checked() {
        assert!(validate_slug("q4").is_ok());
        assert!(validate_slug("hr-2026_a").is_ok());
        assert!(validate_slug("").is_err());
        assert!(validate_slug(&"a".repeat(33)).is_err());
        assert!(validate_slug("예산").is_err());
        assert!(validate_slug("a/b").is_err());
        assert!(validate_slug("API").is_err(), "예약어는 대소문자를 가리지 않는다");
        assert!(validate_slug("ws").is_err());
        assert!(validate_slug("__dev").is_err());
    }

    #[tokio::test]
    async fn publish_resolve_and_unpublish() {
        let hub = Hub::new();
        let (a, b) = (session("예산"), session("인력"));
        assert!(matches!(hub.resolve(None), Err((StatusCode::NOT_FOUND, _))));
        hub.publish("q4", a.clone()).unwrap();
        hub.publish("q4", a.clone()).unwrap(); // 다시 공개해도 그대로
        assert!(hub.publish("Q4", b.clone()).unwrap_err().contains("다른 업무"), "대소문자 무시");
        assert!(hub.publish("hr2", a.clone()).unwrap_err().contains("이미"));
        // 하나뿐이면 주소 없이도 그 업무
        assert!(hub.resolve(None).unwrap().session.same(&a));
        hub.publish("hr", b.clone()).unwrap();
        assert!(matches!(hub.resolve(None), Err((StatusCode::BAD_REQUEST, _))));
        assert!(hub.resolve(Some("HR")).unwrap().session.same(&b));
        assert!(matches!(hub.resolve(Some("nope")), Err((StatusCode::NOT_FOUND, _))));
        assert_eq!(hub.sessions().iter().map(|(s, _)| s.as_str()).collect::<Vec<_>>(), ["q4", "hr"], "공개한 순서");
        assert_eq!(hub.slug_of(&b).as_deref(), Some("hr"));

        let mut gone = hub.resolve(Some("hr")).unwrap().gone;
        assert_eq!(*gone.borrow(), None);
        assert!(hub.unpublish("hr", true).unwrap().same(&b));
        gone.changed().await.unwrap();
        assert_eq!(*gone.borrow(), Some(Gone::Ended));
        assert!(hub.unpublish("hr", false).is_none());
        assert_eq!(hub.len(), 1);
        // 다시 공개하면 새 신호(앞 연결의 신호와 섞이지 않는다)
        hub.publish("hr", b).unwrap();
        assert_eq!(*hub.resolve(Some("hr")).unwrap().gone.borrow(), None);
    }

    #[tokio::test]
    async fn list_marks_my_sessions() {
        let hub = Hub::new();
        let (a, b) = (session("2026년 부서별 예산 신청"), session("인력 계획"));
        hub.publish("q4", a.clone()).unwrap();
        hub.publish("hr", b.clone()).unwrap();
        let me: IpAddr = "192.168.0.11".parse().unwrap();
        let list = hub.list(me);
        assert_eq!(list.iter().map(|i| i.title.as_str()).collect::<Vec<_>>(), ["2026년 부서별 예산 신청", "인력 계획"]);
        assert!(list.iter().all(|i| !i.mine), "IP 명단에 없으면 내 담당 없음");

        // 김민지는 두 업무 모두 행 담당(예산 시트의 담당자 열). b에서는 담당자 열을 지운다.
        hub.set_ip_names(IpNames::parse_json(r#"{"192.168.0.11": "김민지"}"#).unwrap());
        b.with(|s| {
            s.sheets[0].config.assignment.key_col = None;
            s.changed();
        })
        .await
        .unwrap();
        let list = hub.list(me);
        assert!(list[0].mine);
        assert!(!list[1].mine, "명단에서 빠졌다");
        assert!(!hub.list("192.168.0.12".parse().unwrap())[0].mine);
    }
}
