//! Tauri·Excel 없이 참여자 서버만 띄우는 개발용 바이너리.
//!
//! ```text
//! cargo run -p collect-server --bin collect-dev-server -- --fixture fixtures/budget.sheet.json
//!     [--fixture fixtures/staff.sheet.json …] [--port 8080] [--bind 0.0.0.0] [--slug q4] [--title 제목]
//!     [--requester 박지훈] [--required F,H,2:C] [--locked A,B,2:A] [--others-hidden] [--add-rows] [--ip-names people.json]
//!     [--dev-admin] [--session hr --fixture fixtures/staff.sheet.json --title "인력 계획 취합" …]
//! ```
//! `--fixture`를 여러 번 주면 그 순서대로 여러 시트를 함께 취합한다. `--required`·`--locked`의 `2:C`는 둘째 시트의 C열(번호가
//! 없으면 첫 시트). 설정은 시트마다 `CollectConfig::suggest`로 만든 초안(모든 열 입력, 행 배정 기준은 후보 1순위)에 위 옵션을
//! 더한다. `--locked`는 호스트가 1b에서 하듯 그 열을 잠근다.
//!
//! `--session <주소 경로>`는 같은 서버에 업무를 하나 더 연다(여러 업무 동시 진행). 그 뒤에 오는 `--fixture`·`--slug`·`--title`·
//! `--requester`·`--required`·`--locked`·`--others-hidden`·`--add-rows`는 그 업무 것이고, 첫 `--session` 앞은 첫 업무(주소 경로
//! 기본 q4)다.
//! `--port`·`--bind`·`--ip-names`·`--dev-admin`은 서버 공통.
//!
//! `--dev-admin`은 `/__dev/*`(승인·거절·연결 끊기·잠금 해제·설정·수식 결과·공개 멈추기)를 켠다. 업무가 여럿이면 `?s=<주소 경로>`로 고른다.
//! `--ip-names`는 IP별 이름 JSON(`{"192.168.0.11": "이서연"}`): 그 IP에서 열면 이름 없이 바로 들어온다. 없으면 모두 이름 + 승인
//! (`--dev-admin`이면 `POST /__dev/approve`).

use std::net::IpAddr;

use anyhow::Context;
use collect_core::SheetModel;
use collect_core::a1::col_from_letters;
use collect_core::config::{CollectConfig, ColumnRole, ColumnRule, OthersRows, suggested_key_col};
use collect_server::{Hub, IpNames, META_JOB, ServerConfig, SessionHandle, SessionInit, lan_addresses, start};

/// 업무 하나의 옵션.
struct JobSpec {
    slug: String,
    fixtures: Vec<String>,
    title: String,
    requester: String,
    required: Vec<String>,
    locked: Vec<String>,
    others_hidden: bool,
    add_rows: bool,
}

impl JobSpec {
    fn new(slug: &str, title: &str) -> Self {
        JobSpec {
            slug: slug.into(),
            fixtures: Vec::new(),
            title: title.into(),
            requester: "박지훈".into(),
            required: Vec::new(),
            locked: Vec::new(),
            others_hidden: false,
            add_rows: false,
        }
    }

    async fn build(self) -> anyhow::Result<(String, SessionHandle)> {
        anyhow::ensure!(!self.fixtures.is_empty(), "‘{}’ 업무에 --fixture <시트 JSON>이 필요합니다", self.slug);
        let mut sheets = Vec::new();
        for path in &self.fixtures {
            let model: SheetModel = serde_json::from_slice(&std::fs::read(path).with_context(|| path.clone())?)?;
            let mut config = CollectConfig::suggest(&model);
            // 개발 서버는 담당자 열(후보 1순위)로 행을 배정해 둔다(호스트 앱은 1b에서 고른다).
            config.assignment.key_col = suggested_key_col(&model);
            if self.others_hidden {
                config.permissions.others_rows = OthersRows::Hidden;
            }
            config.permissions.add_rows = self.add_rows;
            sheets.push((model, config));
        }
        // 공통 설정(제목·요청자·안내)은 첫 시트에 둔다(세션이 다른 시트에 옮겨 적는다).
        let first = &mut sheets[0].1;
        first.title = self.title;
        first.requester = self.requester;
        first.description = "부서별 담당 비목의 신청 금액과 산출 근거를 입력해 주세요.".into();
        for spec in &self.required {
            column_rule(&mut sheets, spec)?.required = true;
        }
        // 잠그면 필수도 꺼진다(1b와 같이, 필수는 입력 열에만).
        for spec in &self.locked {
            let rule = column_rule(&mut sheets, spec)?;
            rule.role = ColumnRole::Locked;
            rule.required = false;
        }
        let session = SessionHandle::spawn(SessionInit::multi(sheets));
        // 호스트 앱처럼 업무 id를 둔다(참여자가 같은 주소의 다른 업무를 알아본다).
        let id = format!("dev-{}", self.slug);
        session.with(move |s| s.set_meta(META_JOB, &id)).await.map_err(anyhow::Error::msg)?;
        Ok((self.slug, session))
    }
}

/// `F`(첫 시트의 F열)·`2:C`(둘째 시트의 C열)가 가리키는 열 규칙.
fn column_rule<'a>(sheets: &'a mut [(SheetModel, CollectConfig)], spec: &str) -> anyhow::Result<&'a mut ColumnRule> {
    let (sheet, letters) = match spec.split_once(':') {
        Some((n, l)) => (n.trim().parse::<usize>().context("시트 번호가 잘못되었습니다")?, l.trim()),
        None => (1, spec),
    };
    let config = &mut sheets.get_mut(sheet.wrapping_sub(1)).with_context(|| format!("없는 시트: {spec}"))?.1;
    let c = col_from_letters(letters).with_context(|| format!("열 이름이 잘못되었습니다: {letters}"))?;
    config.columns.get_mut(c as usize).with_context(|| format!("없는 열: {spec}"))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into())).init();

    let mut server = ServerConfig { bind: IpAddr::from([0, 0, 0, 0]), port: 8080, dev_admin: false };
    let mut jobs = vec![JobSpec::new("q4", "2026년 부서별 예산 신청")];
    let mut ip_names = None;
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        let job = jobs.last_mut().expect("업무 하나는 늘 있다");
        match flag.as_str() {
            "--dev-admin" => server.dev_admin = true,
            "--others-hidden" => job.others_hidden = true,
            "--add-rows" => job.add_rows = true,
            _ => {
                let value = args.next().with_context(|| format!("{flag} 값이 없습니다"))?;
                match flag.as_str() {
                    "--session" => {
                        let title = format!("{value} 취합");
                        jobs.push(JobSpec::new(&value, &title));
                    }
                    "--fixture" => job.fixtures.push(value),
                    "--port" => server.port = value.parse()?,
                    "--bind" => server.bind = value.parse()?,
                    "--slug" => job.slug = value,
                    "--title" => job.title = value,
                    "--requester" => job.requester = value,
                    "--required" => job.required.extend(value.split(',').map(|s| s.trim().to_string())),
                    "--locked" => job.locked.extend(value.split(',').map(|s| s.trim().to_string())),
                    "--ip-names" => ip_names = Some(value),
                    other => anyhow::bail!("알 수 없는 옵션 {other}"),
                }
            }
        }
    }

    let hub = Hub::new();
    let names = match ip_names {
        Some(path) => {
            let text = std::fs::read_to_string(&path).with_context(|| path.clone())?;
            let names = IpNames::parse_json(&text).map_err(anyhow::Error::msg)?;
            println!("  IP 명단 {}명 ({path})", names.len());
            Some(names)
        }
        None => None,
    };
    let mut slugs = Vec::new();
    for job in jobs {
        let (slug, session) = job.build().await?;
        if let Some(names) = &names {
            session.set_ip_names(names.clone());
        }
        hub.publish(&slug, session).map_err(anyhow::Error::msg)?;
        slugs.push(slug);
    }
    if let Some(names) = names {
        hub.set_ip_names(names);
    }
    let handle = start(server.clone(), hub).await?;
    let port = handle.port();
    for lan in lan_addresses() {
        println!("  {} → http://{}:{port}/ (업무 목록)", lan.name, lan.ip);
    }
    println!("  로컬 → http://127.0.0.1:{port}/ (업무 목록)   (Ctrl+C로 종료)");
    for slug in &slugs {
        println!("         http://127.0.0.1:{port}/{slug}");
    }
    tokio::signal::ctrl_c().await?;
    handle.stop().await;
    Ok(())
}
