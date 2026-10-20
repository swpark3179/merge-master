//! 트레이 'IP 명단 관리' 창이 보내는 요청(에이전트 PC에서만 — HTTP 경로가 없어 다른 PC에서는 닿지 않는다). 창은
//! [`crate::admin_window`](WebView2)가 띄우고, 요청은 JSON 한 통([`handle_json`])으로 받아 지금 상태([`AdminView`])를
//! 돌려준다.
//!
//! - 에이전트 명단(`ip-names.json`): 더하기·고치기·빼기·통째로 바꾸기(JSON 불러오기). 이 명단이 이긴다.
//! - 넘겨받은 명단(서비스마다): 에이전트 명단에 넣기, 그 서비스에서 빼기.
//! - 명단 밖 참여자(자동 승인으로 들어온 사람): IP와 함께 보이고 '명단에 넣기'.
//! - 승인 대기(같은 이름이 접속 중, 호스트가 끊은 이름): 승인·거부(트레이처럼 관리 키 없이).

use std::collections::BTreeSet;
use std::net::IpAddr;
use std::sync::Arc;

use agent_protocol::{Approve, ServiceState};
use collect_server::protocol::PendingReason;
use collect_server::{IpEntry, IpNames, ParticipantStatus};
use serde::{Deserialize, Serialize};

use crate::agent::Agent;

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Request {
    State,
    /// 한 사람을 더하거나 이름을 바꾼다.
    Put {
        ip: String,
        name: String,
    },
    Remove {
        ip: String,
    },
    /// 명단을 통째로 바꾼다(화면의 표).
    Save {
        entries: Vec<IpEntry>,
    },
    /// JSON 파일 내용으로 바꾼다(`{"IP": "이름"}` 또는 `[{"ip", "name"}]`).
    ImportText {
        text: String,
    },
    /// 넘겨받은 명단을 에이전트 명단에 넣는다(`ip`가 없으면 그 서비스의 것 모두 — 에이전트 명단에 있는 IP는 그대로).
    AdoptReceived {
        service: String,
        ip: Option<String>,
    },
    /// 넘겨받은 명단에서 뺀다(그 서비스에서만).
    RemoveReceived {
        service: String,
        ip: String,
    },
    Approve {
        service: String,
        name: String,
    },
    Reject {
        service: String,
        name: String,
    },
}

/// 에이전트 명단 한 줄.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryRow {
    pub ip: String,
    pub name: String,
    /// 그 IP에서 접속 중인 서비스(제목).
    pub online: Vec<String>,
    /// 같은 이름이 다른 IP에도 있다.
    pub duplicate: bool,
}

/// 서비스가 넘겨받은 명단 한 줄.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReceivedRow {
    pub service: String,
    pub title: String,
    pub ip: String,
    pub name: String,
    /// 에이전트 명단에 같은 IP가 있어 쓰지 않는다(에이전트 명단이 이긴다).
    pub shadowed: bool,
}

/// 명단 밖 참여자(자동 승인으로 들어온 사람).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutsiderRow {
    pub service: String,
    pub title: String,
    pub name: String,
    pub ip: String,
    pub online: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingRow {
    pub service: String,
    pub title: String,
    pub name: String,
    pub ip: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<PendingReason>,
}

/// 창에 그릴 지금 상태(요청마다 돌려준다).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminView {
    /// 명단 파일(직접 고쳐도 된다 — 에이전트를 다시 켜면 읽는다).
    pub path: String,
    pub entries: Vec<EntryRow>,
    pub received: Vec<ReceivedRow>,
    pub outsiders: Vec<OutsiderRow>,
    pub pending: Vec<PendingRow>,
    /// 이 PC의 IP(참여자가 이 PC에서 열면 이 IP로 보인다).
    pub agent_ips: Vec<String>,
}

fn same_ip(a: &str, b: &str) -> bool {
    match (a.parse::<IpAddr>(), b.parse::<IpAddr>()) {
        (Ok(a), Ok(b)) => a.to_canonical() == b.to_canonical(),
        _ => a == b,
    }
}

/// 지금 상태.
pub fn view(agent: &Arc<Agent>) -> AdminView {
    let names = agent.ip_names();
    let entries = names.entries();
    let services: Vec<_> = agent.services_now().into_iter().filter(|s| s.state() == ServiceState::Running).collect();
    let mut received = Vec::new();
    let mut outsiders = Vec::new();
    let mut pending = Vec::new();
    let mut online_at: Vec<(String, String)> = Vec::new(); // (IP, 서비스 제목)
    for s in &services {
        let record = s.record();
        let title = record.manifest.title.clone();
        for (ip, name) in &record.manifest.ip_names {
            let shadowed = entries.iter().any(|e| same_ip(&e.ip, ip));
            received.push(ReceivedRow {
                service: s.id.clone(),
                title: title.clone(),
                ip: ip.clone(),
                name: name.clone(),
                shadowed,
            });
        }
        let Some(session) = s.session() else { continue };
        let effective = agent.effective_ip_names(s);
        let snap = session.snapshot();
        for p in &snap.participants {
            let Some(ip) = &p.ip else { continue };
            let online = p.status != ParticipantStatus::Offline;
            if online {
                online_at.push((ip.clone(), title.clone()));
            }
            if ip.parse::<IpAddr>().ok().and_then(|a| effective.get(a)).is_none() {
                outsiders.push(OutsiderRow {
                    service: s.id.clone(),
                    title: title.clone(),
                    name: p.name.clone(),
                    ip: ip.clone(),
                    online,
                });
            }
        }
        for p in &snap.pending {
            pending.push(PendingRow {
                service: s.id.clone(),
                title: title.clone(),
                name: p.name.clone(),
                ip: p.ip.clone(),
                reason: p.reason,
            });
        }
    }
    let rows = entries
        .iter()
        .map(|e| {
            let online: BTreeSet<String> =
                online_at.iter().filter(|(ip, _)| same_ip(ip, &e.ip)).map(|(_, t)| t.clone()).collect();
            EntryRow {
                ip: e.ip.clone(),
                name: e.name.clone(),
                online: online.into_iter().collect(),
                duplicate: entries.iter().any(|o| o.name == e.name && o.ip != e.ip),
            }
        })
        .collect();
    AdminView {
        path: agent.ip_names_path().display().to_string(),
        entries: rows,
        received,
        outsiders,
        pending,
        agent_ips: collect_server::lan_addresses().into_iter().map(|a| a.ip).collect(),
    }
}

/// 요청 하나를 하고 지금 상태를 돌려준다.
pub async fn handle(agent: &Arc<Agent>, req: Request) -> Result<AdminView, String> {
    match req {
        Request::State => {}
        Request::Put { ip, name } => agent.put_ip_name(&ip, &name)?,
        Request::Remove { ip } => agent.remove_ip_name(&ip)?,
        Request::Save { entries } => agent.set_ip_names(IpNames::from_entries(&entries)?)?,
        Request::ImportText { text } => agent.set_ip_names(IpNames::parse_json(&text)?)?,
        Request::AdoptReceived { service, ip } => {
            let s = agent.services_now().into_iter().find(|s| s.id == service).ok_or("에이전트에 없는 서비스입니다")?;
            let received = s.record().manifest.ip_names;
            let picked: Vec<IpEntry> = received
                .iter()
                .filter(|(k, _)| ip.as_deref().is_none_or(|ip| same_ip(k, ip)))
                .map(|(ip, name)| IpEntry { ip: ip.clone(), name: name.clone() })
                .collect();
            if picked.is_empty() {
                return Err("넘겨받은 명단에 없는 IP입니다".into());
            }
            let mut names = agent.ip_names();
            names.merge_missing(&IpNames::from_entries(&picked)?);
            agent.set_ip_names(names)?;
        }
        Request::RemoveReceived { service, ip } => agent.remove_received(&service, &ip).map_err(|e| e.message)?,
        Request::Approve { service, name } => {
            agent.approve(&service, None, Approve { name, note: String::new() }).await.map_err(|e| e.message)?;
        }
        Request::Reject { service, name } => {
            agent.reject(&service, None, &name).await.map_err(|e| e.message)?;
        }
    }
    Ok(view(agent))
}

/// 창이 보낸 JSON 요청 → `{"ok":true,"view":…}` 또는 `{"ok":false,"message":…}`(그대로 JS 값으로 쓸 수 있다).
pub async fn handle_json(agent: &Arc<Agent>, body: &str) -> String {
    let out = match serde_json::from_str::<Request>(body) {
        Ok(req) => handle(agent, req).await,
        Err(e) => Err(format!("알 수 없는 요청입니다: {e}")),
    };
    match out {
        Ok(view) => serde_json::json!({ "ok": true, "view": view }).to_string(),
        Err(message) => serde_json::json!({ "ok": false, "message": message }).to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_read_from_the_page() {
        let r: Request = serde_json::from_str(r#"{"op":"put","ip":"10.0.0.1","name":"이서연"}"#).unwrap();
        assert_eq!(r, Request::Put { ip: "10.0.0.1".into(), name: "이서연".into() });
        let r: Request = serde_json::from_str(r#"{"op":"adoptReceived","service":"s1"}"#).unwrap();
        assert_eq!(r, Request::AdoptReceived { service: "s1".into(), ip: None });
        assert!(serde_json::from_str::<Request>(r#"{"op":"nope"}"#).is_err());
        assert!(same_ip("::ffff:10.0.0.1", "10.0.0.1") && !same_ip("10.0.0.1", "10.0.0.2"));
    }
}
