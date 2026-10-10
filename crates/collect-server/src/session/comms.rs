//! 6단계 소통(1i): 공지(대상·중요·읽음), 셀 강조(보일 사람·확인), 채팅(전체·호스트와 1:1).
//!
//! 받는 사람은 보낼 때 정한다(명단 기준, 접속하지 않은 사람은 들어올 때 `welcome`으로 받는다).
//! 호스트 화면에는 [`CommsSnapshot`]을 따로 내보낸다(바뀔 때만).

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Duration;

use collect_core::CellRange;
use serde::{Deserialize, Serialize};
use tokio::time::Instant;
use ts_rs::TS;

use super::edit::now_ms;
use super::{ConnId, State};
use crate::protocol::{ChatMessage, ChatTo, HighlightView, NoticeView, ServerMsg};

/// 채팅 한 통의 최대 글자 수.
pub const MAX_CHAT_CHARS: usize = 1000;
/// 이 시간 동안 한 사람이 보낼 수 있는 채팅 수.
const CHAT_WINDOW: Duration = Duration::from_secs(10);
const CHAT_PER_WINDOW: usize = 10;
/// 입장할 때 보내는 채팅 수 / 호스트 화면에 보이는 채팅 수.
const CHAT_IN_WELCOME: usize = 200;
const CHAT_IN_SNAPSHOT: usize = 500;
/// 공지·강조 메모의 최대 글자 수.
pub const MAX_NOTICE_CHARS: usize = 2000;
const MAX_NOTE_CHARS: usize = 500;
/// 강조 하나가 덮을 수 있는 칸 수(보일 사람을 정할 때 칸마다 담당자를 본다).
const MAX_HIGHLIGHT_CELLS: u64 = 10_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum NoticeTarget {
    All,
    /// 필수 입력을 다 하지 않은 사람.
    Incomplete,
    People {
        names: Vec<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum HighlightAudience {
    /// 그 칸을 맡은 사람(행 담당자 + 범위 배정). 담당자 없는 칸(누구나 입력)이 섞여 있으면 모두.
    Owners,
    All,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Notice {
    id: u32,
    at_ms: i64,
    text: String,
    important: bool,
    target: NoticeTarget,
    recipients: Vec<String>,
    read: HashMap<String, i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Highlight {
    id: u32,
    at_ms: i64,
    range: CellRange,
    note: String,
    audience: HighlightAudience,
    recipients: Vec<String>,
    acked: HashMap<String, i64>,
    active: bool,
}

#[derive(Default)]
pub(super) struct Comms {
    notices: Vec<Notice>,
    highlights: Vec<Highlight>,
    chat: Vec<ChatMessage>,
    next_id: u32,
    /// 바뀔 때마다 늘어난다(호스트 화면에 보낼지 판단).
    pub(super) rev: u64,
    chat_times: HashMap<String, VecDeque<Instant>>,
}

/// 기록 파일에 남기는 소통 상태(7단계 복구).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CommsData {
    notices: Vec<Notice>,
    highlights: Vec<Highlight>,
    chat: Vec<ChatMessage>,
    next_id: u32,
}

impl Comms {
    fn id(&mut self) -> u32 {
        self.next_id += 1;
        self.next_id
    }

    pub(super) fn to_data(&self) -> CommsData {
        CommsData {
            notices: self.notices.clone(),
            highlights: self.highlights.clone(),
            chat: self.chat.clone(),
            next_id: self.next_id,
        }
    }

    pub(super) fn from_data(d: CommsData) -> Self {
        Self { notices: d.notices, highlights: d.highlights, chat: d.chat, next_id: d.next_id, ..Default::default() }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct NoticeSummary {
    pub id: u32,
    #[ts(type = "number")]
    pub at_ms: i64,
    pub text: String,
    pub important: bool,
    pub target: NoticeTarget,
    pub recipients: Vec<String>,
    /// 확인한 사람.
    pub read: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HighlightSummary {
    pub id: u32,
    #[ts(type = "number")]
    pub at_ms: i64,
    pub range: CellRange,
    pub note: String,
    pub audience: HighlightAudience,
    pub recipients: Vec<String>,
    /// 확인한 사람과 시각.
    #[ts(type = "Array<[string, number]>")]
    pub acked: Vec<(String, i64)>,
    pub active: bool,
}

/// 호스트 1i 화면: 보낸 공지·강조, 모든 채팅.
#[derive(Debug, Clone, Default, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CommsSnapshot {
    pub notices: Vec<NoticeSummary>,
    pub highlights: Vec<HighlightSummary>,
    pub chat: Vec<ChatMessage>,
}

impl State {
    /// 호스트 이름(공지·강조·채팅·접속자 목록에 보인다): 호스트 PC의 IP가 IP 명단에 있으면 그 이름, 없으면 요청자,
    /// 그것도 없으면 '호스트'.
    pub(super) fn host_name(&self) -> String {
        if let Some(name) = self.host_ips.iter().find_map(|ip| self.ip_names.get(*ip)) {
            return name.to_string();
        }
        let r = self.config.requester.trim();
        if r.is_empty() { "호스트".into() } else { r.to_string() }
    }

    fn conn_of(&self, name: &str) -> Option<ConnId> {
        self.people.get(name).and_then(|p| p.conn)
    }

    fn notice_view(&self, n: &Notice, name: &str) -> NoticeView {
        NoticeView {
            id: n.id,
            at_ms: n.at_ms,
            from: self.host_name(),
            text: n.text.clone(),
            important: n.important,
            read: n.read.contains_key(name),
        }
    }

    fn highlight_view(&self, h: &Highlight, name: &str) -> HighlightView {
        HighlightView {
            id: h.id,
            at_ms: h.at_ms,
            from: self.host_name(),
            range: h.range,
            note: h.note.clone(),
            acked: h.acked.contains_key(name),
        }
    }

    // ------------------------------------------------------------ 호스트

    pub(super) fn send_notice(&mut self, text: String, important: bool, target: NoticeTarget) {
        let text: String = text.trim().chars().take(MAX_NOTICE_CHARS).collect();
        if text.is_empty() {
            return;
        }
        let roster: Vec<String> = self.access.roster().to_vec();
        let recipients: Vec<String> = match &target {
            NoticeTarget::All => roster,
            NoticeTarget::Incomplete => roster
                .into_iter()
                .enumerate()
                .filter(|(i, _)| self.progress.by_person.get(*i).is_some_and(|p| p.filled < p.required))
                .map(|(_, n)| n)
                .collect(),
            NoticeTarget::People { names } => {
                let wanted: HashSet<&str> = names.iter().map(|n| n.trim()).collect();
                roster.into_iter().filter(|n| wanted.contains(n.as_str())).collect()
            }
        };
        let id = self.comms.id();
        let notice = Notice { id, at_ms: now_ms(), text, important, target, recipients, read: HashMap::new() };
        for name in &notice.recipients {
            if let Some(conn) = self.conn_of(name) {
                self.send(conn, ServerMsg::Notice(self.notice_view(&notice, name)));
            }
        }
        self.comms.notices.push(notice);
        self.comms.rev += 1;
    }

    pub(super) fn add_highlight(&mut self, range: CellRange, note: String, audience: HighlightAudience) {
        let Some(range) = range.intersect(&self.model.bounds()) else { return };
        let roster = self.access.roster().to_vec();
        let mut people: HashSet<usize> = HashSet::new();
        match audience {
            HighlightAudience::All => people.extend(0..roster.len()),
            HighlightAudience::Owners => {
                if range.len() <= MAX_HIGHLIGHT_CELLS {
                    for at in range.cells() {
                        // 담당자가 없는 칸은 누구나 입력하므로 모두에게 보인다.
                        if self.access.is_open(at) {
                            people.extend(0..roster.len());
                            break;
                        }
                        people.extend(self.access.owners(at));
                    }
                }
            }
        }
        // '남의 행 숨김'이면 그 행이 보이는 사람에게만.
        let recipients: Vec<String> = roster
            .iter()
            .enumerate()
            .filter(|(i, _)| people.contains(i) && (range.row..=range.last_row()).any(|r| self.row_visible(*i, r)))
            .map(|(_, n)| n.clone())
            .collect();
        let id = self.comms.id();
        let h = Highlight {
            id,
            at_ms: now_ms(),
            range,
            note: note.trim().chars().take(MAX_NOTE_CHARS).collect(),
            audience,
            recipients,
            acked: HashMap::new(),
            active: true,
        };
        for name in &h.recipients {
            if let Some(conn) = self.conn_of(name) {
                self.send(conn, ServerMsg::Highlight(self.highlight_view(&h, name)));
            }
        }
        self.comms.highlights.push(h);
        self.comms.rev += 1;
    }

    pub(super) fn clear_highlight(&mut self, id: u32) {
        let Some(i) = self.comms.highlights.iter().position(|h| h.id == id && h.active) else { return };
        self.comms.highlights[i].active = false;
        let recipients = self.comms.highlights[i].recipients.clone();
        for name in recipients {
            if let Some(conn) = self.conn_of(&name) {
                self.send(conn, ServerMsg::HighlightCleared { id });
            }
        }
        self.comms.rev += 1;
    }

    pub(super) fn host_chat(&mut self, to: ChatTo, text: String) {
        let to = match to {
            ChatTo::Host => return,
            other => other,
        };
        let from = self.host_name();
        self.post_chat(from, true, to, text);
    }

    // ------------------------------------------------------------ 참여자

    pub(super) fn ack_notice(&mut self, conn: ConnId, id: u32) {
        let Some((name, _)) = self.person_of(conn) else { return };
        if let Some(n) = self.comms.notices.iter_mut().find(|n| n.id == id && n.recipients.contains(&name))
            && !n.read.contains_key(&name)
        {
            n.read.insert(name, now_ms());
            self.comms.rev += 1;
        }
    }

    pub(super) fn ack_highlight(&mut self, conn: ConnId, id: u32) {
        let Some((name, _)) = self.person_of(conn) else { return };
        if let Some(h) = self.comms.highlights.iter_mut().find(|h| h.id == id && h.recipients.contains(&name))
            && !h.acked.contains_key(&name)
        {
            h.acked.insert(name, now_ms());
            self.comms.rev += 1;
        }
    }

    pub(super) fn chat(&mut self, conn: ConnId, to: ChatTo, text: String) {
        let Some((name, _)) = self.person_of(conn) else { return };
        if matches!(to, ChatTo::Person { .. }) {
            return self.send(conn, ServerMsg::Error { message: "참여자끼리 1:1 채팅은 할 수 없습니다".into() });
        }
        let times = self.comms.chat_times.entry(name.clone()).or_default();
        let now = Instant::now();
        while times.front().is_some_and(|t| now.duration_since(*t) > CHAT_WINDOW) {
            times.pop_front();
        }
        if times.len() >= CHAT_PER_WINDOW {
            return self.send(
                conn,
                ServerMsg::Error {
                    message: "채팅을 너무 빨리 보내고 있습니다. 잠시 뒤에 보내 주세요".into()
                },
            );
        }
        times.push_back(now);
        self.touch(conn);
        self.post_chat(name, false, to, text);
    }

    fn post_chat(&mut self, from: String, host: bool, to: ChatTo, text: String) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        let text: String = text.chars().take(MAX_CHAT_CHARS).collect();
        let id = self.comms.id();
        let msg = ChatMessage { id, at_ms: now_ms(), from, host, to, text };
        for (conn, p) in self.attached() {
            let name = &self.access.roster()[p];
            if chat_visible(&msg, name) {
                self.send(conn, ServerMsg::Chat(msg.clone()));
            }
        }
        self.comms.chat.push(msg);
        self.comms.rev += 1;
    }

    // ------------------------------------------------------------ 보기

    pub(super) fn notices_for(&self, name: &str) -> Vec<NoticeView> {
        self.comms
            .notices
            .iter()
            .filter(|n| n.recipients.iter().any(|r| r == name))
            .map(|n| self.notice_view(n, name))
            .collect()
    }

    pub(super) fn highlights_for(&self, name: &str) -> Vec<HighlightView> {
        self.comms
            .highlights
            .iter()
            .filter(|h| h.active && h.recipients.iter().any(|r| r == name))
            .map(|h| self.highlight_view(h, name))
            .collect()
    }

    pub(super) fn chat_for(&self, name: &str) -> Vec<ChatMessage> {
        let mine: Vec<&ChatMessage> = self.comms.chat.iter().filter(|m| chat_visible(m, name)).collect();
        mine[mine.len().saturating_sub(CHAT_IN_WELCOME)..].iter().map(|m| (*m).clone()).collect()
    }

    pub(super) fn comms_snapshot(&self) -> CommsSnapshot {
        let c = &self.comms;
        CommsSnapshot {
            notices: c
                .notices
                .iter()
                .map(|n| NoticeSummary {
                    id: n.id,
                    at_ms: n.at_ms,
                    text: n.text.clone(),
                    important: n.important,
                    target: n.target.clone(),
                    recipients: n.recipients.clone(),
                    read: n.recipients.iter().filter(|r| n.read.contains_key(*r)).cloned().collect(),
                })
                .collect(),
            highlights: c
                .highlights
                .iter()
                .map(|h| {
                    let mut acked: Vec<(String, i64)> = h.acked.iter().map(|(k, v)| (k.clone(), *v)).collect();
                    acked.sort_by_key(|(_, t)| *t);
                    HighlightSummary {
                        id: h.id,
                        at_ms: h.at_ms,
                        range: h.range,
                        note: h.note.clone(),
                        audience: h.audience,
                        recipients: h.recipients.clone(),
                        acked,
                        active: h.active,
                    }
                })
                .collect(),
            chat: c.chat[c.chat.len().saturating_sub(CHAT_IN_SNAPSHOT)..].to_vec(),
        }
    }
}

/// 그 사람에게 보이는 채팅: 전체, 내가 호스트에게 보낸 것, 호스트가 나에게 보낸 것.
fn chat_visible(m: &ChatMessage, name: &str) -> bool {
    match &m.to {
        ChatTo::All => true,
        ChatTo::Host => m.from == name,
        ChatTo::Person { name: to } => to == name,
    }
}
