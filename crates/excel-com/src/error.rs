#[derive(Debug, thiserror::Error)]
pub enum ExcelError {
    #[error("Excel이 응답하지 않습니다. Excel에서 편집 중인 셀이나 열린 대화상자가 있으면 닫아 주세요")]
    Busy,
    #[error("Excel 연결이 끊겼습니다(Excel이나 통합문서가 닫힘). '다시 찾기'를 눌러 주세요")]
    Disconnected,
    #[error("읽기를 취소했습니다")]
    Cancelled,
    #[error("범위가 너무 큽니다({cells}셀). {limit}셀 이하로 지정해 주세요")]
    TooLarge { cells: u64, limit: u64 },
    #[error("Excel 호출 실패 `{member}`: {message} (0x{hresult:08X})")]
    Call { member: String, hresult: u32, message: String },
    #[error("예상하지 못한 값 형식: {0}")]
    Type(String),
    #[error("통합문서를 찾을 수 없습니다: {0}")]
    WorkbookNotFound(String),
    #[error("시트를 찾을 수 없습니다: {0}")]
    SheetNotFound(String),
    #[error("잘못된 범위: {0}")]
    Range(#[from] collect_core::a1::A1Error),
    #[error("Excel 스레드가 종료되었습니다")]
    ThreadGone,
    #[error(transparent)]
    Windows(#[from] windows_core::Error),
}

pub type Result<T> = std::result::Result<T, ExcelError>;
