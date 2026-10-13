//! 취합 도구의 도메인 모델.
//!
//! - [`a1`]: A1 주소 표기 ↔ 행/열 인덱스 변환
//! - [`sheet`]: Excel에서 불러온 시트를 표현하는 [`SheetModel`](sheet::SheetModel)
//! - [`detect`]: 표 범위·머리글·배정 기준 열 감지(1a)
//! - [`config`]: 취합 설정(1b) — 열 역할·입력 형식·셀 예외·담당자 배정·참여자 권한
//! - [`edit`]: 1b 화면의 설정 편집 동작
//! - [`access`]: 권한 엔진(누가 어느 셀에 입력할 수 있나)과 필수 진행률
//! - [`format`]: 1b 서식 도구 줄
//! - [`view`]: 화면에 보낼 설정 보기
//! - [`project`]: 참여자에게 보낼 시트(남의 행 숨김 반영)
//! - [`validate`]: 참여자 입력 검사·정규화(열 형식)
//! - [`export`]: 엑셀 저장 계획·원본 충돌 확인·저장 요약(5단계)
//! - [`history`]: 셀 변경 기록·히스토리 보기·되돌리기·스냅샷 비교(6단계)
//! - [`structure`]: 표 끝에 행·열 붙이기, 앱에서 붙인 행·열 지우기, 행 취소 표시

pub mod a1;
pub mod access;
pub mod config;
pub mod detect;
pub mod edit;
pub mod export;
pub mod format;
pub mod history;
pub mod project;
pub mod sheet;
pub mod structure;
pub mod validate;
pub mod view;

pub use a1::{CellRange, CellRef, SheetCell};
pub use sheet::*;
