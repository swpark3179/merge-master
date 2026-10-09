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
  SaveOptions,
  SaveResult,
  FirewallStatus,
  HostSnapshot,
  ServiceInfo,
  SheetInspection,
  SheetModel,
  WorkbookInfo,
} from "@mm/shared";
import { Channel, invoke } from "@tauri-apps/api/core";

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

  commitImport: (headerRows: number, frozenCols: number) =>
    invoke<SheetModel>("import_commit", { headerRows, frozenCols }),

  configView: (viewer: string | null) => invoke<ConfigView>("config_view_get", { viewer }),

  configApply: (edit: ConfigEdit, viewer: string | null) => invoke<ConfigView>("config_apply", { edit, viewer }),

  sheetFormat: (edit: FormatEdit, viewer: string | null) => invoke<FormatResult>("sheet_format", { edit, viewer }),

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

  /** `range`는 시트 주소(예: H500:H620). 주면 그 범위를 배정하고 승인한다. */
  participantApprove: (name: string, range: string | null, note: string) =>
    invoke<void>("participant_approve", { name, range, note }),

  participantReject: (name: string) => invoke<void>("participant_reject", { name }),

  participantKick: (name: string) => invoke<void>("participant_kick", { name }),

  /** 그 사람이 잡고 있는 편집 잠금을 푼다. */
  participantUnlock: (name: string) => invoke<void>("participant_unlock", { name }),

  recalcStatus: () => invoke<RecalcStatus>("recalc_status"),

  /** 6 저장·마감: 요약·원본 상태·기본 저장 경로. */
  saveInfo: () => invoke<SaveInfo>("save_info"),

  /** 저장 전 확인: 쓸 칸, 그사이 Excel에서 고친 칸, 표 위치 변화. */
  saveCheck: (options: SaveOptions) => invoke<SaveCheck>("save_check", { options }),

  /** 충돌 칸은 모두 `keepExcel`(Excel 값 유지) 또는 `useCollected`(취합 값) 중 하나여야 한다. */
  saveRun: (options: SaveOptions, keepExcel: CellRef[], useCollected: CellRef[], force: boolean) =>
    invoke<SaveResult>("save_run", { options, keepExcel, useCollected, force }),

  /** 취합 마감(참여자 읽기 전용) / 다시 열기. */
  collectClose: (closed: boolean) => invoke<void>("collect_close", { closed }),

  revealFile: (path: string) => invoke<void>("reveal_file", { path }),

  // ---------------------------------------------------------- 4 진행 모니터링 · 1i 소통
  /** 지금 시트 + 호스트 시점 설정 보기. */
  monitorGet: () => invoke<MonitorData>("monitor_get"),

  monitorProgress: () => invoke<ProgressStats>("monitor_progress"),

  monitorActivity: (limit?: number) => invoke<Activity>("monitor_activity", { limit }),

  commsGet: () => invoke<CommsSnapshot>("comms_get"),

  noticeSend: (text: string, important: boolean, target: NoticeTarget) =>
    invoke<void>("notice_send", { text, important, target }),

  /** `range`는 모델 좌표. */
  highlightAdd: (range: CellRange, note: string, audience: HighlightAudience) =>
    invoke<void>("highlight_add", { range, note, audience }),

  highlightClear: (id: number) => invoke<void>("highlight_clear", { id }),

  chatSend: (to: ChatTo, text: string) => invoke<void>("chat_send", { to, text }),

  // ---------------------------------------------------------- 5 히스토리
  historyQuery: (filter: HistoryFilter, offset: number, limit: number) =>
    invoke<HistoryPage>("history_query", { filter, offset, limit }),

  historyTimeline: (at: CellRef) => invoke<Timeline>("history_timeline", { at }),

  /** `stale`이 있으면 `force` 없이는 되돌리지 않는다(결과의 `changed` = 0). */
  historyRevert: (seqs: number[], force: boolean) => invoke<RevertResult>("history_revert", { seqs, force }),

  historyRevertBatch: (batch: string, force: boolean) => invoke<RevertResult>("history_revert_batch", { batch, force }),

  historySetValue: (at: CellRef, value: CellValue) => invoke<void>("history_set_value", { at, value }),

  historyExportCsv: (filter: HistoryFilter, path: string) => invoke<number>("history_export_csv", { filter, path }),

  snapshotList: () => invoke<SnapshotView[]>("snapshot_list"),

  snapshotTake: (label: string) => invoke<void>("snapshot_take", { label }),

  snapshotDiff: (id: number) => invoke<SnapshotDiff>("snapshot_diff", { id }),

  snapshotRestore: (id: number) => invoke<number>("snapshot_restore", { id }),

  qrSvg: (text: string) => invoke<string>("qr_svg", { text }),

  firewallCheck: (port: number) => invoke<FirewallStatus>("firewall_check", { port }),

  firewallAdd: (port: number) => invoke<void>("firewall_add", { port }),
};

/** Rust `ExcelError::Cancelled`의 메시지. 사용자가 취소했거나 새 읽기가 앞선 읽기를 대신한 경우라 오류로 보이지 않는다. */
export const CANCELLED = "읽기를 취소했습니다";

export function errorText(e: unknown): string {
  return typeof e === "string" ? e : e instanceof Error ? e.message : JSON.stringify(e);
}
