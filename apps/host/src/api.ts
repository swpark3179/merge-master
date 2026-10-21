import type {
  Activity,
  AgentConnection,
  AgentSaved,
  AgentServiceView,
  CellRange,
  CellRef,
  CellValue,
  ChatReaction,
  ChatTo,
  CommsSnapshot,
  ConfigEdit,
  ConfigView,
  ExcelStatus,
  FolderList,
  FormatEdit,
  FormatResult,
  HandoffCheck,
  HandoffOptions,
  HighlightAudience,
  HistoryFilter,
  HistoryPage,
  HostSnapshot,
  ImportResult,
  IpEntry,
  IpNamesView,
  JobState,
  JobSummary,
  LanAddress,
  MonitorData,
  NoticeTarget,
  ProgressStats,
  ReadOptions,
  ReadProgress,
  RecalcStatus,
  RestoreReport,
  RevertResult,
  SaveCheck,
  SaveInfo,
  SaveOptions,
  SaveResult,
  ServerStatus,
  ServiceInfo,
  SheetCell,
  SheetInspection,
  SheetModel,
  SnapshotDiff,
  SnapshotView,
  StructureEdit,
  Timeline,
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

/**
 * Tauri command 래퍼(앱 공통 — 업무와 상관없는 것). 이름·인수는 src-tauri/src/lib.rs와 맞춘다
 * (Rust snake_case ↔ JS camelCase). 업무에 딸린 command는 [`jobApi`].
 */
export const appApi = {
  listWorkbooks: () => invoke<ExcelStatus>("excel_list_workbooks"),

  openFile: (path: string) => invoke<WorkbookInfo>("excel_open_file", { path }),

  inspectSheet: (workbookId: string, sheet: string) =>
    invoke<SheetInspection>("excel_inspect_sheet", { workbookId, sheet }),

  cancelRead: () => invoke<void>("excel_cancel_read"),

  lanAddresses: () => invoke<LanAddress[]>("lan_addresses"),

  // ---------------------------------------------------------- 취합 업무
  jobsList: () => invoke<JobSummary[]>("jobs_list"),

  /** 새 취합 업무(시트는 1a에서 불러와 확정한다). */
  jobCreate: () => invoke<JobSummary>("job_create"),

  /** 업무를 끝낸다(공개 중지, 계산 사본 닫기, 앱을 다시 켜도 되살리지 않음). 남은 업무 목록. */
  jobEnd: (job: string) => invoke<JobSummary[]>("job_end", { job }),

  /** 앱을 켤 때 한 번: 끝내지 않은 업무를 모두 되살린다(공개 중이던 업무는 다시 공개). 업무가 없으면 빈 업무 하나. */
  jobsRestore: () => invoke<RestoreReport>("jobs_restore"),

  /** 참여자 서버(모든 업무 공통 포트). */
  serverStatus: () => invoke<ServerStatus>("server_status"),

  // ---------------------------------------------------------- 1c IP 명단(IP별 참여자 이름, 모든 업무 공통)
  /** 명단 IP에서 오면 그 이름으로 바로 들어오고, 명단 밖 IP는 이름 + 승인. 호스트 IP의 이름이 호스트 이름이 된다. */
  ipNamesGet: () => invoke<IpNamesView>("ip_names_get"),

  ipNamesSet: (entries: IpEntry[]) => invoke<IpNamesView>("ip_names_set", { entries }),

  /** 한 사람을 더하거나 이름을 바꾼다(승인할 때 'IP 명단에 저장'). */
  ipNamesPut: (ip: string, name: string) => invoke<IpNamesView>("ip_names_put", { ip, name }),

  ipNamesImport: (path: string) => invoke<IpNamesView>("ip_names_import", { path }),

  /** 쓴 사람 수를 돌려준다. */
  ipNamesExport: (path: string) => invoke<number>("ip_names_export", { path }),

  revealFile: (path: string) => invoke<void>("reveal_file", { path }),

  // ---------------------------------------------------------- 에이전트 PC(다른 PC에서 서비스)
  /** 마지막으로 연결한 에이전트 주소. */
  agentLastAddr: () => invoke<string | null>("agent_last_addr"),

  /** 에이전트에 연결해 본다(`PC01`, `10.0.0.7`, `10.0.0.7:8090` 모두 받는다). */
  agentConnect: (addr: string) => invoke<AgentConnection>("agent_connect", { addr }),

  /** 에이전트 기본 저장 폴더 아래 폴더 하나(`path`는 경로 조각). */
  agentFolders: (addr: string, path: string[]) => invoke<FolderList>("agent_folders", { addr, path }),

  /** 넘긴 서비스들(3초마다 `agent-services` 이벤트로도 온다). */
  agentServices: () => invoke<AgentServiceView[]>("agent_services"),

  agentRefresh: () => invoke<AgentServiceView[]>("agent_refresh"),

  /** 지금 저장하고 끝내기(`save`) / 저장 없이 끝내기. 저장은 에이전트가 뒤에서 한다(상태 '저장 중'). */
  agentFinish: (id: string, save: boolean) => invoke<AgentServiceView>("agent_finish", { id, save }),

  /** 저장 실패·관리자 저장 대기를 에이전트 PC에서 다시 저장한다. */
  agentRetry: (id: string) => invoke<AgentServiceView>("agent_retry", { id }),

  /** 이 PC에서 저장할 기본 경로(원본 폴더, 관리자 저장 대기면 에이전트가 저장하려던 이름). */
  agentSaveDefault: (id: string) => invoke<string>("agent_save_default", { id }),

  /** 서비스 중인 에이전트 업무의 지금 상태를 이 PC에 새 파일로 저장한다(서비스는 그대로). */
  agentSaveNow: (id: string, path: string) => invoke<AgentSaved>("agent_save_now", { id, path }),

  /** 관리자 저장 대기의 취합 데이터를 이 PC에 저장하고 에이전트에 알린다(그쪽 데이터를 지우고 끝낸다). */
  agentSaveResult: (id: string, path: string) => invoke<AgentSaved>("agent_save_result", { id, path }),

  /** 목록에서 지운다(에이전트의 서비스는 그대로). */
  agentForget: (id: string) => invoke<AgentServiceView[]>("agent_forget", { id }),
};

/**
 * 그 업무의 command 래퍼: 모든 호출에 업무 id(`job`)를 붙인다. 화면은 [`useApi`](./job)로 지금 업무의 것을 받아 쓰므로,
 * 업무를 바꾼 뒤 늦게 끝난 호출도 원래 업무로 간다.
 */
export function jobApi(job: string) {
  const call = <T>(cmd: string, args: Record<string, unknown> = {}) => invoke<T>(cmd, { job, ...args });
  return {
    /** 업무를 고를 때 다시 그릴 것(확정한 시트·공개 상태·접속자 표·소통·재계산). */
    jobState: () => call<JobState>("job_state"),

    readSheet: (workbookId: string, sheet: string, options: ReadOptions, onProgress: (p: ReadProgress) => void) => {
      const channel = new Channel<ReadProgress>();
      channel.onmessage = onProgress;
      return call<ImportResult>("excel_read_sheet", { workbookId, sheet, options, onProgress: channel });
    },

    /** 미리보기로 읽은 시트들을 확정한다(통합문서 순서). 시트 순번 = 이 순서. */
    commitImport: (workbookId: string, sheets: CommitSheet[]) =>
      call<SheetModel[]>("import_commit", { workbookId, sheets }),

    configView: (sheet: number, viewer: string | null) => call<ConfigView>("config_view_get", { sheet, viewer }),

    /** 제목·안내·마감, 직접 넣은 명단은 모든 시트가 같이 쓴다(어느 시트에서 고쳐도 된다). */
    configApply: (sheet: number, edit: ConfigEdit, viewer: string | null) =>
      call<ConfigView>("config_apply", { sheet, edit, viewer }),

    sheetFormat: (sheet: number, edit: FormatEdit, viewer: string | null) =>
      call<FormatResult>("sheet_format", { sheet, edit, viewer }),

    /**
     * 표 구조 바꾸기(1b·1d 오른쪽 클릭 메뉴): 표 아래에 행·오른쪽에 열 추가, 앱에서 붙인 행·열 지우기, 행 취소 표시.
     * 지울 수 없는 행·열이면 까닭을 오류로 돌려준다. 바뀐 시트와 설정 보기를 돌려준다(참여자에게는 새 시트가 간다).
     */
    sheetStructure: (sheet: number, edit: StructureEdit, viewer: string | null) =>
      call<FormatResult>("sheet_structure", { sheet, edit, viewer }),

    /** 호스트가 칸 내용을 바꾼다(1b·모니터링). 같은 값은 건너뛴다(수식 칸은 수식 대신 그 값). 바뀐 칸은 `sheet-changes`로 온다. */
    sheetSetCells: (sheet: number, cells: CellWrite[]) => call<void>("sheet_set_cells", { sheet, cells }),

    /** 이 업무를 이 주소 경로로 공개한다. 포트는 모든 업무가 같이 쓴다(다른 업무가 공개 중이면 바꿀 수 없다). */
    servicePublish: (port: number, slug: string) => call<ServiceInfo>("service_publish", { port, slug }),

    /** 이 업무의 공개를 멈춘다(다른 업무는 그대로). */
    serviceUnpublish: () => call<void>("service_unpublish"),

    serviceInfo: () => call<ServiceInfo | null>("service_info"),

    participantsGet: () => call<HostSnapshot | null>("participants_get"),

    /** `range`는 `sheet` 시트의 주소(예: H500:H620). 주면 그 범위를 배정하고 승인한다. */
    participantApprove: (name: string, sheet: number, range: string | null, note: string) =>
      call<void>("participant_approve", { name, sheet, range, note }),

    participantReject: (name: string) => call<void>("participant_reject", { name }),

    participantKick: (name: string) => call<void>("participant_kick", { name }),

    /** 그 사람이 잡고 있는 편집 잠금을 푼다. */
    participantUnlock: (name: string) => call<void>("participant_unlock", { name }),

    recalcStatus: () => call<RecalcStatus>("recalc_status"),

    /** 에이전트로 넘기기 전 확인(원본 표 위치·그사이 Excel에서 고친 칸). */
    agentHandoffCheck: () => call<HandoffCheck>("agent_handoff_check"),

    /** 에이전트로 넘긴다. 받으면 이 PC의 업무는 끝난다(`jobs` 이벤트). */
    agentHandoff: (options: HandoffOptions) => call<AgentServiceView>("agent_handoff", { options }),

    /** 6 저장·마감: 요약·원본 상태·기본 저장 경로. */
    saveInfo: () => call<SaveInfo>("save_info"),

    /** 저장 전 확인: 쓸 칸, 그사이 Excel에서 고친 칸, 표 위치 변화. */
    saveCheck: (options: SaveOptions) => call<SaveCheck>("save_check", { options }),

    /** 충돌 칸(시트·칸)은 모두 `keepExcel`(Excel 값 유지) 또는 `useCollected`(취합 값) 중 하나여야 한다. */
    saveRun: (options: SaveOptions, keepExcel: SheetCell[], useCollected: SheetCell[], force: boolean) =>
      call<SaveResult>("save_run", { options, keepExcel, useCollected, force }),

    /** 취합 마감(참여자 읽기 전용) / 다시 열기. */
    collectClose: (closed: boolean) => call<void>("collect_close", { closed }),

    // ---------------------------------------------------------- 4 진행 모니터링 · 1i 소통
    /** 그 시트의 지금 값 + 호스트 시점 설정 보기. */
    monitorGet: (sheet: number) => call<MonitorData>("monitor_get", { sheet }),

    monitorProgress: (sheet: number) => call<ProgressStats>("monitor_progress", { sheet }),

    /** 활동 피드는 모든 시트, '최근 삭제된 셀'은 `sheet` 시트. */
    monitorActivity: (sheet: number, limit?: number) => call<Activity>("monitor_activity", { sheet, limit }),

    commsGet: () => call<CommsSnapshot>("comms_get"),

    noticeSend: (text: string, important: boolean, target: NoticeTarget) =>
      call<void>("notice_send", { text, important, target }),

    /** `range`는 그 시트의 모델 좌표. */
    highlightAdd: (sheet: number, range: CellRange, note: string, audience: HighlightAudience) =>
      call<void>("highlight_add", { sheet, range, note, audience }),

    highlightClear: (id: number) => call<void>("highlight_clear", { id }),

    chatSend: (to: ChatTo, text: string) => call<void>("chat_send", { to, text }),

    /** 참여자 채팅에 리액션을 달거나 뗀다(같은 종류를 다시 보내면 뗀다). */
    chatReact: (id: number, kind: ChatReaction) => call<void>("chat_react", { id, kind }),

    // ---------------------------------------------------------- 5 히스토리
    historyQuery: (filter: HistoryFilter, offset: number, limit: number) =>
      call<HistoryPage>("history_query", { filter, offset, limit }),

    historyTimeline: (sheet: number, at: CellRef) => call<Timeline>("history_timeline", { sheet, at }),

    /** `stale`이 있으면 `force` 없이는 되돌리지 않는다(결과의 `changed` = 0). */
    historyRevert: (seqs: number[], force: boolean) => call<RevertResult>("history_revert", { seqs, force }),

    historyRevertBatch: (batch: string, force: boolean) => call<RevertResult>("history_revert_batch", { batch, force }),

    historySetValue: (sheet: number, at: CellRef, value: CellValue) =>
      call<void>("history_set_value", { sheet, at, value }),

    historyExportCsv: (filter: HistoryFilter, path: string) => call<number>("history_export_csv", { filter, path }),

    snapshotList: () => call<SnapshotView[]>("snapshot_list"),

    snapshotTake: (label: string) => call<void>("snapshot_take", { label }),

    snapshotDiff: (id: number) => call<SnapshotDiff>("snapshot_diff", { id }),

    snapshotRestore: (id: number) => call<number>("snapshot_restore", { id }),
  };
}

/** 화면이 쓰는 API: 앱 공통 + 그 업무. */
export type Api = typeof appApi & ReturnType<typeof jobApi>;

export function makeApi(job: string): Api {
  return { ...appApi, ...jobApi(job) };
}

/** 그 업무의 이벤트 이름(`participants:{업무 id}`). Rust `jobs::event`와 같다. */
export function jobEvent(name: string, job: string): string {
  return `${name}:${job}`;
}

/** Rust `ExcelError::Cancelled`의 메시지. 사용자가 취소했거나 새 읽기가 앞선 읽기를 대신한 경우라 오류로 보이지 않는다. */
export const CANCELLED = "읽기를 취소했습니다";

export function errorText(e: unknown): string {
  return typeof e === "string" ? e : e instanceof Error ? e.message : JSON.stringify(e);
}
