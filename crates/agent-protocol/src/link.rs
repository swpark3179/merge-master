//! 관리자 연결(`GET /agent/services/{id}/host`, WebSocket)의 형식: 에이전트로 넘긴 업무를 호스트 앱이 이 PC의 업무처럼
//! 다룬다(설정·모니터링·소통·히스토리·스냅샷·참여자 승인 — 호스트 앱의 업무 command와 같은 [`HostRequest`]).
//!
//! - 헤더 [`HEADER_AGENT`](crate::HEADER_AGENT)가 [`LINK_PROTOCOL`] 이상이고 [`HEADER_TOKEN`](crate::HEADER_TOKEN)이 그
//!   서비스의 관리 키여야 한다. 브라우저 요청(`Origin`)은 받지 않는다(참여자 브라우저는 여전히 참여자만 된다).
//! - 호스트가 [`LinkFrame`]을 보내면 에이전트가 같은 `id`로 [`LinkMsg::Ok`]나 [`LinkMsg::Err`]를 돌려준다. 그 밖에
//!   세션이 바뀔 때마다 에이전트가 [`LinkMsg::Push`]를 밀어 준다(붙자마자 한 번씩).
//! - 값은 세션이 만든 JSON 그대로 옮긴다(호스트 앱은 받은 그대로 화면에 보낸다).
//! - 서비스 중일 때만 붙는다(아니면 409). 서비스가 끝나면 에이전트가 닫는다.

use std::collections::BTreeMap;

use collect_server::RecalcStatus;
use collect_server::host::HostRequest;
use serde::{Deserialize, Serialize};

use crate::ServiceStatus;

/// 관리자 연결이 생긴 관리 API 판.
pub const LINK_PROTOCOL: u32 = 2;
/// 메시지 하나의 최대 크기(시트 전체를 보내는 `State`·모니터링 보기도 들어가게).
pub const MAX_LINK_MESSAGE: usize = 256 * 1024 * 1024;

/// 그 서비스의 관리자 연결 경로.
pub fn link_path(id: &str) -> String {
    format!("/agent/services/{id}/host")
}

/// 호스트 → 에이전트: 요청 하나.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LinkFrame {
    pub id: u64,
    pub call: LinkCall,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum LinkCall {
    /// 업무 command 하나(답은 그 결과 JSON).
    Host { req: HostRequest },
    /// 업무를 처음 그릴 것([`LinkState`]).
    State,
    /// 호스트 PC의 IP 명단(`{"IP": "이름"}`): 그 서비스가 넘겨받은 명단을 바꾼다(에이전트 PC 명단에 있는 IP는 그쪽이 이긴다).
    IpNames { names: BTreeMap<String, String> },
}

/// [`LinkCall::State`]의 답.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkState {
    /// 확정한 시트들(`SheetModel`, 세션 안 순번 순서).
    pub models: serde_json::Value,
    /// 접속자 표(`HostSnapshot`).
    pub snapshot: serde_json::Value,
    /// 공지·강조·채팅(`CommsSnapshot`).
    pub comms: serde_json::Value,
    pub recalc: RecalcStatus,
    pub status: ServiceStatus,
}

/// 에이전트 → 호스트.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum LinkMsg {
    Ok { id: u64, value: serde_json::Value },
    Err { id: u64, message: String },
    Push { push: HostPush },
}

/// 세션이 바뀌었다(호스트 앱은 이 PC 업무와 같은 이름의 이벤트로 화면에 보낸다).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "camelCase")]
pub enum HostPush {
    /// 접속자 표(`HostSnapshot` → `participants:{id}`).
    Snapshot(serde_json::Value),
    /// 소통(`CommsSnapshot` → `comms:{id}`).
    Comms(serde_json::Value),
    /// 셀 변경·서식·크기·다시 읽기(`HostEvent` → `sheet-changes:{id}`).
    Event(serde_json::Value),
    /// 수식 계산 상태(→ `recalc-status:{id}`).
    Recalc(RecalcStatus),
    /// 서비스 상태(마감·저장 등 — 사이드바·에이전트 단계).
    Status(ServiceStatus),
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn frames_look_like_this() {
        let f = LinkFrame { id: 7, call: LinkCall::Host { req: HostRequest::CommsGet } };
        let text = serde_json::to_value(&f).unwrap();
        assert_eq!(text, json!({ "id": 7, "call": { "kind": "host", "req": { "req": "commsGet" } } }));
        assert_eq!(serde_json::from_value::<LinkFrame>(text).unwrap(), f);
        let names = LinkCall::IpNames { names: BTreeMap::from([("10.0.0.5".into(), "이서연".into())]) };
        assert_eq!(
            serde_json::to_value(&names).unwrap(),
            json!({ "kind": "ipNames", "names": { "10.0.0.5": "이서연" } })
        );

        let push = LinkMsg::Push { push: HostPush::Event(json!({ "type": "reset" })) };
        let text = serde_json::to_value(&push).unwrap();
        assert_eq!(text, json!({ "t": "push", "push": { "kind": "event", "value": { "type": "reset" } } }));
        assert_eq!(serde_json::from_value::<LinkMsg>(text).unwrap(), push);
        let err: LinkMsg = serde_json::from_value(json!({ "t": "err", "id": 3, "message": "안 됨" })).unwrap();
        assert_eq!(err, LinkMsg::Err { id: 3, message: "안 됨".into() });
    }
}
