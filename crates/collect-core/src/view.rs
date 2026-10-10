//! 화면에 보낼 설정 보기: 설정 + 셀 상태 비트 + 명단·행 담당자 + 진행률 + 범례 집계.
//!
//! 셀 상태는 셀당 1바이트([`crate::access::flag`])를 base64로 보낸다(100만 셀 ≈ 1.3MB).

use base64::Engine;
use serde::Serialize;
use ts_rs::TS;

use crate::access::{Access, ProgressStats, Viewer, flag};
use crate::config::{CollectConfig, ColumnRole, OthersRows, suggested_key_col};
use crate::detect::{TableGuess, column_labels};
use crate::sheet::SheetModel;

#[derive(Debug, Clone, Default, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Legend {
    pub input_cols: u32,
    pub locked_cols: u32,
    pub filter_cols: u32,
    pub exceptions: u32,
    pub required_missing: u32,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ConfigView {
    pub config: CollectConfig,
    pub roster: Vec<String>,
    /// 행마다 담당자(명단 인덱스), 없으면 -1.
    pub row_owner: Vec<i32>,
    /// 셀 상태 비트(행 우선, 셀당 1바이트)의 base64.
    pub flags: String,
    /// 이 시점으로 계산했다(없으면 호스트).
    #[ts(optional)]
    pub viewer: Option<String>,
    /// 보는 사람에게 보이지 않는 데이터 행('남의 행 숨김'일 때 남의 행).
    pub hidden_rows: Vec<u32>,
    pub progress: ProgressStats,
    pub legend: Legend,
    /// 열 이름(머리글 기준).
    pub labels: Vec<String>,
    /// 행 배정 기준으로 추천하는 열(담당자 이름 열 후보 1순위).
    #[ts(optional)]
    pub suggested_key_col: Option<u32>,
}

/// 셀 상태 비트 → base64(화면·참여자에게 보낼 형태).
pub fn encode_flags(flags: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(flags)
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("명단에 없는 이름입니다: {0}")]
pub struct UnknownPerson(pub String);

pub fn config_view(
    model: &SheetModel,
    config: &CollectConfig,
    viewer: Option<&str>,
) -> Result<ConfigView, UnknownPerson> {
    let access = Access::compile(model, config);
    let (v, person) = match viewer {
        None => (Viewer::Host, None),
        Some(name) => {
            let p = access.person(name).ok_or_else(|| UnknownPerson(name.to_string()))?;
            (Viewer::Person(p), Some(p))
        }
    };
    let flags = access.viewer_flags(v);
    let hidden_rows = match person {
        Some(p) if config.permissions.others_rows == OthersRows::Hidden => {
            let mine = access.rows_of(p);
            (model.header_rows..model.rows).filter(|r| mine.binary_search(r).is_err()).collect()
        }
        _ => Vec::new(),
    };
    let progress = access.progress(model);
    let legend = Legend {
        input_cols: config.columns.iter().filter(|c| c.role == ColumnRole::Input).count() as u32,
        locked_cols: config.columns.iter().filter(|c| c.role == ColumnRole::Locked).count() as u32,
        filter_cols: config.columns.iter().filter(|c| c.filter).count() as u32,
        exceptions: config.exceptions.len() as u32,
        required_missing: progress.required - progress.filled,
    };
    let labels = column_labels(model, &TableGuess { range: model.bounds(), header_rows: model.header_rows });
    debug_assert!(flags.iter().all(|f| f & flag::MINE == 0 || f & flag::INPUT != 0));
    Ok(ConfigView {
        config: config.clone(),
        roster: access.roster().to_vec(),
        row_owner: access.row_owners().to_vec(),
        flags: encode_flags(&flags),
        viewer: viewer.map(str::to_string),
        hidden_rows,
        progress,
        legend,
        labels,
        suggested_key_col: suggested_key_col(model),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::tests::{assigned, budget};

    fn decode(s: &str) -> Vec<u8> {
        base64::engine::general_purpose::STANDARD.decode(s).unwrap()
    }

    #[test]
    fn host_view() {
        let m = budget();
        let mut cfg = assigned(&m);
        cfg.columns[5].required = true;
        let v = config_view(&m, &cfg, None).unwrap();
        assert_eq!(v.roster, ["이서연", "최도윤", "김민지"]);
        assert_eq!(decode(&v.flags).len(), (m.rows * m.cols) as usize);
        assert_eq!(
            v.legend,
            Legend { input_cols: 3, locked_cols: 6, filter_cols: 3, exceptions: 0, required_missing: 4 }
        );
        assert_eq!(v.labels[5], "신청 금액");
        assert!(v.hidden_rows.is_empty());
    }

    #[test]
    fn participant_view_and_hidden_policy() {
        let m = budget();
        let mut cfg = assigned(&m);
        let v = config_view(&m, &cfg, Some("김민지")).unwrap();
        let flags = decode(&v.flags);
        let mine = |r: u32, c: u32| flags[(r * m.cols + c) as usize] & flag::MINE != 0;
        assert!(mine(6, 5) && !mine(2, 5));
        assert!(v.hidden_rows.is_empty()); // 기본은 읽기 전용으로 보인다

        cfg.permissions.others_rows = OthersRows::Hidden;
        let v = config_view(&m, &cfg, Some("김민지")).unwrap();
        assert_eq!(v.hidden_rows, [2, 3, 4, 5]);
        assert_eq!(config_view(&m, &cfg, Some("없는사람")).unwrap_err(), UnknownPerson("없는사람".into()));
    }
}
