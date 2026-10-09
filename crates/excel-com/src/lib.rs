//! Excel COM 접근 계층.
//!
//! 모든 COM 호출은 [`ExcelThread`]가 띄운 전용 STA 스레드에서만 실행된다. 바깥에서는
//! `thread.run(|session| session.list_workbooks())`처럼 작업을 보내고 결과를 받는다.
#![cfg(windows)]

mod calc;
mod dispatch;
mod error;
mod inspect;
mod reader;
mod session;
mod thread;
mod variant;
mod writer;

pub use calc::CalcInfo;
pub use dispatch::{Dispatch, com_calls};
pub use error::{ExcelError, Result};
pub use inspect::{ColumnInfo, SheetInspection, ValidationInfo};
pub use reader::{ImportItems, MAX_CELLS, ReadOptions, ReadProgress, ReadStage, ReadStats};
pub use session::{ExcelStatus, OrphanReport, Session, WorkbookInfo};
pub use thread::ExcelThread;
pub use variant::{Grid, Var};
pub use writer::{HISTORY_SHEET, SaveReport, SourceStatus};
