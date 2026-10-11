import type {
  ConfigEdit,
  ConfigView,
  ExcelStatus,
  FormatEdit,
  FormatResult,
  ImportResult,
  LanAddress,
  ReadOptions,
  ReadProgress,
  RecalcStatus,
  RecoveryInfo,
  ResumeResult,
  Activity,
  CellRange,
  CellValue,
  ChatTo,
  CommsSnapshot,
  HighlightAudience,
  HistoryFilter,
  HistoryPage,
  MonitorData,
  NoticeTarget,
  ProgressStats,
  RevertResult,
  SnapshotDiff,
  SnapshotView,
  Timeline,
  CellRef,
  SaveCheck,
  SaveInfo,
  SheetCell,
  SaveOptions,
  SaveResult,
  HostSnapshot,
  IpEntry,
  IpNamesView,
  ServiceInfo,
  SheetInspection,
  SheetModel,
  WorkbookInfo,
} from "@mm/shared";
import { Channel, invoke } from "@tauri-apps/api/core";

/** 호스트가 칸에 넣을 값(모델 좌표). Rust `src-tauri/src/lib.rs`의 `CellWrite`. */
export interface CellWrite {
  row: number;
  col: number;
  value: CellValue;
}

/** 확정할 시트 하나(미리보기로 읽은 것)와 다시 읽지 않아도 되는 조정. Rust `lib.rs`의 `CommitSheet`. */
export interface CommitSheet {
  sheet: string;
  headerRows: number;
  frozenCols: number;
}

/** Tauri command 래퍼. 이름·인수는 src-tauri/src/lib.rs와 맞춘다(Rust snake_case ↔ JS camelCase). */
export const api = {
  listWorkbooks: () => invoke<ExcelStatus>("excel_list_workbooks"),

  openFile: (path: string) => invoke<WorkbookInfo>("excel_open_file", { path }),

  inspectSheet: (workbookId: string, sheet: string) =>
    invoke<SheetInspection>("excel_inspect_sheet", { workbookId, sheet }),

  readSheet: (workbookId: string, sheet: string, options: ReadOptions, onProgress: (p: ReadProgress) => void) => {
    const channel = new Channel<ReadProgress>();
    channel.onmessage = onProgress;
    return invoke<ImportResult>("excel_read_sheet", { workbookId, sheet, options, onProgress: channel });
  },

  cancelRead: () => invoke<void>("excel_cancel_read"),

  /** 미리보기로 읽은 시트들을 확정한다(통합문서 순서). 시트 순번 = 이 순서. */
  commitImport: (workbookId: string, sheets: CommitSheet[]) =>
    invoke<SheetModel[]>("import_commit", { workbookId, sheets }),

  configView: (sheet: number, viewer: string | null) => invoke<ConfigView>("config_view_get", { sheet, viewer }),

  /** 제목·안내·마감, 직접 넣은 명단은 모든 시트가 같이 쓴다(어느 시트에서 고쳐도 된다). */
  configApply: (sheet: number, edit: ConfigEdit, viewer: string | null) =>
    invoke<ConfigView>("config_apply", { sheet, edit, viewer }),

  sheetFormat: (sheet: number, edit: FormatEdit, viewer: string | null) =>
    invoke<FormatResult>("sheet_format", { sheet, edit, viewer }),

  /** 호스트가 칸 내용을 바꾼다(1b·모니터링). 수식 칸·같은 값은 건너뛴다. 바뀐 칸은 `sheet-changes`로 온다. */
  sheetSetCells: (sheet: number, cells: CellWrite[]) => invoke<void>("sheet_set_cells", { sheet, cells }),

  lanAddresses: () => invoke<LanAddress[]>("lan_addresses"),

  serverStart: (port: number, slug: string) => invoke<ServiceInfo>("server_start", { port, slug }),

  serverStop: () => invoke<void>("server_stop"),

  serverInfo: () => invoke<ServiceInfo | null>("server_info"),

  /** 앱을 다시 켰을 때 이어서 할 수 있는 지난 취합. */
  recoveryCheck: () => invoke<RecoveryInfo | null>("recovery_check"),

  /** 지난 취합을 되살린다(서비스가 켜져 있었으면 같은 포트로 다시 연다). */
  recoveryResume: () => invoke<ResumeResult>("recovery_resume"),

  recoveryDiscard: () => invoke<void>("recovery_discard"),

  participantsGet: () => invoke<HostSnapshot | null>("participants_get"),

  /** `range`는 `sheet` 시트의 주소(예: H500:H620). 주면 그 범위를 배정하고 승인한다. */
  participantApprove: (name: string, sheet: number, range: string | null, note: string) =>
    invoke<void>("participant_approve", { name, sheet, range, note }),

  participantReject: (name: string) => invoke<void>("participant_reject", { name }),

  participantKick: (name: string) => invoke<void>("participant_kick", { name }),

  /** 그 사람이 잡고 있는 편집 잠금을 푼다. */
  participantUnlock: (name: string) => invoke<void>("participant_unlock", { name }),

  // ---------------------------------------------------------- 1c IP 명단(IP별 참여자 이름)
  /** 명단 IP에서 오면 그 이름으로 바로 들어오고, 명단 밖 IP는 이름 + 승인. 호스트 IP의 이름이 호스트 이름이 된다. */
  ipNamesGet: () => invoke<IpNamesView>("ip_names_get"),

  ipNamesSet: (entries: IpEntry[]) => invoke<IpNamesView>("ip_names_set", { entries }),

  /** 한 사람을 더하거나 이름을 바꾼다(승인할 때 'IP 명단에 저장'). */
  ipNamesPut: (ip: string, name: string) => invoke<IpNamesView>("ip_names_put", { ip, name }),

  ipNamesImport: (path: string) => invoke<IpNamesView>("ip_names_import", { path }),

  /** 쓴 사람 수를 돌려준다. */
  ipNamesExport: (path: string) => invoke<number>("ip_names_export", { path }),

  recalcStatus: () => invoke<RecalcStatus>("recalc_status"),

  /** 6 저장·마감: 요약·원본 상태·기본 저장 경로. */
  saveInfo: () => invoke<SaveInfo>("save_info"),

  /** 저장 전 확인: 쓸 칸, 그사이 Excel에서 고친 칸, 표 위치 변화. */
  saveCheck: (options: SaveOptions) => invoke<SaveCheck>("save_check", { options }),

  /** 충돌 칸(시트·칸)은 모두 `keepExcel`(Excel 값 유지) 또는 `useCollected`(취합 값) 중 하나여야 한다. */
  saveRun: (options: SaveOptions, keepExcel: SheetCell[], useCollected: SheetCell[], force: boolean) =>
    invoke<SaveResult>("save_run", { options, keepExcel, useCollected, force }),

  /** 취합 마감(참여자 읽기 전용) / 다시 열기. */
  collectClose: (closed: boolean) => invoke<void>("collect_close", { closed }),

  revealFile: (path: string) => invoke<void>("reveal_file", { path }),

  // ---------------------------------------------------------- 4 진행 모니터링 · 1i 소통
  /** 그 시트의 지금 값 + 호스트 시점 설정 보기. */
  monitorGet: (sheet: number) => invoke<MonitorData>("monitor_get", { sheet }),

  monitorProgress: (sheet: number) => invoke<ProgressStats>("monitor_progress", { sheet }),

  /** 활동 피드는 모든 시트, '최근 삭제된 셀'은 `sheet` 시트. */
  monitorActivity: (sheet: number, limit?: number) => invoke<Activity>("monitor_activity", { sheet, limit }),

  commsGet: () => invoke<CommsSnapshot>("comms_get"),

  noticeSend: (text: string, important: boolean, target: NoticeTarget) =>
    invoke<void>("notice_send", { text, important, target }),

  /** `range`는 그 시트의 모델 좌표. */
  highlightAdd: (sheet: number, range: CellRange, note: string, audience: HighlightAudience) =>
    invoke<void>("highlight_add", { sheet, range, note, audience }),

  highlightClear: (id: number) => invoke<void>("highlight_clear", { id }),

  chatSend: (to: ChatTo, text: string) => invoke<void>("chat_send", { to, text }),

  // ---------------------------------------------------------- 5 히스토리
  historyQuery: (filter: HistoryFilter, offset: number, limit: number) =>
    invoke<HistoryPage>("history_query", { filter, offset, limit }),

  historyTimeline: (sheet: number, at: CellRef) => invoke<Timeline>("history_timeline", { sheet, at }),

  /** `stale`이 있으면 `force` 없이는 되돌리지 않는다(결과의 `changed` = 0). */
  historyRevert: (seqs: number[], force: boolean) => invoke<RevertResult>("history_revert", { seqs, force }),

  historyRevertBatch: (batch: string, force: boolean) => invoke<RevertResult>("history_revert_batch", { batch, force }),

  historySetValue: (sheet: number, at: CellRef, value: CellValue) =>
    invoke<void>("history_set_value", { sheet, at, value }),

  historyExportCsv: (filter: HistoryFilter, path: string) => invoke<number>("history_export_csv", { filter, path }),

  snapshotList: () => invoke<SnapshotView[]>("snapshot_list"),

  snapshotTake: (label: string) => invoke<void>("snapshot_take", { label }),

  snapshotDiff: (id: number) => invoke<SnapshotDiff>("snapshot_diff", { id }),

  snapshotRestore: (id: number) => invoke<number>("snapshot_restore", { id }),
};

/** Rust `ExcelError::Cancelled`의 메시지. 사용자가 취소했거나 새 읽기가 앞선 읽기를 대신한 경우라 오류로 보이지 않는다. */
export const CANCELLED = "읽기를 취소했습니다";

export function errorText(e: unknown): string {
  return typeof e === "string" ? e : e instanceof Error ? e.message : JSON.stringify(e);
}
