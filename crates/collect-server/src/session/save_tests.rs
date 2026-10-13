//! 5단계 저장을 돕는 세션 기능: 기준 시트, 호스트가 바꾼 값, 취합 마감.

use super::tests::{Client, connect, fixture, host, join};
use super::*;
use crate::protocol::CellEdit;

const F: u32 = 5;

fn setup(names: &[&str]) -> (State, Vec<Client>) {
    let mut st = State::new(fixture());
    let mut clients = Vec::new();
    for (i, name) in names.iter().enumerate() {
        let c = connect(&mut st, i as u64 + 1);
        join(&mut st, &c, name, None);
        clients.push(c);
    }
    for c in &mut clients {
        c.take();
    }
    (st, clients)
}

fn first_row(st: &State, name: &str) -> u32 {
    st.rt[0].access.rows_of(st.rt[0].access.person(name).unwrap())[0]
}

#[tokio::test]
async fn baseline_stays_at_import_until_host_moves_it() {
    let (mut st, c) = setup(&["이서연"]);
    let r = first_row(&st, "이서연");
    let msg = ClientMsg::SetCells {
        sheet: 0,
        batch: "b".into(),
        cells: vec![CellEdit { row: r, col: F, value: CellValue::Number(9800.0), base: 0 }],
        origin: None,
        layout: None,
    };
    st.handle(Command::Client { conn: c[0].conn, msg });
    let at = CellRef::new(r, F);
    assert_eq!(st.sheets[0].baseline.cell(at).unwrap().v, CellValue::Empty);
    assert_eq!(st.sheets[0].model.cell(at).unwrap().v, CellValue::Number(9800.0));
    // 원본에 저장한 뒤: 쓴 값이 새 기준이 된다.
    host(&mut st, move |s| {
        let v = s.sheets[0].model.cell(at).unwrap().v.clone();
        s.sheets[0].baseline.cell_mut(at).unwrap().v = v;
    });
    assert_eq!(st.sheets[0].baseline.cell(at).unwrap().v, CellValue::Number(9800.0));
    // 다시 불러와 확정하면 기준도 새 시트.
    host(&mut st, |s| {
        let (mut model, config) = (s.sheets[0].model.clone(), s.sheets[0].config.clone());
        model.cells[0].v = CellValue::Text("새 시트".into());
        s.replace(vec![(model, config)]);
    });
    assert_eq!(st.sheets[0].baseline.cells[0].v, CellValue::Text("새 시트".into()));
}

#[tokio::test]
async fn host_values_are_logged_counted_and_broadcast() {
    let (mut st, mut c) = setup(&["이서연", "최도윤"]);
    let r = first_row(&st, "이서연");
    let before = st.rt[0].progress.filled;
    host(&mut st, move |s| {
        s.host_set(
            "Excel(원본)",
            collect_core::history::BatchOrigin::Excel,
            0,
            vec![
                (CellRef::new(r, F), CellValue::Number(1234.0)),
                (CellRef::new(r, 6), CellValue::Number(9.0)), // 수식 칸은 건너뛴다
            ],
        );
    });
    assert_eq!(st.sheets[0].model.cell(CellRef::new(r, F)).unwrap().v, CellValue::Number(1234.0));
    assert_eq!(st.rt[0].progress.filled, before + 1);
    assert_eq!(st.log.len(), 1);
    assert_eq!(st.log[0].user, "Excel(원본)");
    for client in &mut c {
        let seen = client.take();
        assert!(
            seen.iter()
                .any(|m| matches!(m, ServerMsg::Cells { by, cells, .. } if by == "Excel(원본)" && cells.len() == 1)),
            "{seen:?}"
        );
    }
    assert!(st.rt[0].writer.values().all(|w| w == "Excel(원본)"));
}

#[tokio::test]
async fn host_can_close_and_reopen_collection() {
    let (mut st, mut c) = setup(&["이서연"]);
    let r = first_row(&st, "이서연");
    host(&mut st, |s| s.set_closed(true));
    assert!(st.read_only && st.closed && st.snapshot().closed);
    let seen = c[0].take();
    assert!(
        seen.iter()
            .any(|m| matches!(m, ServerMsg::ReadOnly { read_only: true, reason: Some(r) } if r.contains("호스트")))
    );
    let msg = ClientMsg::SetCells {
        sheet: 0,
        batch: "late".into(),
        cells: vec![CellEdit { row: r, col: F, value: CellValue::Number(1.0), base: 0 }],
        origin: None,
        layout: None,
    };
    st.handle(Command::Client { conn: c[0].conn, msg });
    let rejected = c[0].take().into_iter().find_map(|m| match m {
        ServerMsg::Applied { rejected, .. } => Some(rejected),
        _ => None,
    });
    assert_eq!(rejected.map(|r| r.len()), Some(1));
    assert!(st.info().closed && st.info().read_only);

    // 주기 점검이 마감 시각만 보고 다시 열어 버리지 않는다.
    st.handle(Command::Tick);
    assert!(st.read_only);

    host(&mut st, |s| {
        assert!(s.closed());
        s.set_closed(false);
    });
    assert!(!st.read_only);
    assert!(c[0].take().iter().any(|m| matches!(m, ServerMsg::ReadOnly { read_only: false, reason: None })));
}

/// 호스트가 칸을 고쳐 명단(행 배정 기준 열)이 바뀌면, 고친 뒤 `changed()`로 다시 계산해야 새 이름이 명단에 들어온다
/// (같은 작업 안에서는 `changed`가 값 반영보다 먼저 돈다 — 호스트 앱은 두 번 나눠 보낸다).
#[tokio::test]
async fn host_edit_of_key_column_updates_roster_after_changed() {
    let (mut st, _c) = setup(&[]);
    let key = st.sheets[0].config.assignment.key_col.unwrap();
    let r = first_row(&st, "이서연");
    let head = st.sheets[0].model.merges.iter().find(|m| m.contains(CellRef::new(r, key))).map_or(r, |m| m.row);
    host(&mut st, move |s| {
        let cells = vec![(CellRef::new(head, key), CellValue::Text("윤새봄".into()))];
        s.host_set("호스트", collect_core::history::BatchOrigin::Edit, 0, cells);
    });
    assert_eq!(st.log.last().map(|e| e.user.as_str()), Some("호스트"));
    assert!(st.rt[0].access.person("윤새봄").is_none()); // 아직 옛 명단
    host(&mut st, |s| s.changed());
    let p = st.rt[0].access.person("윤새봄").expect("새 이름이 명단에 들어온다");
    assert!(st.rt[0].access.rows_of(p).contains(&r));
}
