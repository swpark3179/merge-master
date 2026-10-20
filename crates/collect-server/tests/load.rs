//! 7단계 부하 시험: 1,000행 × 10열, 30명이 동시에 입력한다(실제 WebSocket, 같은 PC).
//!
//! `pnpm load` (= `cargo test -p collect-server --release --test load -- --ignored --nocapture`)
//!
//! 사람마다 자기 행에 초당 약 2.5칸씩 입력하고 커서를 초당 5번 움직인다(30명 → 초당 입력 75칸, 알림 약 2,200통).
//! 잰다: 반영 지연(보낸 사람이 보낸 뒤 다른 사람 화면에 그 값이 도착할 때까지), 확인 지연(`applied`가 돌아올 때까지).
//! 중간에 한 사람이 붙여넣기(자기 행 전부 × 8열)를 한 번 한다. 목표: 반영 지연 p95 < 100ms.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use collect_core::config::{CollectConfig, ColumnRole, InputFormat};
use collect_core::{Cell, CellRange, CellRef, CellStyle, CellValue, SheetModel};
use collect_server::protocol::{CellEdit, ClientMsg, PROTOCOL_VERSION, ServerMsg};
use collect_server::{Hub, ServerConfig, SessionHandle, SessionInit, start};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

const PEOPLE: usize = 30;
const DATA_ROWS: u32 = 1000;
const COLS: u32 = 10;
const HEADER: u32 = 2;
const DURATION: Duration = Duration::from_secs(20);
const EDIT_EVERY: Duration = Duration::from_millis(400);
const CURSOR_EVERY: Duration = Duration::from_millis(200);

fn names() -> Vec<String> {
    let family = ["김", "이", "박", "최", "정", "강", "조", "윤", "장", "임"];
    let given = ["민준", "서연", "하준"];
    family.iter().flat_map(|f| given.iter().map(move |g| format!("{f}{g}"))).take(PEOPLE).collect()
}

/// A열 담당자(행마다 돌아가며), B열 항목, C~J열 숫자 입력.
fn sheet(people: &[String]) -> (SheetModel, CollectConfig) {
    let rows = HEADER + DATA_ROWS;
    let mut cells = vec![Cell::default(); (rows * COLS) as usize];
    let labels = ["담당자", "항목", "1월", "2월", "3월", "4월", "5월", "6월", "7월", "8월"];
    for (c, l) in labels.iter().enumerate() {
        cells[c].v = CellValue::Text("부하 시험".into());
        cells[COLS as usize + c].v = CellValue::Text((*l).into());
    }
    for r in HEADER..rows {
        let i = (r * COLS) as usize;
        cells[i].v = CellValue::Text(people[(r - HEADER) as usize % people.len()].clone());
        cells[i + 1].v = CellValue::Text(format!("항목 {r}"));
    }
    let model = SheetModel {
        workbook: "load.xlsx".into(),
        sheet: "부하".into(),
        origin: CellRef::new(0, 0),
        rows,
        cols: COLS,
        cells,
        styles: vec![CellStyle::default()],
        merges: vec![CellRange::parse("A1:J1").unwrap()],
        col_widths: vec![60.0; COLS as usize],
        row_heights: vec![15.0; rows as usize],
        header_rows: HEADER,
        frozen_rows: HEADER,
        frozen_cols: 2,
        validations: vec![],
        hidden_rows: vec![],
        hidden_cols: vec![],
        unhidden_rows: Vec::new(),
        unhidden_cols: Vec::new(),
        deleted_rows: Vec::new(),
        added_rows: Vec::new(),
        cancelled_rows: Vec::new(),
        layout: 0,
    };
    let mut config = CollectConfig::suggest(&model);
    config.assignment.key_col = Some(0);
    for (c, rule) in config.columns.iter_mut().enumerate() {
        if c >= 2 {
            rule.role = ColumnRole::Input;
            rule.format = InputFormat::Number { unit: String::new() };
        } else {
            rule.role = ColumnRole::Locked;
        }
    }
    (model, config)
}

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

fn text(msg: &ClientMsg) -> Message {
    Message::Text(serde_json::to_string(msg).unwrap().into())
}

#[derive(Default)]
struct Stats {
    /// 값 → 보낸 시각.
    sent: HashMap<u64, Instant>,
    broadcast_ms: Vec<f64>,
    ack_ms: Vec<f64>,
    received: u64,
    /// 칸 → 마지막으로 보낸 값.
    last: HashMap<(u32, u32), f64>,
    paste_ms: Option<f64>,
}

fn pct(v: &mut [f64], p: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    v[((v.len() - 1) as f64 * p).round() as usize]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "부하 시험(수십 초)"]
async fn thirty_people_editing_at_once() {
    let people = names();
    let (model, config) = sheet(&people);
    let session = SessionHandle::spawn(SessionInit::new(model, config));
    let cfg = ServerConfig { bind: IpAddr::V4(Ipv4Addr::LOCALHOST), port: 0, dev_admin: false };
    let hub = Hub::new();
    hub.publish("load", session.clone()).unwrap();
    let server = start(cfg, hub).await.unwrap();
    let url = format!("ws://{}/ws?s=load", server.local_addr);
    let stats = Arc::new(Mutex::new(Stats::default()));
    let started = Instant::now();

    let mut tasks = Vec::new();
    for (k, name) in people.iter().enumerate() {
        let (url, name, stats, host) = (url.clone(), name.clone(), stats.clone(), session.clone());
        tasks.push(tokio::spawn(async move {
            let (ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
            let (mut tx, mut rx): (futures_util::stream::SplitSink<Ws, Message>, _) = ws.split();
            tx.send(text(&ClientMsg::Join { name: name.clone(), token: None, protocol: PROTOCOL_VERSION }))
                .await
                .unwrap();
            // 입장(모두 같은 PC라 IP 명단 밖: 승인 대기 → 호스트 승인): 내 행
            let mine: Vec<u32> = loop {
                let Some(Ok(Message::Text(t))) = rx.next().await else { panic!("연결 끊김") };
                let msg = serde_json::from_str::<ServerMsg>(&t);
                if let Ok(ServerMsg::Pending { .. }) = msg {
                    host.approve(&name, None, "").await.unwrap();
                }
                if let Ok(ServerMsg::Welcome(w)) = msg {
                    break w.sheets[0]
                        .row_owner
                        .iter()
                        .enumerate()
                        .filter(|(_, o)| **o == w.color)
                        .map(|(r, _)| r as u32)
                        .collect();
                }
            };
            let reader_stats = stats.clone();
            let reader = tokio::spawn(async move {
                while let Some(Ok(msg)) = rx.next().await {
                    let Message::Text(t) = msg else { continue };
                    let now = Instant::now();
                    let Ok(m) = serde_json::from_str::<ServerMsg>(&t) else { continue };
                    let mut s = reader_stats.lock().unwrap();
                    s.received += 1;
                    match m {
                        ServerMsg::Cells { cells, .. } => {
                            for c in cells {
                                if let CellValue::Number(v) = c.value
                                    && let Some(t0) = s.sent.get(&(v as u64)).copied()
                                {
                                    s.broadcast_ms.push(now.duration_since(t0).as_secs_f64() * 1000.0);
                                }
                            }
                        }
                        ServerMsg::Applied { cells, .. } => {
                            if let Some(CellValue::Number(v)) = cells.first().map(|c| c.value.clone())
                                && let Some(t0) = s.sent.get(&(v as u64)).copied()
                            {
                                let ms = now.duration_since(t0).as_secs_f64() * 1000.0;
                                if cells.len() > 100 {
                                    s.paste_ms = Some(ms);
                                } else {
                                    s.ack_ms.push(ms);
                                }
                            }
                        }
                        _ => {}
                    }
                }
            });

            let mut seq: u64 = 0;
            let mut next_edit = Instant::now() + EDIT_EVERY.mul_f64((k as f64) / PEOPLE as f64);
            let mut next_cursor = Instant::now();
            let mut pasted = false;
            while started.elapsed() < DURATION {
                let now = Instant::now();
                if now >= next_cursor {
                    let at = CellRef::new(mine[(seq as usize) % mine.len()], 2 + (seq % 8) as u32);
                    tx.send(text(&ClientMsg::Cursor { sheet: 0, at: Some(at) })).await.unwrap();
                    next_cursor = now + CURSOR_EVERY;
                }
                if now >= next_edit {
                    seq += 1;
                    let value = (k as u64 + 1) * 10_000_000 + seq;
                    let row = mine[(seq as usize * 7) % mine.len()];
                    let col = 2 + (seq % 8) as u32;
                    let cells = if k == 0 && !pasted && started.elapsed() > DURATION / 2 {
                        // 붙여넣기: 내 행 전부 × 8열을 한 번에
                        pasted = true;
                        let mut v = Vec::new();
                        for (i, &r) in mine.iter().enumerate() {
                            for c in 2..COLS {
                                v.push(CellEdit {
                                    row: r,
                                    col: c,
                                    value: CellValue::Number((value + (i as u64) * 100 + c as u64) as f64),
                                    base: 0,
                                });
                            }
                        }
                        v
                    } else {
                        vec![CellEdit { row, col, value: CellValue::Number(value as f64), base: 0 }]
                    };
                    {
                        let mut s = stats.lock().unwrap();
                        let t0 = Instant::now();
                        for c in &cells {
                            if let CellValue::Number(v) = c.value {
                                s.sent.insert(v as u64, t0);
                                s.last.insert((c.row, c.col), v);
                            }
                        }
                    }
                    let msg = ClientMsg::SetCells {
                        sheet: 0,
                        batch: format!("{k}-{seq}"),
                        cells,
                        origin: None,
                        layout: None,
                    };
                    tx.send(text(&msg)).await.unwrap();
                    next_edit += EDIT_EVERY;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            tokio::time::sleep(Duration::from_millis(500)).await; // 마지막 알림을 받는다
            reader.abort();
            seq
        }));
    }
    let mut edits = 0;
    for t in tasks {
        edits += t.await.unwrap();
    }
    let elapsed = started.elapsed().as_secs_f64();

    // 서버 값이 마지막으로 보낸 값과 같아야 한다.
    let last = stats.lock().unwrap().last.clone();
    let mismatched = session
        .with(move |s| {
            last.iter()
                .filter(|((r, c), v)| {
                    s.sheets[0].model.cell(CellRef::new(*r, *c)).map(|x| x.v.clone()) != Some(CellValue::Number(**v))
                })
                .count()
        })
        .await
        .unwrap();
    server.stop().await;

    let mut s = stats.lock().unwrap();
    let samples = s.broadcast_ms.len();
    let (b50, b95, b99) =
        (pct(&mut s.broadcast_ms, 0.5), pct(&mut s.broadcast_ms, 0.95), pct(&mut s.broadcast_ms, 0.99));
    let bmax = s.broadcast_ms.last().copied().unwrap_or(f64::NAN);
    let (a50, a95) = (pct(&mut s.ack_ms, 0.5), pct(&mut s.ack_ms, 0.95));
    println!("\n=== 부하 시험: {PEOPLE}명 · {DATA_ROWS}행 × {COLS}열 · {elapsed:.1}초 ===");
    println!(
        "입력 묶음 {edits}개({:.0}/초) · 받은 메시지 {}개({:.0}/초)",
        edits as f64 / elapsed,
        s.received,
        s.received as f64 / elapsed
    );
    println!(
        "반영 지연(다른 사람 화면까지) {samples}건: p50 {b50:.1}ms · p95 {b95:.1}ms · p99 {b99:.1}ms · 최대 {bmax:.1}ms"
    );
    println!("확인 지연(applied): p50 {a50:.1}ms · p95 {a95:.1}ms");
    println!("붙여넣기({}칸) 확인까지: {:.1}ms", 34 * 8, s.paste_ms.unwrap_or(f64::NAN));
    println!("서버 값 ≠ 마지막 입력: {mismatched}칸");
    assert_eq!(mismatched, 0, "모든 칸이 마지막 입력값이어야 한다");
    assert!(samples > 10_000, "반영 지연 표본이 너무 적다: {samples}");
    assert!(b95 < 100.0, "반영 지연 p95 {b95:.1}ms ≥ 100ms");
}
