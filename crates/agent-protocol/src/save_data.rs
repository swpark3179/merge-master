//! 취합 완료 데이터([`SaveData`]): 엑셀로 저장하는 데 필요한 것(시트마다 기준·지금 시트, 변경 기록, 함께 넣기)을 파일
//! 하나로. 에이전트는 마감 때 Excel로 저장하기 **전에** 이것부터 서비스 폴더에 남긴다 — 저장이 실패하거나 도중에 꺼져도
//! 취합한 값이 남고, 관리자 앱이 접속하면 받아 그 PC의 Excel로 저장한다. 서비스 중에는 관리자 앱이 '지금 상태'를 받아
//! 저장할 때도 쓴다.

use std::io::Read;

use collect_core::export::{ExportOptions, ExportPlan};
use collect_server::SaveParts;
use serde::{Deserialize, Serialize};

const MAGIC: &[u8; 8] = b"MMSAVE\0\0";
const FORMAT: u32 = 1;
/// 풀었을 때 이보다 크면 거절한다(손상·잘못 보낸 파일).
const MAX_DECODED: u64 = 2 * crate::MAX_PACKAGE as u64;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveData {
    /// 서비스 id.
    pub id: String,
    pub title: String,
    /// 원본 통합문서 파일 이름.
    pub workbook_name: String,
    /// 마감 때 저장할 파일 이름(관리자 PC에서 저장할 때 기본 이름).
    pub file_name: String,
    /// Excel 시트 이름(세션 안 순번 순서).
    pub sheets: Vec<String>,
    pub export: ExportOptions,
    /// 뜬 PC의 UTC 차이(분) — '변경 이력' 시트의 시각.
    pub utc_offset_minutes: i32,
    /// 뜬 시각(에이전트 PC, ms).
    pub frozen_at_ms: i64,
    /// 서비스 중에 뜬 '지금 상태'(마감 때 뜬 것이 아니다).
    #[serde(default)]
    pub live: bool,
    pub parts: SaveParts,
}

impl SaveData {
    /// 저장 계획(마감 저장과 같다 — 충돌 칸은 취합 값).
    pub fn plan(&self) -> Vec<(String, ExportPlan)> {
        self.parts.plan(&self.sheets, self.export, self.utc_offset_minutes, &[])
    }

    /// `MMSAVE\0\0` · 형식(u32, little endian) · zstd(JSON).
    pub fn encode(&self) -> Vec<u8> {
        let json = serde_json::to_vec(self).expect("직렬화");
        let mut out = Vec::with_capacity(12 + json.len() / 4);
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&FORMAT.to_le_bytes());
        out.extend_from_slice(&zstd::encode_all(json.as_slice(), 3).expect("zstd 압축"));
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<SaveData, String> {
        let bad = || "취합 데이터 파일이 손상되었습니다".to_string();
        if bytes.len() < 12 || &bytes[..8] != MAGIC {
            return Err(bad());
        }
        let format = u32::from_le_bytes(bytes[8..12].try_into().map_err(|_| bad())?);
        if format != FORMAT {
            return Err(format!(
                "알 수 없는 취합 데이터 형식({format})입니다. 호스트 앱과 에이전트를 같은 버전으로 맞춰 주세요"
            ));
        }
        let mut json = Vec::new();
        zstd::stream::read::Decoder::new(&bytes[12..])
            .map_err(|_| bad())?
            .take(MAX_DECODED + 1)
            .read_to_end(&mut json)
            .map_err(|_| bad())?;
        if json.len() as u64 > MAX_DECODED {
            return Err("취합 데이터 파일이 너무 큽니다".into());
        }
        serde_json::from_slice(&json).map_err(|e| format!("취합 데이터 파일을 읽을 수 없습니다: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use collect_core::{CellRef, CellValue, SheetModel};
    use collect_server::SaveSheet;

    use super::*;

    fn data() -> SaveData {
        let baseline: SheetModel =
            serde_json::from_slice(include_bytes!("../../../fixtures/budget.sheet.json")).unwrap();
        let mut current = baseline.clone();
        let at = CellRef { row: baseline.header_rows, col: 0 };
        current.cell_mut(at).unwrap().v = CellValue::Text("바뀐 값".into());
        let config = collect_core::config::CollectConfig::suggest(&baseline);
        let sheet = SaveSheet { baseline, current, config, labels: vec!["A".into()] };
        SaveData {
            id: "s1".into(),
            title: "예산".into(),
            workbook_name: "budget.xlsx".into(),
            file_name: "budget_취합_20261030.xlsx".into(),
            sheets: vec!["예산".into()],
            export: ExportOptions { mark: Some(0xFFFF00), styles: true, history: true, cancelled: false },
            utc_offset_minutes: 540,
            frozen_at_ms: 1_790_000_000_000,
            live: false,
            parts: SaveParts { sheets: vec![sheet], history: vec![] },
        }
    }

    #[test]
    fn round_trip_keeps_the_plan() {
        let d = data();
        let back = SaveData::decode(&d.encode()).unwrap();
        assert_eq!(back, d);
        assert_eq!(back.plan(), d.plan());
        assert_eq!(d.plan()[0].1.cells.len(), 1, "바뀐 칸 하나");
    }

    #[test]
    fn broken_files_are_rejected() {
        let bytes = data().encode();
        assert!(SaveData::decode(&bytes[..bytes.len() - 3]).is_err(), "잘림");
        assert!(SaveData::decode(b"MMAGENT\0\x01\0\0\0").is_err());
        assert!(SaveData::decode(&[]).is_err());
        let mut other = bytes.clone();
        other[8] = 7;
        assert!(SaveData::decode(&other).unwrap_err().contains("형식"));
    }
}
