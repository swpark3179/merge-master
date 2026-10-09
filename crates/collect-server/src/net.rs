use std::net::IpAddr;

use serde::Serialize;
use ts_rs::TS;

/// 참여자가 접속할 수 있는 이 PC의 IPv4 주소.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct LanAddress {
    /// 어댑터 이름(이더넷, Wi-Fi …).
    pub name: String,
    pub ip: String,
}

pub fn lan_addresses() -> Vec<LanAddress> {
    let Ok(ifaces) = if_addrs::get_if_addrs() else { return Vec::new() };
    ifaces
        .into_iter()
        .filter(|i| i.is_oper_up() && !i.is_loopback())
        .filter_map(|i| match i.ip() {
            // 169.254.x.x(APIPA)는 DHCP 실패 주소라 다른 PC가 닿지 않는다.
            IpAddr::V4(v4) if !v4.is_link_local() => Some(LanAddress { name: i.name, ip: v4.to_string() }),
            _ => None,
        })
        .collect()
}
