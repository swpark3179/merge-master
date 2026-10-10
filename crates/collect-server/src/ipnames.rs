//! IP별 참여자 이름(호스트가 JSON 파일로 관리하는 접속자 명단).
//!
//! 이 명단에 있는 IP에서 접속하면 이름을 묻지 않고 그 이름으로 바로 들어온다. 명단 밖의 IP는 이름을 입력하고 호스트가
//! 승인해야 들어온다. 호스트 PC의 IP가 명단에 있으면 그 이름이 호스트 이름이 된다(공지·채팅·변경 기록).
//!
//! 파일 모양: `{"192.168.0.11": "이서연", "192.168.0.12": "최도윤"}`. 불러올 때는 `[{"ip": …, "name": …}]`도 받는다.

use std::collections::BTreeMap;
use std::net::IpAddr;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// 이름 최대 길이(참여자가 입력하는 이름과 같다).
pub const MAX_NAME_CHARS: usize = 20;
/// 명단 최대 항목 수.
pub const MAX_ENTRIES: usize = 2_000;

/// 화면에 보이는 한 줄.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct IpEntry {
    pub ip: String,
    pub name: String,
}

/// 호스트 1c 화면의 IP 명단.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct IpNamesView {
    pub entries: Vec<IpEntry>,
    /// 이 PC(호스트)의 LAN 주소. 명단에 있으면 그 이름이 호스트 이름이다.
    pub host_ips: Vec<String>,
    /// 명단을 둔 파일(앱 설정 폴더).
    pub path: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IpNames(BTreeMap<IpAddr, String>);

/// IPv4를 IPv6로 감싼 주소(`::ffff:192.168.0.5`)는 IPv4로 본다.
pub fn canonical(ip: IpAddr) -> IpAddr {
    ip.to_canonical()
}

impl IpNames {
    pub fn get(&self, ip: IpAddr) -> Option<&str> {
        self.0.get(&canonical(ip)).map(String::as_str)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// IP 순서로.
    pub fn entries(&self) -> Vec<IpEntry> {
        self.0.iter().map(|(ip, name)| IpEntry { ip: ip.to_string(), name: name.clone() }).collect()
    }

    /// 한 사람을 더하거나 이름을 바꾼다(승인할 때 'IP 명단에 저장').
    pub fn put(&mut self, ip: &str, name: &str) -> Result<(), String> {
        let mut entries = self.entries();
        entries.push(IpEntry { ip: ip.to_string(), name: name.to_string() });
        *self = Self::from_entries(&entries)?;
        Ok(())
    }

    /// 화면에서 고친 목록. 빈 줄은 건너뛰고, 같은 IP가 여러 번이면 마지막 것을 쓴다.
    pub fn from_entries(entries: &[IpEntry]) -> Result<Self, String> {
        let mut map = BTreeMap::new();
        for e in entries {
            let (ip, name) = (e.ip.trim(), e.name.trim());
            if ip.is_empty() && name.is_empty() {
                continue;
            }
            let addr: IpAddr = ip.parse().map_err(|_| format!("IP 주소가 잘못되었습니다: {ip}"))?;
            if name.is_empty() {
                return Err(format!("{ip}의 이름이 비어 있습니다"));
            }
            if name.chars().count() > MAX_NAME_CHARS {
                return Err(format!("이름은 {MAX_NAME_CHARS}자까지입니다: {name}"));
            }
            map.insert(canonical(addr), name.to_string());
        }
        if map.len() > MAX_ENTRIES {
            return Err(format!("명단은 {MAX_ENTRIES}줄까지입니다"));
        }
        Ok(Self(map))
    }

    /// JSON 파일 내용. `{"IP": "이름"}` 또는 `[{"ip": "IP", "name": "이름"}]`.
    pub fn parse_json(text: &str) -> Result<Self, String> {
        let text = text.trim_start_matches('\u{feff}');
        let value: serde_json::Value =
            serde_json::from_str(text).map_err(|e| format!("JSON 파일을 읽을 수 없습니다: {e}"))?;
        let entries = match value {
            serde_json::Value::Object(map) => map
                .into_iter()
                .map(|(ip, name)| match name {
                    serde_json::Value::String(name) => Ok(IpEntry { ip, name }),
                    _ => Err(format!("{ip}의 이름은 글자여야 합니다")),
                })
                .collect::<Result<Vec<_>, _>>()?,
            serde_json::Value::Array(_) => serde_json::from_value::<Vec<IpEntry>>(value)
                .map_err(|e| format!("목록 모양이 잘못되었습니다(ip·name): {e}"))?,
            _ => return Err("JSON은 {\"IP\": \"이름\"} 모양이어야 합니다".into()),
        };
        Self::from_entries(&entries)
    }

    /// 파일에 쓸 JSON(IP 순서, 한 줄에 한 사람).
    pub fn to_json(&self) -> String {
        if self.0.is_empty() {
            return "{}\n".into();
        }
        let lines: Vec<String> = self
            .0
            .iter()
            .map(|(ip, name)| {
                let key = serde_json::to_string(&ip.to_string()).expect("문자열");
                let value = serde_json::to_string(name).expect("문자열");
                format!("  {key}: {value}")
            })
            .collect();
        format!("{{\n{}\n}}\n", lines.join(",\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_object_and_list_and_round_trips() {
        let a = IpNames::parse_json("\u{feff}{\"192.168.0.12\": \" 최도윤 \", \"192.168.0.2\": \"이서연\"}").unwrap();
        assert_eq!(a.get("192.168.0.2".parse().unwrap()), Some("이서연"));
        assert_eq!(a.get("::ffff:192.168.0.12".parse().unwrap()), Some("최도윤"));
        assert_eq!(a.entries()[0].ip, "192.168.0.2", "IP 순서");
        let b = IpNames::parse_json(
            r#"[{"ip": "192.168.0.2", "name": "이서연"}, {"ip": "192.168.0.12", "name": "최도윤"}]"#,
        )
        .unwrap();
        assert_eq!(a, b);
        assert_eq!(IpNames::parse_json(&a.to_json()).unwrap(), a);
        assert_eq!(a.to_json(), "{\n  \"192.168.0.2\": \"이서연\",\n  \"192.168.0.12\": \"최도윤\"\n}\n");
        assert_eq!(IpNames::parse_json(&IpNames::default().to_json()).unwrap(), IpNames::default());
    }

    #[test]
    fn rejects_bad_entries() {
        assert!(IpNames::parse_json(r#"{"192.168.0.300": "가"}"#).unwrap_err().contains("IP 주소"));
        assert!(IpNames::parse_json(r#"{"192.168.0.3": ""}"#).unwrap_err().contains("비어"));
        assert!(IpNames::parse_json(r#"{"192.168.0.3": 3}"#).is_err());
        assert!(IpNames::parse_json("[1, 2]").is_err());
        assert!(IpNames::parse_json("\"x\"").is_err());
        let long = "가".repeat(MAX_NAME_CHARS + 1);
        assert!(IpNames::from_entries(&[IpEntry { ip: "10.0.0.1".into(), name: long }]).is_err());
        // 빈 줄은 건너뛴다(화면에서 추가만 하고 비워 둔 줄).
        let blank = IpEntry { ip: " ".into(), name: String::new() };
        assert!(IpNames::from_entries(&[blank]).unwrap().is_empty());
    }
}
