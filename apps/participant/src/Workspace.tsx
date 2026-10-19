import { PickHint, useCellPick } from "@mm/chat";
import type {
  CellDecoration,
  EditorState,
  GridHit,
  GridMark,
  GridRange,
  GridSelection,
  Move,
  SheetGridHandle,
} from "@mm/grid";
import {
  CellEditor,
  ContextMenu,
  fitRowHeights,
  MenuItem,
  MenuNote,
  MenuSep,
  parseTsv,
  resizeRuns,
  selectForMenu,
  SheetGrid,
  SizeControl,
  toTsv,
  trimHiddenRows,
} from "@mm/grid";
import type { Axis, CellRange, CellRef, HAlign, SheetModel, StylePatch } from "@mm/shared";
import {
  a1,
  cellRefToken,
  colLetters,
  displayValue,
  editText,
  FLAG,
  formatRange,
  normalize,
  personColor,
  rgbHex,
  sheetPrefix,
} from "@mm/shared";
import type { ClipboardEvent, CSSProperties, KeyboardEvent, MouseEvent as ReactMouseEvent, ReactNode } from "react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { useTitleAlert } from "./attention";
import type { BenchResult } from "./bench";
import { runScrollBench } from "./bench";
import type { CellLeave } from "./CellDetail";
import { DeadlineClock } from "./DeadlineClock";
import type { RowFilter } from "./filters";
import { filteredOut, filterOptions, isFiltering, myRowsOf, NO_FILTER, withHiddenRows } from "./filters";
import type { BulkResult, EditContext } from "./gridOps";
import { canEdit, clearRange, copyRange, pasteRange } from "./gridOps";
import { loadHiddenNotes, pruneHidden, saveHiddenNotes, toggleHidden } from "./hiddenNotes";
import { NoticeBanners } from "./NoticeBanners";
import { RowForm } from "./RowForm";
import { SearchSelect } from "./SearchSelect";
import type { PanelTab } from "./SidePanel";
import { clampSideWidth, SIDE_WIDTH, SidePanel } from "./SidePanel";
import { storageSlug } from "./slug";
import type { Notice, SessionState } from "./useSession";
import { pageTitle } from "./useSession";

const FILLS: { label: string; rgb: number | null }[] = [
  { label: "노랑", rgb: 0xfff3b0 },
  { label: "초록", rgb: 0xd9f2e3 },
  { label: "파랑", rgb: 0xdbe8f8 },
  { label: "분홍", rgb: 0xfadbe6 },
  { label: "빨강", rgb: 0xea2a2a },
  { label: "진한 노랑", rgb: 0xffff00 },
  { label: "진한 파랑", rgb: 0x0000ff },
  { label: "채우기 없음", rgb: null },
];

/** 글자색(채우기처럼 골라 바꾼다). 검정이 기본 글자색이다. */
const TEXT_COLORS: { label: string; rgb: number }[] = [
  { label: "검정", rgb: 0x000000 },
  { label: "빨강", rgb: 0xea2a2a },
  { label: "진한 파랑", rgb: 0x0000ff },
  { label: "진한 노랑", rgb: 0xffff00 },
  { label: "회색", rgb: 0x7f7f7f },
];

const ALIGNS: { value: HAlign; label: string; icon: ReactNode }[] = [
  { value: "left", label: "왼쪽 맞춤", icon: <AlignIcon lines={[16, 10, 14]} anchor="start" /> },
  { value: "center", label: "가운데 맞춤", icon: <AlignIcon lines={[16, 10, 14]} anchor="middle" /> },
  { value: "right", label: "오른쪽 맞춤", icon: <AlignIcon lines={[16, 10, 14]} anchor="end" /> },
];

/** 맞춤 단추 그림: 길이가 다른 가로줄 셋을 왼쪽·가운데·오른쪽에 맞춘다. */
function AlignIcon({ lines, anchor }: { lines: number[]; anchor: "start" | "middle" | "end" }) {
  return (
    <svg width="16" height="16" viewBox="0 0 20 20" aria-hidden>
      {lines.map((w, i) => {
        const x = anchor === "start" ? 2 : anchor === "end" ? 18 - w : 10 - w / 2;
        return <rect key={i} x={x} y={4 + i * 5} width={w} height={2} rx={1} fill="currentColor" />;
      })}
    </svg>
  );
}

/** 도구바 선 그림(24×24 경로). */
function ToolIcon({ d, size = 16 }: { d: string; size?: number }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
    >
      <path d={d} />
    </svg>
  );
}

/**
 * 오른쪽 클릭 메뉴: 행·열 머리글, 또는 메모가 있는 강조 칸(메모 숨기기·셀 이력). `confirm`: 값이 있는 행을 지우기 전
 * 확인 중(지울 칸 수).
 */
interface HeaderMenu {
  x: number;
  y: number;
  hit: GridHit;
  confirm?: number;
}

interface ActiveEdit {
  at: CellRef;
  state: EditorState;
  /** 서버가 잠금을 허락했다(이후 풀림 알림은 호스트가 강제로 푼 것). */
  locked: boolean;
}

function rectOf(sel: GridSelection): CellRange {
  const row = Math.min(sel.anchor.row, sel.focus.row);
  const col = Math.min(sel.anchor.col, sel.focus.col);
  return {
    row,
    col,
    rows: Math.abs(sel.anchor.row - sel.focus.row) + 1,
    cols: Math.abs(sel.anchor.col - sel.focus.col) + 1,
  };
}

/** 채팅 참조로 간 범위를 표시해 두는 시간(ms): 그리드의 `mm-ping`(900ms)이 끝난 뒤 뗀다. */
const PING_MS = 1000;

/** 조회조건을 고르지 않은 열·항목이 없는 열(렌더마다 새 배열을 만들지 않게). */
const NO_VALUES: readonly string[] = [];

/** 범위를 시트 안으로 줄인다(채팅에 적힌 주소가 시트보다 클 수 있다). 시트 밖이면 `null`. */
function clampTo(model: Pick<SheetModel, "rows" | "cols">, r: CellRange): CellRange | null {
  const row = Math.max(0, r.row);
  const col = Math.max(0, r.col);
  const rows = Math.min(model.rows, r.row + r.rows) - row;
  const cols = Math.min(model.cols, r.col + r.cols) - col;
  return rows > 0 && cols > 0 ? { row, col, rows, cols } : null;
}

/** 사이드바를 열어 둘지(주소 경로마다 기억). 처음에는 넓은 화면이면 연다. */
function sideKey(): string {
  return `mm.${storageSlug()}.side`;
}

function loadSide(): boolean {
  try {
    const v = localStorage.getItem(sideKey());
    if (v !== null) return v === "1";
  } catch {
    // 기억하지 못해도 동작은 한다.
  }
  return window.innerWidth >= 1100;
}

function saveSide(open: boolean) {
  try {
    localStorage.setItem(sideKey(), open ? "1" : "0");
  } catch {
    // 사생활 보호 모드 등
  }
}

/** 사이드바 너비(끌어서 바꾼 것, 주소 경로마다 기억). */
function loadSideWidth(): number {
  try {
    const v = Number(localStorage.getItem(`${sideKey()}.width`));
    if (v > 0) return clampSideWidth(v);
  } catch {
    // 기억하지 못해도 동작은 한다.
  }
  return SIDE_WIDTH;
}

function saveSideWidth(w: number) {
  try {
    localStorage.setItem(`${sideKey()}.width`, String(w));
  } catch {
    // 사생활 보호 모드 등
  }
}

function clock(ms: number): string {
  const d = new Date(ms);
  return [d.getHours(), d.getMinutes(), d.getSeconds()].map((n) => String(n).padStart(2, "0")).join(":");
}

/** 1f 참여자 그리드(셀 단위 동시 편집)와 1g 행 단위 입력 폼. */
export function Workspace({ s }: { s: SessionState }) {
  const w = s.welcome!;
  const sheet = s.sheet!;
  const sv = s.view!;
  const model = sheet.model;
  const { columns, labels } = sv;
  const flags = s.flags;
  const active = s.active;
  const [mode, setMode] = useState<"grid" | "form">("grid");
  const [selection, setSelection] = useState<GridSelection | null>(null);
  const [filter, setFilter] = useState<RowFilter>(NO_FILTER);
  const [editor, setEditor] = useState<ActiveEdit | null>(null);
  const [formRow, setFormRow] = useState<number | null>(null);
  /** '끝나면 알려 주기'를 건 칸(`시트:칸 인덱스`). */
  const [waitFree, setWaitFree] = useState<Set<string>>(() => new Set());
  const [bench, setBench] = useState<BenchResult | null>(null);
  /** 오른쪽 사이드바(셀 내용·알림·채팅·셀 이력)와 그 탭. */
  const [sideOpen, setSideOpenState] = useState(loadSide);
  /** 사람이 연 사이드바만 미끄러져 들어온다(처음 화면에 이미 열려 있는 것은 그대로). */
  const [sideSlide, setSideSlide] = useState(false);
  const [panel, setPanelTab] = useState<PanelTab>("cell");
  const setSideOpen = useCallback((open: boolean) => {
    setSideOpenState(open);
    setSideSlide(open);
    saveSide(open);
  }, []);
  const [sideWidth, setSideWidthState] = useState(loadSideWidth);
  const setSideWidth = useCallback((w: number) => {
    setSideWidthState(w);
    saveSideWidth(w);
  }, []);
  /** 사이드바를 열고 그 탭을 보인다. */
  const showPanel = useCallback(
    (tab: PanelTab) => {
      setPanelTab(tab);
      setSideOpen(true);
    },
    [setSideOpen],
  );
  const [chatSeen, setChatSeen] = useState(0);
  /** 채팅 초안·'님에게만'(탭을 바꾸거나 사이드바를 닫아도 남는다). */
  const [chatDraft, setChatDraft] = useState("");
  const [chatToHost, setChatToHost] = useState(false);
  const [menu, setMenu] = useState<HeaderMenu | null>(null);
  const onChatSeen = useCallback((id: number) => setChatSeen((x) => Math.max(x, id)), []);
  /** 탭을 보고 있지 않을 때 온 채팅·공지를 탭 제목 깜빡임으로 알린다. */
  const titleAlert = useTitleAlert(pageTitle(w.session.title));

  /** 내가 숨긴 강조 메모(이 브라우저에만, 업무마다). */
  const job = w.session.job ?? "";
  const [hiddenNotes, setHiddenNotes] = useState(() => loadHiddenNotes(job));
  const notesJob = useRef(job);
  useEffect(() => {
    if (notesJob.current === job) return;
    notesJob.current = job;
    setHiddenNotes(loadHiddenNotes(job));
  }, [job]);
  // 호스트가 해제한 강조는 잊는다.
  useEffect(
    () =>
      setHiddenNotes((ids) =>
        pruneHidden(
          ids,
          s.highlights.map((h) => h.id),
        ),
      ),
    [s.highlights],
  );
  useEffect(() => saveHiddenNotes(notesJob.current, hiddenNotes), [hiddenNotes]);
  const toggleNote = useCallback((id: number) => setHiddenNotes((ids) => toggleHidden(ids, id)), []);
  /** 채팅 참조·강조로 간 범위: 테두리가 두 번 깜빡인다(`PING_MS` 뒤에 뗀다). */
  const [ping, setPing] = useState<{ sheet: number; range: CellRange; seq: number } | null>(null);
  const pingSeq = useRef(0);
  useEffect(() => {
    if (!ping) return;
    const id = window.setTimeout(() => setPing(null), PING_MS);
    return () => window.clearTimeout(id);
  }, [ping]);
  const editorRef = useRef<ActiveEdit | null>(null);
  editorRef.current = editor;
  /** 사이드바 '셀 내용'에서 고치던 것을 칸을 옮기기 전에 저장한다(패널이 열려 있을 때만 있다). */
  const cellLeave = useRef<CellLeave | null>(null);
  const input = useRef<HTMLTextAreaElement>(null);
  const grid = useRef<SheetGridHandle>(null);
  const gridHost = useRef<HTMLDivElement>(null);
  const lastBlocked = useRef(0);
  const { notify } = s;
  const idx = (r: number, c: number) => r * model.cols + c;
  const addr = (at: CellRef) => a1(model.origin.row + at.row, model.origin.col + at.col);

  // 새로 온 남의 채팅: 채팅 탭이 안 보이면 토스트(누르면 채팅 탭), 이 탭을 보고 있지 않으면 제목 깜빡임.
  // 들어올 때 이미 있던 글에는 알리지 않는다(업무가 바뀌면 다시 센다: 글 번호는 업무마다 새로 시작한다).
  const chatTop = useRef<{ job: string; id: number } | null>(null);
  const lastChat = s.chat[s.chat.length - 1]?.id ?? 0;
  const chatVisible = sideOpen && mode === "grid" && panel === "chat";
  useEffect(() => {
    const top = chatTop.current;
    if (!top || top.job !== job) {
      chatTop.current = { job, id: lastChat };
      return;
    }
    if (lastChat <= top.id) return;
    const fresh = s.chat.filter((m) => m.id > top.id && !(m.from === w.name && !m.host));
    top.id = lastChat;
    if (fresh.length === 0) return;
    if (!chatVisible) {
      const m = fresh[fresh.length - 1]!;
      const text = m.text.replace(/\s+/g, " ").trim();
      const who = `${m.from}${m.to.kind === "person" ? "(나에게)" : ""}`;
      const head = fresh.length > 1 ? `새 채팅 ${fresh.length}개 · ` : "";
      notify(
        `${head}${who}: ${text.length > 120 ? `${text.slice(0, 120)}…` : text}`,
        "info",
        { label: "채팅 열기", chat: true },
        "chat",
      );
    }
    for (let i = 0; i < fresh.length; i++) titleAlert("chat");
    // 마지막 글 번호가 바뀔 때만(그때의 탭 상태를 본다).
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [lastChat, job]);

  // 새로 온 공지: 위 막대가 알리고, 이 탭을 보고 있지 않으면 제목도 깜빡인다.
  const noticeTop = useRef<{ job: string; id: number } | null>(null);
  const lastNotice = s.announcements.reduce((m, n) => Math.max(m, n.id), 0);
  useEffect(() => {
    const top = noticeTop.current;
    if (!top || top.job !== job) {
      noticeTop.current = { job, id: lastNotice };
      return;
    }
    if (lastNotice <= top.id) return;
    if (s.announcements.some((n) => n.id > top.id && !n.read)) titleAlert("notice");
    top.id = lastNotice;
    // 마지막 공지 번호가 바뀔 때만.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [lastNotice, job]);

  const myRows = useMemo(
    () => myRowsOf(model, flags, sv.rowOwner, w.color, sv.permissions.othersRows === "editable"),
    [model, flags, sv.rowOwner, w.color, sv.permissions.othersRows],
  );
  const myRowCount = useMemo(() => myRows.reduce((n, x) => n + x, 0), [myRows]);
  // 필터는 걸 때 한 번 계산한다(입력하는 동안 행이 사라지지 않게).
  const hidden = useMemo(() => filteredOut(model, flags, myRows, filter), [model, flags, myRows, filter]);
  const filtered = useMemo(() => withHiddenRows(model, hidden), [model, hidden]);
  // 줄바꿈(Alt+Enter)이 든 칸은 다 보이도록 행 높이를 늘린다(칸 값은 제자리에서 바뀌므로 sheet.rev로 다시 본다).
  // 높이가 그대로면 앞의 보기를 그대로 써서 그리드가 배치를 다시 계산하지 않게 한다.
  const lastFit = useRef<{ base: SheetModel; heights: number[]; out: SheetModel } | null>(null);
  const view = useMemo(() => {
    const out = fitRowHeights(filtered);
    const prev = lastFit.current;
    if (
      prev &&
      prev.base === filtered &&
      prev.heights.length === out.rowHeights.length &&
      prev.heights.every((h, i) => h === out.rowHeights[i])
    )
      return prev.out;
    lastFit.current = { base: filtered, heights: out.rowHeights, out };
    return out;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [filtered, sheet.rev]);
  const filterCols = useMemo(
    () => (sv.permissions.filterSort ? columns.map((c, i) => (c.filter ? i : -1)).filter((i) => i >= 0) : []),
    [columns, sv.permissions.filterSort],
  );
  const options = useMemo(() => new Map(filterCols.map((c) => [c, filterOptions(model, c)])), [filterCols, model]);

  const ctx: EditContext = {
    model,
    flags,
    columns,
    locks: sheet.locks,
    hiddenRows: new Set(view.hiddenRows),
    hiddenCols: new Set(model.hiddenCols),
    readOnly: s.readOnly,
  };

  const at = selection?.anchor ?? null;
  const editable = at !== null && canEdit(ctx, at.row, at.col);
  const locker = at ? sheet.locks.get(idx(at.row, at.col)) : undefined;
  const rule = at ? columns[at.col] : undefined;
  const listItems = rule?.format.kind === "list" ? rule.format.items : undefined;

  // ------------------------------------------------------------ 편집기

  const startEdit = useCallback(
    (target: CellRef, text: string, opts: { arrowsMove: boolean; listOpen?: boolean }) => {
      const next: ActiveEdit = {
        at: target,
        state: { text, arrowsMove: opts.arrowsMove, listOpen: opts.listOpen ?? false },
        locked: false,
      };
      editorRef.current = next;
      setEditor(next);
      s.lock(target);
      // 편집기에 포커스를 주고 글자 끝에 커서(F2·더블클릭·그리드가 받은 첫 글자)
      requestAnimationFrame(() => {
        const el = input.current;
        if (!el || editorRef.current !== next) return;
        el.focus({ preventScroll: true });
        el.setSelectionRange(el.value.length, el.value.length);
      });
    },
    [s],
  );

  const close = useCallback(() => {
    editorRef.current = null;
    setEditor(null);
  }, []);

  const cancel = useCallback(() => {
    const e = editorRef.current;
    if (!e) return;
    close();
    s.unlock(e.at);
  }, [close, s]);

  /** 편집을 확정한다. 형식이 맞지 않으면 편집기에 오류를 보이고 오류 글을 돌려준다. */
  const commit = useCallback(
    (move: Move, override?: string): string | null => {
      const e = editorRef.current;
      if (!e) return null;
      const text = override ?? e.state.text;
      const n = normalize(columns[e.at.col]?.format ?? { kind: "text" }, text);
      if (!n.ok) {
        const next = { ...e, state: { ...e.state, text, error: n.error } };
        editorRef.current = next;
        setEditor(next);
        return n.error;
      }
      close();
      if (!s.edit([{ row: e.at.row, col: e.at.col, value: n.value }])) s.unlock(e.at); // 바뀐 게 없으면 잠금만 푼다
      if (move) grid.current?.step(move[0], move[1]);
      return null;
    },
    [close, columns, s],
  );

  /** 편집 중에 다른 곳으로 가면: 맞는 값은 저장, 아니면 취소하고 알린다. */
  const leave = useCallback(() => {
    const e = editorRef.current;
    if (!e) return;
    const error = commit(null);
    if (error) {
      cancel();
      const where = a1(model.origin.row + e.at.row, model.origin.col + e.at.col);
      notify(`${where}: ${error} — 입력을 취소했습니다`, "error");
    }
  }, [cancel, commit, model.origin, notify]);

  /** 사이드바 '셀 내용'에서 고치던 것을 지금 칸에 저장한다(`to`는 새로 고를 칸, 다른 시트면 `null`). */
  const leaveDetail = useCallback(
    (to: CellRef | null) => {
      const error = cellLeave.current?.(to);
      if (error) notify(error, "error");
    },
    [notify],
  );

  const onSelection = useCallback(
    (sel: GridSelection) => {
      const e = editorRef.current;
      if (e && (sel.anchor.row !== e.at.row || sel.anchor.col !== e.at.col)) leave();
      // 그리드는 누르는 순간 선택을 바꾼다: 사이드바에서 고치던 것은 바뀌기 전에 그 칸에 넣는다.
      leaveDetail(sel.anchor);
      setSelection(sel);
      s.cursor(sel.anchor);
      // 강조된 칸을 보러 왔으면 확인으로 표시한다.
      const { row, col } = sel.anchor;
      for (const h of s.highlights) {
        const r = h.range;
        if (
          !h.acked &&
          h.sheet === active &&
          row >= r.row &&
          row < r.row + r.rows &&
          col >= r.col &&
          col < r.col + r.cols
        )
          s.ackHighlight(h.id);
      }
    },
    [leave, leaveDetail, s, active],
  );

  /** 채팅의 '@ 셀 선택': 지금 시트에서 범위를 골라 `#시트!범위`로 넣는다(시트 탭을 바꿔도 이어진다). */
  const pick = useCellPick({
    start: () => selection?.anchor ?? { row: Math.min(model.headerRows, model.rows - 1), col: 0 },
    move: (from, dRow, dCol) => grid.current?.moveFrom(from, dRow, dCol) ?? from,
    describe: (r) => {
      const at = { ...r, row: model.origin.row + r.row, col: model.origin.col + r.col };
      return { token: cellRefToken(model.sheet, at), sheet: sheetPrefix(model.sheet), addr: formatRange(at) };
    },
    fromSheet: (r) => ({ ...r, row: r.row - model.origin.row, col: r.col - model.origin.col }),
    onBegin: () => {
      if (editorRef.current) leave();
    },
  });

  // 활성 칸이 입력 가능하면 투명 편집기에, 아니면 그리드에 포커스(그리드 안에서 움직일 때만).
  const overlayOn = mode === "grid" && at !== null && (editable || editor !== null);
  const atKey = at ? `${at.row}:${at.col}` : null;
  useEffect(() => {
    if (mode !== "grid" || !atKey) return;
    // 마우스로 칸을 누르면 브라우저가 이벤트가 끝난 뒤 그리드에 포커스를 주므로 한 프레임 뒤에 옮긴다.
    const id = requestAnimationFrame(() => {
      const active = document.activeElement;
      if (active && active !== document.body && !gridHost.current?.contains(active)) return;
      if (overlayOn) input.current?.focus({ preventScroll: true });
      else grid.current?.focus();
    });
    return () => cancelAnimationFrame(id);
  }, [atKey, overlayOn, mode]);

  // 잠금 결과·풀림(이 효과는 알림이 올 때만 돈다: 그때의 편집기·대기 목록을 본다)
  const { lockEvent } = s;
  const lockState = useRef({ waitFree, active });
  lockState.current = { waitFree, active };
  useEffect(() => {
    const { waitFree, active } = lockState.current;
    if (!lockEvent) return;
    const e = editorRef.current;
    const same =
      e !== null && lockEvent.sheet === active && e.at.row === lockEvent.at.row && e.at.col === lockEvent.at.col;
    if (lockEvent.kind === "granted" && same && !e.locked) {
      const next = { ...e, locked: true };
      editorRef.current = next;
      setEditor(next);
    } else if (lockEvent.kind === "denied" && same) {
      close();
      notify(lockEvent.by ? `${lockEvent.by} 님이 입력 중입니다` : (lockEvent.reason ?? "입력할 수 없습니다"), "warn");
    } else if (lockEvent.kind === "released") {
      if (same && e.locked) {
        close();
        notify("호스트가 편집 잠금을 해제했습니다. 입력하던 값은 저장되지 않았습니다", "warn");
      }
      const m = s.sheets[lockEvent.sheet]?.model;
      const key = m ? `${lockEvent.sheet}:${lockEvent.at.row * m.cols + lockEvent.at.col}` : "";
      if (m && waitFree.has(key)) {
        const where = a1(m.origin.row + lockEvent.at.row, m.origin.col + lockEvent.at.col);
        notify(`${s.sheets.length > 1 ? sheetPrefix(m.sheet) : ""}${where} 칸을 이제 입력할 수 있습니다`);
        setWaitFree((set) => {
          const next = new Set(set);
          next.delete(key);
          return next;
        });
      }
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [lockEvent, close, notify]);

  // 마감되면 편집을 닫는다.
  useEffect(() => {
    if (s.readOnly && editorRef.current) cancel();
  }, [s.readOnly, cancel]);

  // 편집기를 오래 열어 두면 서버가 잠금을 풀지 않도록 1분마다 다시 잡는다.
  const editingAt = editor ? `${editor.at.row}:${editor.at.col}` : null;
  useEffect(() => {
    if (!editingAt) return;
    const id = window.setInterval(() => {
      const e = editorRef.current;
      if (e) s.lock(e.at);
    }, 60_000);
    return () => window.clearInterval(id);
  }, [editingAt, s]);

  // ------------------------------------------------------------ 범위 작업

  const report = (what: string, r: BulkResult) => {
    const parts: string[] = [];
    if (r.invalid > 0) parts.push(`형식이 맞지 않는 ${r.invalid}칸${r.firstError ? `(${r.firstError})` : ""}`);
    if (r.skipped > 0) parts.push(`입력할 수 없는 ${r.skipped}칸`);
    if (parts.length > 0)
      notify(`${what}: ${parts.join(", ")}은 건너뛰었습니다`, r.changes.length > 0 ? "warn" : "error");
  };

  const blocked = () => {
    const now = Date.now();
    if (now - lastBlocked.current < 1500) return;
    lastBlocked.current = now;
    if (s.readOnly) notify("마감되어 입력할 수 없습니다", "warn");
    else if (locker) notify(`${locker} 님이 입력 중입니다`, "warn");
    else {
      notify("이 칸은 입력할 수 없습니다. 흰 바탕의 내 담당 칸에 입력해 주세요", "warn");
      setMineHint(true); // 말 대신 위치로도 알려 준다
    }
  };

  const undo = () => {
    if (!s.undo()) notify("되돌릴 내 변경이 없습니다");
  };
  const redo = () => {
    if (!s.redo()) notify("다시 실행할 변경이 없습니다");
  };

  const onKey = (e: KeyboardEvent) => {
    if (editorRef.current) return;
    const mod = e.ctrlKey || e.metaKey;
    if (mod && e.key.toLowerCase() === "z") {
      e.preventDefault();
      if (e.shiftKey) redo();
      else undo();
      return;
    }
    if (mod && e.key.toLowerCase() === "y") {
      e.preventDefault();
      redo();
      return;
    }
    if (!at || !selection) return;
    const current = sheet.value(at.row, at.col);
    if (e.key === "Delete") {
      e.preventDefault();
      const r = clearRange(ctx, rectOf(selection));
      s.edit(r.changes, "clear");
      if (r.changes.length === 0 && r.skipped > 0) blocked();
      return;
    }
    if (e.key === "Backspace" || e.key === "F2" || (e.altKey && e.key === "ArrowDown" && listItems)) {
      e.preventDefault();
      if (!editable) return blocked();
      if (e.key === "Backspace") startEdit(at, "", { arrowsMove: true });
      else startEdit(at, editText(current, rule?.format), { arrowsMove: false, listOpen: e.key === "ArrowDown" });
      return;
    }
    if (e.key.length === 1 && !mod && !e.altKey) {
      if (!editable) return blocked();
      // 편집기가 포커스를 받기 전에 그리드로 온 첫 글자(칸을 누르자마자 입력)
      e.preventDefault();
      startEdit(at, e.key, { arrowsMove: true });
    }
  };

  const onDouble = (row: number, col: number) => {
    const target = { row, col };
    if (!canEdit(ctx, row, col)) return blocked();
    startEdit(target, editText(sheet.value(row, col), columns[col]?.format), { arrowsMove: false });
  };

  const display = (r: number, c: number) => {
    const cell = model.cells[idx(r, c)];
    return displayValue(cell?.v, model.styles[cell?.s ?? 0]?.numberFormat ?? "", columns[c]?.format);
  };

  const onCopy = (e: ClipboardEvent, cut: boolean) => {
    if (editorRef.current || !selection) return;
    e.preventDefault();
    const rect = rectOf(selection);
    e.clipboardData.setData("text/plain", toTsv(copyRange(ctx, rect, display)));
    if (cut) {
      const r = clearRange(ctx, rect);
      s.edit(r.changes, "clear");
      report("잘라내기", r);
    }
  };

  const onPaste = (e: ClipboardEvent) => {
    if (editorRef.current || !selection) return;
    e.preventDefault();
    const r = pasteRange(ctx, rectOf(selection), parseTsv(e.clipboardData.getData("text/plain")));
    s.edit(r.changes, "paste");
    report("붙여넣기", r);
  };

  // ------------------------------------------------------------ 서식

  const policy = sv.permissions.format;
  const applyFormat = (patch: StylePatch) => {
    if (!selection) return;
    const rect = rectOf(selection);
    for (let r = rect.row; r < rect.row + rect.rows; r++) {
      for (let c = rect.col; c < rect.col + rect.cols; c++) {
        if (!canEdit(ctx, r, c) && !((flags[idx(r, c)] ?? 0) & FLAG.COVERED)) {
          return notify("내가 입력할 수 있는 칸만 서식을 바꿀 수 있습니다", "warn");
        }
      }
    }
    s.format(rect, patch);
  };
  const curStyle = at ? model.styles[model.cells[idx(at.row, at.col)]?.s ?? 0] : undefined;

  // 열 너비·행 높이(그 시트의 참여자 권한 '크기 조절'): 머리글 경계를 끌면 서버가 맞춘 크기를 모두에게 보내고(`resized`),
  // 그때 그린다. 머리글로 여러 열(행)을 골랐으면 모두(필터·남의 행 숨김으로 숨은 행은 건너뛴다).
  const canResize = sv.permissions.resize && !s.readOnly;
  const onResize = (axis: Axis, index: number, size: number) => {
    let sent = true;
    for (const { from, count } of resizeRuns(view, selection && rectOf(selection), axis, index))
      sent = s.resize(axis, from, count, size) && sent;
    if (!sent) notify("연결이 끊겨 크기를 바꾸지 못했습니다. 다시 연결되면 해 주세요", "warn");
  };

  // ------------------------------------------------------------ 취소 행·행 추가·행 삭제

  // 담당자를 배정하지 않은 시트에는 '내 담당만'이 없다(시트를 오가며 기억한 필터에 남아 있어도 끈다).
  useEffect(() => {
    if (!sv.assigned && filter.mineOnly) setFilter((f) => ({ ...f, mineOnly: false }));
  }, [sv.assigned, filter.mineOnly]);

  const cancelled = useMemo(() => new Set(model.cancelledRows ?? []), [model]);
  /** 취소 표시를 바꿀 행: 그리드면 고른 범위의 보이는 데이터 행(이어진 구간마다), 행 단위 입력이면 그 행. */
  const targetRuns = (): { from: number; count: number }[] => {
    if (mode === "form") return formRow !== null ? [{ from: formRow, count: 1 }] : [];
    if (!selection) return [];
    const r = rectOf(selection);
    const shown = new Set(view.hiddenRows);
    const out: { from: number; count: number }[] = [];
    for (let row = Math.max(r.row, model.headerRows); row < r.row + r.rows; row++) {
      if (shown.has(row)) continue;
      const last = out[out.length - 1];
      if (last && last.from + last.count === row) last.count++;
      else out.push({ from: row, count: 1 });
    }
    return out;
  };
  const activeRow = mode === "form" ? formRow : (at?.row ?? null);
  const activeCancelled = activeRow !== null && cancelled.has(activeRow);
  /** 고른 행을 취소 표시하거나 푼다(내가 입력할 수 있는 칸이 있는 행만). */
  const toggleCancel = (on: boolean, runs = targetRuns()) => {
    if (runs.length === 0) return notify("취소 표시할 데이터 행을 고르세요", "warn");
    for (const { from, count } of runs)
      for (let r = from; r < from + count; r++)
        if (!myRows[r]) return notify("내가 입력할 수 있는 행만 취소 표시할 수 있습니다", "warn");
    if (editorRef.current) leave();
    let sent = true;
    for (const { from, count } of runs) sent = s.cancelRows(from, count, on) && sent;
    if (!sent) notify("연결이 끊겨 취소 표시를 바꾸지 못했습니다. 다시 연결되면 해 주세요", "warn");
  };

  const addRow = () => {
    if (editorRef.current) leave();
    if (!s.addRows(1)) notify("연결이 끊겨 행을 추가하지 못했습니다. 다시 연결되면 해 주세요", "warn");
  };

  /** 내가 추가한 행인가(지울 수 있는 행). */
  const mine = (r: number) => (model.addedRows ?? []).some((a) => a.row === r && a.by === w.name);

  /** 강조 메모 메뉴(메모 숨기기·그 칸의 셀 이력)를 연다. `at`: 누른 칸(말풍선이면 강조의 머리 칸). */
  const openNoteMenu = (at: CellRef, e: ReactMouseEvent) => {
    e.preventDefault();
    if (editorRef.current) leave();
    setMenu({ x: e.clientX, y: e.clientY, hit: { kind: "cell", ...at } });
  };

  /**
   * 말풍선 위 오른쪽 클릭: 말풍선은 클릭이 아래(옆 칸·그리드 밖 빈 곳)로 지나가므로 위치로 찾아, 그리드보다 먼저
   * (capture) 메모 메뉴를 연다.
   */
  const onBubbleContext = (e: ReactMouseEvent) => {
    for (const b of gridHost.current?.querySelectorAll<HTMLElement>(".callout[data-hl]") ?? []) {
      const r = b.getBoundingClientRect();
      if (e.clientX < r.left || e.clientX > r.right || e.clientY < r.top || e.clientY > r.bottom) continue;
      const h = [...highlighted.values()].find((x) => x.id === Number(b.dataset.hl));
      if (!h) continue;
      e.stopPropagation();
      return openNoteMenu(h.at, e);
    }
  };

  // 칸 오른쪽 클릭 → 셀 이력(메모가 있는 강조 칸이면 메모 숨기기·셀 이력 메뉴). 행·열 머리글 → 크기·행 추가·행 삭제·
  // 취소 표시 메뉴(할 수 있는 것이 있을 때만).
  const onGridContext = (hit: GridHit, e: ReactMouseEvent) => {
    if (hit.kind === "cell") {
      if (highlighted.get(`${hit.row}:${hit.col}`)?.note) return openNoteMenu(hit, e);
      e.preventDefault();
      grid.current?.reveal({ row: hit.row, col: hit.col });
      showPanel("history");
      return;
    }
    const p = sv.permissions;
    const any =
      hit.kind === "col"
        ? canResize
        : canResize || p.addRows || p.deleteRows || (hit.row >= model.headerRows && !s.readOnly);
    if (!any) return;
    e.preventDefault();
    if (editorRef.current) leave();
    const sel = selectForMenu(view, selection, hit);
    if (sel !== selection) onSelection(sel);
    setMenu({ x: e.clientX, y: e.clientY, hit });
  };

  const headerMenu = (m: HeaderMenu) => {
    const close = () => setMenu(null);
    const r = selection ? rectOf(selection) : null;
    if (m.hit.kind === "cell") {
      const at = { row: m.hit.row, col: m.hit.col };
      const note = highlighted.get(`${at.row}:${at.col}`);
      if (!note) return null; // 메뉴를 연 사이에 호스트가 해제했다
      const hidden = hiddenNotes.has(note.id);
      return (
        <ContextMenu x={m.x} y={m.y} onClose={close} label="칸 메뉴">
          <MenuNote title>{addr(note.at)} 강조 메모</MenuNote>
          <MenuItem
            title={hidden ? "말풍선을 다시 보입니다" : "이 화면에서만 말풍선을 숨깁니다(노란 강조는 남습니다)"}
            onClick={() => {
              close();
              toggleNote(note.id);
            }}
          >
            {hidden ? "메모 다시 보기" : "메모 숨기기"}
          </MenuItem>
          <MenuSep />
          <MenuItem
            onClick={() => {
              close();
              grid.current?.reveal(at);
              showPanel("history");
            }}
          >
            셀 이력 보기
          </MenuItem>
        </ContextMenu>
      );
    }
    if (m.hit.kind === "col") {
      const c = m.hit.col;
      const count = r && r.cols > 1 && c >= r.col && c < r.col + r.cols ? r.cols : 1;
      return (
        <ContextMenu x={m.x} y={m.y} onClose={close} label="열 메뉴">
          <MenuNote title>
            {colLetters(model.origin.col + (count > 1 ? r!.col : c))}
            {count > 1 ? `~${colLetters(model.origin.col + r!.col + count - 1)}` : ""}열
          </MenuNote>
          <SizeControl
            axis="col"
            value={view.colWidths[c] ?? 48}
            count={count}
            onCommit={(pt) => onResize("col", c, pt)}
          />
        </ContextMenu>
      );
    }
    const row = m.hit.row;
    // 행 머리글로 고른 범위 안이면 그 행들 모두, 아니면 누른 행 하나.
    const whole = r !== null && r.col === 0 && r.cols === model.cols && row >= r.row && row < r.row + r.rows;
    const from = whole ? r.row : row;
    const count = whole ? r.rows : 1;
    const rows = Array.from({ length: count }, (_, k) => from + k);
    const data = rows.every((x) => x >= model.headerRows);
    const label = (x: number) => model.origin.row + x + 1;
    const p = sv.permissions;
    const deletable = data && rows.every(mine);
    const doDelete = () => {
      const filled = rows.reduce(
        (n, x) =>
          n +
          Array.from({ length: model.cols }, (_, c) => c).filter((c) => {
            const v = model.cells[idx(x, c)]?.v;
            return (flags[idx(x, c)] ?? 0) & FLAG.MINE && v !== undefined && v !== null && v !== "";
          }).length,
        0,
      );
      if (filled > 0 && m.confirm === undefined) return setMenu({ ...m, confirm: filled });
      if (!s.deleteRows(from, count)) notify("연결이 끊겨 행을 지우지 못했습니다. 다시 연결되면 해 주세요", "warn");
      close();
    };
    const allCancelled = rows.every((x) => cancelled.has(x));
    return (
      <ContextMenu x={m.x} y={m.y} onClose={close} label="행 메뉴">
        <MenuNote title>{count > 1 ? `${label(from)}~${label(from + count - 1)}행` : `${label(row)}행`}</MenuNote>
        {m.confirm !== undefined ? (
          <>
            <MenuNote>값이 있는 칸 {m.confirm}개도 함께 지워집니다. 지울까요?</MenuNote>
            <div className="mmg-menu-actions">
              <button type="button" className="danger" data-menu-focus onClick={doDelete}>
                행 삭제
              </button>
              <button type="button" onClick={close}>
                취소
              </button>
            </div>
          </>
        ) : (
          <>
            {canResize && (
              <SizeControl
                axis="row"
                value={view.rowHeights[row] ?? 15}
                count={count}
                onCommit={(pt) => onResize("row", row, pt)}
              />
            )}
            {canResize && (p.addRows || p.deleteRows || data) && <MenuSep />}
            {p.addRows && (
              <MenuItem
                disabled={s.readOnly}
                onClick={() => {
                  close();
                  addRow();
                }}
              >
                ＋ 표 맨 아래에 행 추가
              </MenuItem>
            )}
            {data && (
              <MenuItem
                disabled={s.readOnly || !rows.every((x) => myRows[x])}
                title={rows.every((x) => myRows[x]) ? undefined : "내가 입력할 수 있는 행만 취소 표시할 수 있습니다"}
                onClick={() => {
                  close();
                  toggleCancel(!allCancelled, [{ from, count }]);
                }}
              >
                {allCancelled ? "취소 표시 풀기" : "취소 행으로 표시"}
              </MenuItem>
            )}
            {p.deleteRows && (
              <MenuItem
                danger
                disabled={s.readOnly || !deletable}
                title={deletable ? undefined : "내가 추가한 행만 지울 수 있습니다"}
                onClick={doDelete}
              >
                행 삭제{count > 1 ? ` (${count}행)` : ""}
              </MenuItem>
            )}
          </>
        )}
      </ContextMenu>
    );
  };

  // ------------------------------------------------------------ 그리기

  const colorOf = (name: string) => personColor(Math.max(0, w.roster.indexOf(name)));

  // 다른 사람 표시(그리드 위 겹): 편집 잠금이 있으면 그 칸에 자물쇠 이름표, 없으면 커서. 머리글 칸 커서는 테두리만.
  // 잠금은 편집 상태 안에서 바뀌므로(sheet.locks) 그릴 때마다 만든다(사람 수만큼이라 가볍다).
  const marks: GridMark[] = [];
  const editingNow = new Set<string>();
  for (const [i, name] of sheet.locks) {
    editingNow.add(name);
    marks.push({
      key: name,
      row: Math.floor(i / model.cols),
      col: i % model.cols,
      color: colorOf(name),
      label: name,
      lock: true,
    });
  }
  for (const p of s.presence) {
    if (p.host || p.name === w.name || !p.cursor || p.cursor.sheet !== active || editingNow.has(p.name)) continue;
    const header = (flags[idx(p.cursor.row, p.cursor.col)] ?? 0) & FLAG.HEADER;
    marks.push({
      key: p.name,
      row: p.cursor.row,
      col: p.cursor.col,
      color: personColor(p.color),
      label: header ? undefined : p.name,
    });
  }

  const gridRanges: GridRange[] = [...pick.ranges];
  if (ping && ping.sheet === active) gridRanges.push({ key: `ping${ping.seq}`, range: ping.range, kind: "ping" });

  /** 입력할 수 없는 칸에 쓰려 하면 잠깐 내 담당 칸을 깜빡인다(두 번 깜빡이는 동안만 둔다). */
  const [mineHint, setMineHint] = useState(false);
  useEffect(() => {
    if (!mineHint) return;
    const id = window.setTimeout(() => setMineHint(false), 1000);
    return () => window.clearTimeout(id);
  }, [mineHint]);

  // 강조된 칸: 칸 → (강조 번호, 메모, 왼쪽 위 칸인지)
  const highlighted = useMemo(() => {
    const map = new Map<string, { id: number; note: string; head: boolean; at: CellRef }>();
    for (const h of s.highlights) {
      if (h.sheet !== active) continue;
      const at = { row: h.range.row, col: h.range.col };
      for (let r = h.range.row; r < h.range.row + h.range.rows; r++)
        for (let c = h.range.col; c < h.range.col + h.range.cols; c++)
          map.set(`${r}:${c}`, { id: h.id, note: h.note, head: r === at.row && c === at.col, at });
    }
    return map;
  }, [s.highlights, active]);

  // ------------------------------------------------------------ 시트

  /** 시트마다 보던 칸·필터(시트를 오가도 이어서 본다). */
  const memory = useRef(new Map<number, { selection: GridSelection | null; filter: RowFilter }>());
  /** 다른 시트의 칸으로 가는 중: 시트를 바꾼 뒤 그 칸을 보인다. */
  const [pendingReveal, setPendingReveal] = useState<{ sheet: number; range: CellRange } | null>(null);

  const switchSheet = (to: number) => {
    if (to === active || !s.sheets[to]) return;
    if (editorRef.current) leave(); // 확정·취소는 지금 시트로 간다
    leaveDetail(null);
    memory.current.set(active, { selection, filter });
    const back = memory.current.get(to);
    setSelection(back?.selection ?? null);
    setFilter(back?.filter ?? NO_FILTER);
    setFormRow(null);
    s.setActive(to);
    s.cursor(back?.selection?.anchor ?? null);
    pick.reset();
  };

  const othersRows = useMemo(() => new Set(model.hiddenRows), [model]);

  /**
   * 그 범위(그 시트의 모델 좌표)를 선택한다. 다른 시트면 시트를 바꾸고, 필터에 걸려 숨은 행이 있으면 필터를 지운다.
   * 앞뒤로 걸친 남의 행은 잘라 내고 보이는 부분만 고른다.
   */
  const revealRange = (range: CellRange, highlight?: number, target: number = active) => {
    if (highlight !== undefined) s.ackHighlight(highlight);
    if (mode !== "grid") setMode("grid");
    if (target !== active) {
      if (!s.sheets[target]) return;
      switchSheet(target);
      setPendingReveal({ sheet: target, range });
      return;
    }
    const inSheet = clampTo(model, range);
    const shown = inSheet && trimHiddenRows(inSheet, (r) => othersRows.has(r));
    if (!inSheet || !shown) {
      notify("그 칸은 이 화면에서 볼 수 없습니다(남의 행)", "warn");
      return;
    }
    if (shown.rows < inSheet.rows) {
      const where = formatRange({ ...shown, row: model.origin.row + shown.row, col: model.origin.col + shown.col });
      notify(`일부 행은 볼 수 없어 보이는 부분(${where})만 선택했습니다`, "warn");
    }
    for (let r = shown.row; r < shown.row + shown.rows; r++)
      if (hidden.has(r)) {
        setFilter(NO_FILTER);
        break;
      }
    const last = { row: shown.row + shown.rows - 1, col: shown.col + shown.cols - 1 };
    requestAnimationFrame(() => grid.current?.select({ anchor: { row: shown.row, col: shown.col }, focus: last }));
    setPing({ sheet: active, range: shown, seq: ++pingSeq.current });
  };

  // 시트를 바꾼 뒤(새 시트를 그린 뒤) 가려던 칸을 보인다.
  useEffect(() => {
    if (!pendingReveal || pendingReveal.sheet !== active) return;
    const range = pendingReveal.range;
    setPendingReveal(null);
    revealRange(range);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pendingReveal, active]);

  /** 채팅의 `#시트!F7`: 시트 이름이 있으면 그 시트(없는 이름이면 알림), 없으면 지금 시트. */
  const revealRef = (range: CellRange, sheetName?: string) => {
    const target = sheetName === undefined ? active : s.sheets.findIndex((x) => x.model.sheet === sheetName);
    if (target < 0) return notify(`'${sheetName}' 시트가 없습니다`, "warn");
    const m = s.sheets[target]!.model;
    revealRange(
      { row: range.row - m.origin.row, col: range.col - m.origin.col, rows: range.rows, cols: range.cols },
      undefined,
      target,
    );
  };

  const decorate = (r: number, c: number): CellDecoration | undefined => {
    const i = idx(r, c);
    const f = flags[i] ?? 0;
    if (f & FLAG.HEADER) return undefined;
    const cls: string[] = [];
    if (!myRows[r]) cls.push("deco-others");
    else if (!(f & FLAG.INPUT)) cls.push("deco-locked");
    else if (!(f & FLAG.MINE)) cls.push("deco-others");
    const v = model.cells[i]?.v;
    if (f & FLAG.MINE && f & FLAG.REQUIRED && (v === undefined || v === null || v === "")) cls.push("deco-missing");
    if (sheet.pending.has(i)) cls.push("deco-pending");
    if (mineHint && myRows[r] && f & FLAG.INPUT && f & FLAG.MINE) cls.push("deco-mine-hint");
    let adornment: CellDecoration["adornment"];
    const hl = highlighted.get(`${r}:${c}`);
    if (hl) {
      cls.push("deco-highlight");
      // 메모가 있으면 왼쪽 위 칸에 말풍선으로 보인다(없으면 강조색만). 내가 숨긴 메모는 모서리 표시만 남긴다.
      if (hl.head && hl.note) {
        if (hiddenNotes.has(hl.id)) adornment = <span className="note-mark" aria-hidden />;
        else {
          cls.push("deco-overflow");
          adornment = (
            <span className="callout" data-hl={hl.id}>
              {hl.note}
            </span>
          );
        }
      }
    }
    // 다른 사람이 방금 바꾼 칸은 그 사람 색으로 잠깐 물든다(내 편집에는 쓰지 않는다).
    const recent = s.recentEdits.get(`${active}:${r}:${c}`);
    if (recent) {
      const who = w.roster.indexOf(recent.by);
      const tint = who >= 0 ? ({ "--flash": personColor(who) } as CSSProperties) : undefined;
      adornment = (
        <>
          <span key={recent.seq} className="cell-flash" style={tint} />
          {adornment}
        </>
      );
    }
    return { className: cls.join(" "), adornment };
  };

  const cellText = (_r: number, c: number, cell: { v?: unknown } | undefined, nf: string) => {
    const format = columns[c]?.format;
    return format?.kind === "date" ? displayValue(cell?.v as never, nf, format) : undefined;
  };

  const rowMarker = (r: number) => {
    const p = sv.rowOwner[r] ?? -1;
    return p >= 0 ? personColor(p) : undefined;
  };

  useEffect(() => {
    if (!new URLSearchParams(location.search).has("bench")) return;
    const scroller = gridHost.current?.querySelector<HTMLElement>(".mmg-scroller");
    if (scroller) void runScrollBench(scroller).then(setBench);
  }, []);

  const openForm = () => {
    if (editorRef.current) leave();
    const row = at && myRows[at.row] ? at.row : myRows.indexOf(1);
    setFormRow(row >= 0 ? row : null);
    setMode("form");
  };

  const showInGrid = (row: number) => {
    setMode("grid");
    let col = 0;
    for (let c = 0; c < model.cols; c++) {
      if ((flags[idx(row, c)] ?? 0) & FLAG.MINE) {
        col = c;
        break;
      }
    }
    if (hidden.has(row)) setFilter(NO_FILTER);
    requestAnimationFrame(() => grid.current?.reveal({ row, col }));
  };

  // 내가 행을 추가했으면 새 행(내가 입력할 첫 칸)으로 간다. 새 시트(welcome)를 받은 뒤에 온다.
  const { rowsAdded } = s;
  const rowsAddedSeen = useRef(0);
  useEffect(() => {
    if (!rowsAdded || rowsAdded.seq === rowsAddedSeen.current) return;
    rowsAddedSeen.current = rowsAdded.seq;
    if (rowsAdded.by !== w.name || rowsAdded.sheet !== active) return;
    const row = rowsAdded.range.row;
    const where = model.origin.row + row + 1;
    notify(
      `표 맨 아래에 ${rowsAdded.range.rows > 1 ? `${rowsAdded.range.rows}개 행` : "행"}을 추가했습니다(${where}행)`,
    );
    if (mode === "form") setFormRow(row);
    else {
      // 행 추가 단추(＋, 행 번호 메뉴)에 있던 포커스를 놓아 새 칸으로 가면 곧바로 입력할 수 있게 한다.
      const focused = document.activeElement;
      if (
        focused instanceof HTMLElement &&
        (!gridHost.current?.contains(focused) || focused.closest(".mmg-rowfoot") !== null)
      )
        focused.blur();
      showInGrid(row);
    }
    // 새 행을 받은 시트(model)가 바뀐 뒤 한 번만 움직인다.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [rowsAdded, model]);

  const unreadAlerts = s.highlights.filter((h) => !h.acked).length + s.announcements.filter((n) => !n.read).length;
  const unreadChat = s.chat.filter((m) => m.id > chatSeen && !(m.from === w.name && !m.host)).length;
  /** 사이드바에 그 탭이 보이고 있다. */
  const tabShown = (t: PanelTab) => sideOpen && mode === "grid" && panel === t;
  /** 도구바의 알림·채팅: 그 탭을 연다(이미 보고 있으면 사이드바를 닫는다). */
  const openTab = (t: PanelTab) => {
    if (mode !== "grid") setMode("grid");
    showPanel(t);
  };
  const jumpTo = (t: PanelTab) => (tabShown(t) ? setSideOpen(false) : openTab(t));
  const others = s.presence.filter((p) => p.name !== w.name);
  const p = s.progress ?? { name: w.name, rows: 0, required: 0, filled: 0 };
  const pct = p.required > 0 ? Math.round((p.filled / p.required) * 100) : 100;
  const waiting = s.sheets.reduce((n, x) => n + x.outbox.reduce((k, b) => k + b.cells.length, 0), 0);
  // '저장 중'은 답이 늦을 때만(400ms 넘게 대기할 때) 보인다. 편집마다 상태 글자가 깜빡이지 않게.
  const [slowSave, setSlowSave] = useState(false);
  useEffect(() => {
    if (waiting === 0) return setSlowSave(false);
    const id = window.setTimeout(() => setSlowSave(true), 400);
    return () => window.clearTimeout(id);
  }, [waiting > 0]); // eslint-disable-line react-hooks/exhaustive-deps
  const myChanges = s.sheets.reduce((n, x) => n + x.myChanges, 0);
  const lastSaved = Math.max(0, ...s.sheets.map((x) => x.lastSaved ?? 0)) || null;
  const shownRows = model.rows - model.headerRows - view.hiddenRows.filter((r) => r >= model.headerRows).length;

  const editorNode = overlayOn && at && (
    <CellEditor
      inputRef={input}
      state={editor?.state ?? null}
      items={listItems}
      placeholder={
        rule?.format.kind === "date"
          ? "YYYY-MM-DD"
          : rule?.format.kind === "number" && rule.format.unit
            ? `숫자 (${rule.format.unit})`
            : undefined
      }
      onStart={(text) => startEdit(at, text, { arrowsMove: true })}
      onText={(text) => {
        const e = editorRef.current;
        if (!e) return;
        const next = { ...e, state: { ...e.state, text, error: undefined } };
        editorRef.current = next;
        setEditor(next);
      }}
      onCommit={commit}
      onLeave={leave}
      onCancel={cancel}
      onToggleList={() => {
        const e = editorRef.current;
        if (e) {
          const next = { ...e, state: { ...e.state, listOpen: !e.state.listOpen } };
          editorRef.current = next;
          setEditor(next);
        } else if (editable) {
          startEdit(at, editText(sheet.value(at.row, at.col), rule?.format), { arrowsMove: false, listOpen: true });
        }
      }}
    />
  );

  return (
    <div className="page">
      <header className="top">
        <div className="top-title">
          <div className="eyebrow">
            <a className="back-link" href="/" title="호스트가 공개한 다른 취합 업무">
              업무 목록
            </a>
            {" · "}
            {w.session.requester ? `${w.session.requester} 님의 취합 요청` : "취합 요청"}
          </div>
          <h1>{w.session.title}</h1>
        </div>
        <span className="spacer" />
        {p.required > 0 && (
          <span className={`mine-progress${pct >= 100 ? " done" : ""}`} title="내가 채워야 할 필수 칸">
            <span className="label">내 필수 입력</span>
            <b>
              {p.filled.toLocaleString()} / {p.required.toLocaleString()}
            </b>
            <span className="bar">
              <span style={{ width: `${pct}%` }} />
            </span>
            <span className="pct">{pct}%</span>
          </span>
        )}
        <span className="avatars" aria-label="접속 중">
          {others.map((x) => (
            <span
              key={x.name}
              className={`avatar${x.status === "away" ? " away" : ""}${x.host ? " host" : ""}`}
              title={`${x.name}${x.host ? " (호스트)" : ""}${x.status === "away" ? " · 자리 비움" : ""}`}
              style={{ borderColor: x.host ? "var(--ink)" : personColor(x.color) }}
            >
              {x.name.slice(0, 1)}
            </span>
          ))}
          <span
            className="avatar me"
            style={{ background: personColor(w.color), borderColor: personColor(w.color) }}
            title={`${w.name} (나)`}
          >
            나
          </span>
        </span>
        <DeadlineClock deadline={w.session.deadline} dueAt={w.session.deadlineAt} offset={s.clockOffset} />
      </header>

      {s.readOnly && (
        <div className="readonly-bar" role="status">
          {s.readOnlyReason ?? "마감되었습니다"} — 읽기 전용입니다. 입력한 값은 호스트가 취합합니다.
        </div>
      )}
      <NoticeBanners key={job} notices={s.announcements} onAck={s.ackNotice} />

      <div className="toolbar">
        <div className="tool-group">
          <button className="ghost tool" onClick={undo} disabled={!sheet.canUndo()} title="내 변경 되돌리기 (Ctrl+Z)">
            <ToolIcon d="M9 14 4 9l5-5M4 9h10.5a5.5 5.5 0 0 1 0 11H11" />
            실행 취소
          </button>
        </div>
        <span className="sep" />
        <div className="tool-group" role="group" aria-label="서식">
          <button
            className={`ghost icon${curStyle?.bold ? " on" : ""}`}
            aria-label="굵게"
            disabled={policy !== "all" || s.readOnly || mode !== "grid"}
            onClick={() => applyFormat({ bold: !curStyle?.bold })}
          >
            <b>B</b>
          </button>
          <button
            className={`ghost icon${curStyle?.italic ? " on" : ""}`}
            aria-label="기울임"
            disabled={policy !== "all" || s.readOnly || mode !== "grid"}
            onClick={() => applyFormat({ italic: !curStyle?.italic })}
          >
            <i>I</i>
          </button>
          <details className={`fill-menu${policy === "none" || s.readOnly || mode !== "grid" ? " disabled" : ""}`}>
            <summary aria-label="채우기">채우기 ▾</summary>
            <div className="swatches">
              {FILLS.map((x) => (
                <button
                  key={x.label}
                  className="swatch"
                  title={x.rgb === null ? x.label : `${x.label} ${rgbHex(x.rgb)}`}
                  aria-label={x.label}
                  style={{ background: x.rgb === null ? "#fff" : rgbHex(x.rgb) }}
                  onClick={(e) => {
                    (e.currentTarget.closest("details") as HTMLDetailsElement).open = false;
                    applyFormat({ fill: x.rgb === null ? { kind: "none" } : { kind: "color", rgb: x.rgb } });
                  }}
                >
                  {x.rgb === null ? "✕" : ""}
                </button>
              ))}
            </div>
          </details>
          <details
            className={`fill-menu text-color${policy !== "all" || s.readOnly || mode !== "grid" ? " disabled" : ""}`}
          >
            <summary aria-label="글자색" title="글자색">
              <span className="text-color-mark" style={{ borderColor: rgbHex(curStyle?.color ?? 0) }}>
                가
              </span>{" "}
              ▾
            </summary>
            <div className="swatches">
              {TEXT_COLORS.map((x) => (
                <button
                  key={x.label}
                  className="swatch text-swatch"
                  title={`${x.label} ${rgbHex(x.rgb)}`}
                  aria-label={`글자 ${x.label}`}
                  style={{ color: rgbHex(x.rgb) }}
                  onClick={(e) => {
                    (e.currentTarget.closest("details") as HTMLDetailsElement).open = false;
                    applyFormat({ color: x.rgb });
                  }}
                >
                  가
                </button>
              ))}
            </div>
          </details>
          {ALIGNS.map((x) => {
            const on = curStyle?.hAlign === x.value;
            return (
              <button
                key={x.value}
                className={`ghost icon${on ? " on" : ""}`}
                aria-label={x.label}
                aria-pressed={on}
                title={`${x.label}${on ? " (다시 누르면 일반)" : ""}`}
                disabled={policy !== "all" || s.readOnly || mode !== "grid"}
                onClick={() => applyFormat({ hAlign: on ? "general" : x.value })}
              >
                {x.icon}
              </button>
            );
          })}
        </div>
        {policy !== "all" && (
          <span
            className="tool-hint"
            title={policy === "none" ? "서식은 호스트가 관리합니다" : "배경색만 바꿀 수 있습니다"}
          >
            <ToolIcon d="M7 11V8a5 5 0 0 1 10 0v3M5 11h14v10H5z" size={13} />
            <span className="text">
              {policy === "none" ? "서식은 호스트가 관리합니다" : "배경색만 바꿀 수 있습니다"}
            </span>
          </span>
        )}
        <span className="sep" />
        <div className="tool-group">
          <button
            className={`ghost tool cancel-toggle${activeCancelled ? " on" : ""}`}
            aria-pressed={activeCancelled}
            disabled={s.readOnly || activeRow === null || activeRow < model.headerRows}
            title={
              activeCancelled
                ? "고른 행의 취소 표시를 풉니다"
                : "고른 행 전체를 취소선·회색으로 표시합니다(필수 칸으로 세지 않습니다)"
            }
            onClick={() => toggleCancel(!activeCancelled)}
          >
            <s>취소 행</s>
          </button>
        </div>
        <span className="spacer" />
        <div className="mode-tabs" role="tablist" aria-label="입력 방식">
          <button role="tab" aria-selected={mode === "grid"} onClick={() => setMode("grid")}>
            그리드
          </button>
          <button role="tab" aria-selected={mode === "form"} onClick={openForm}>
            행 단위
          </button>
        </div>
        <span className="sep" />
        <div className="tool-group" role="group" aria-label="사이드바">
          <button
            className={`ghost tool jump${tabShown("alerts") ? " on" : ""}`}
            aria-pressed={tabShown("alerts")}
            title="강조된 셀 · 공지"
            onClick={() => jumpTo("alerts")}
          >
            <ToolIcon d="M6 8a6 6 0 0 1 12 0c0 7 3 9 3 9H3s3-2 3-9M10.3 21a1.9 1.9 0 0 0 3.4 0" />
            알림
            {unreadAlerts > 0 && (
              <span key={unreadAlerts} className="badge">
                {unreadAlerts}
              </span>
            )}
          </button>
          <button
            className={`ghost tool jump${tabShown("chat") ? " on" : ""}`}
            aria-pressed={tabShown("chat")}
            title="채팅(전체 · 호스트에게)"
            onClick={() => jumpTo("chat")}
          >
            <ToolIcon d="M21 12a8 8 0 0 1-11.6 7.1L4 20l1.1-4.4A8 8 0 1 1 21 12z" />
            채팅
            {unreadChat > 0 && !tabShown("chat") && (
              <span key={unreadChat} className="badge">
                {unreadChat}
              </span>
            )}
          </button>
          <button
            className={`ghost tool icon side-toggle${sideOpen && mode === "grid" ? " on" : ""}`}
            aria-pressed={sideOpen && mode === "grid"}
            aria-label="사이드바"
            title="오른쪽 사이드바 열기·닫기: 셀 내용 · 알림 · 채팅 · 셀 이력"
            onClick={() => {
              if (mode !== "grid") {
                setMode("grid");
                setSideOpen(true);
              } else setSideOpen(!sideOpen);
            }}
          >
            <ToolIcon d="M4 4h16v16H4zM15 4v16" />
          </button>
        </div>
      </div>

      <ConnectionBar connected={s.connected} waiting={waiting} />

      {mode === "grid" ? (
        <>
          <div className="filter-bar">
            {filterCols.length > 0 && <span className="tag tag-filter">조회조건</span>}
            {filterCols.map((c) => (
              <span key={c} className="filter">
                {labels[c] || `열 ${c + 1}`}:
                <SearchSelect
                  label={labels[c] ?? ""}
                  value={filter.values[c] ?? NO_VALUES}
                  options={options.get(c) ?? NO_VALUES}
                  onChange={(v) =>
                    setFilter((f) => {
                      const values = { ...f.values };
                      if (v.length > 0) values[c] = v;
                      else delete values[c];
                      return { ...f, values };
                    })
                  }
                />
              </span>
            ))}
            {sv.assigned && (
              <label className="check" title="담당자가 정해지지 않은 행은 누구나 입력할 수 있어 함께 보입니다">
                <input
                  type="checkbox"
                  checked={filter.mineOnly}
                  onChange={(e) => setFilter({ ...filter, mineOnly: e.target.checked })}
                />
                내 담당만 <span className="muted small">(담당자 없는 행 포함)</span>
              </label>
            )}
            <label className="check">
              <input
                type="checkbox"
                checked={filter.missingOnly}
                onChange={(e) => setFilter({ ...filter, missingOnly: e.target.checked })}
              />
              필수 미입력만
            </label>
            {isFiltering(filter) && (
              <button className="link" onClick={() => setFilter(NO_FILTER)}>
                필터 지우기
              </button>
            )}
            <span className="spacer" />
            <span className="muted small" data-testid="row-count">
              {(model.rows - model.headerRows).toLocaleString()}행
              {sv.assigned ? ` · 내 담당 ${myRowCount.toLocaleString()}행` : ""}
              {isFiltering(filter) ? ` · 표시 ${shownRows.toLocaleString()}행` : ""}
            </span>
          </div>

          <div className="work-area">
            <div
              className="grid-host"
              ref={gridHost}
              // 칸을 누르면 브라우저가 그리드에 준 포커스를 바로 편집기로 옮긴다(곧바로 입력해도 첫 글자를 받는다).
              onClick={() => {
                // 채팅에 넣을 범위를 골랐으면 초점은 채팅 입력 칸에 그대로 둔다.
                const now = document.activeElement;
                const here = !now || now === document.body || gridHost.current?.contains(now);
                if (overlayOn && !editorRef.current && here) input.current?.focus({ preventScroll: true });
              }}
              onCopy={(e) => onCopy(e, false)}
              onCut={(e) => onCopy(e, true)}
              onPaste={onPaste}
              onContextMenuCapture={onBubbleContext}
            >
              <SheetGrid
                key={active}
                handle={grid}
                model={view}
                decorate={decorate}
                marks={marks}
                ranges={gridRanges}
                pick={pick.grid}
                rowMarker={rowMarker}
                cellText={cellText}
                selection={selection}
                onSelectionChange={onSelection}
                overlay={editorNode ? { at: editor?.at ?? at!, node: editorNode } : null}
                onKey={onKey}
                onCellDoubleClick={onDouble}
                onResize={canResize ? onResize : undefined}
                onContextMenu={onGridContext}
                rowFooter={
                  sv.permissions.addRows ? (
                    <button
                      type="button"
                      className="mmg-add-row"
                      aria-label="행 추가"
                      title="표 맨 아래에 행 추가"
                      disabled={s.readOnly}
                      onClick={addRow}
                    >
                      ＋
                    </button>
                  ) : undefined
                }
              />
              {pick.active && <PickHint />}
            </div>
            {sideOpen && (
              <SidePanel
                s={s}
                slide={sideSlide}
                tab={panel}
                onTab={setPanelTab}
                onClose={() => setSideOpen(false)}
                width={sideWidth}
                onWidth={setSideWidth}
                at={at}
                labels={labels}
                columns={columns}
                flags={flags}
                canEdit={(r, c) => canEdit(ctx, r, c)}
                locker={locker}
                waiting={at !== null && waitFree.has(`${active}:${idx(at.row, at.col)}`)}
                onWait={() => at && setWaitFree((set) => new Set(set).add(`${active}:${idx(at.row, at.col)}`))}
                cellLeave={cellLeave}
                unreadChat={unreadChat}
                onReveal={revealRange}
                onRef={revealRef}
                onChatSeen={onChatSeen}
                chatSeen={chatSeen}
                pick={pick}
                chatDraft={chatDraft}
                onChatDraft={setChatDraft}
                chatToHost={chatToHost}
                onChatToHost={setChatToHost}
                hiddenNotes={hiddenNotes}
                onToggleNote={toggleNote}
              />
            )}
          </div>
        </>
      ) : (
        <RowForm
          s={s}
          myRows={myRows}
          row={formRow}
          onRow={setFormRow}
          onShowInGrid={showInGrid}
          onAddRow={sv.permissions.addRows ? addRow : undefined}
        />
      )}

      {s.sheets.length > 1 && <SheetTabs s={s} onSelect={switchSheet} keepFocus={pick.active} />}
      {menu && mode === "grid" && headerMenu(menu)}

      <footer className="status">
        {!s.connected ? (
          <span className="warn">
            ○ 오프라인
            {waiting > 0 ? ` — 입력 ${waiting}건이 대기 중입니다(다시 연결되면 보냅니다)` : " — 다시 연결하는 중…"}
          </span>
        ) : waiting > 0 && slowSave ? (
          <span className="muted">◌ 저장 중… ({waiting}건)</span>
        ) : (
          <span className="ok">● 실시간 동기화</span>
        )}
        <span>
          {w.name} 님 · 내 변경 {myChanges.toLocaleString()}건{lastSaved ? ` · 마지막 저장 ${clock(lastSaved)}` : ""}
        </span>
        <span className="spacer" />
        {editor ? (
          <span className="muted">{addr(editor.at)} 편집 중 — 다른 사람에게 잠김으로 보입니다</span>
        ) : (
          <span className="muted">
            Enter·Tab 이동 · F2 편집 · Delete 지우기 · Ctrl+Z 되돌리기 · Excel에서 복사해 붙여넣기
            {canResize ? " · 머리글 경계를 끌어 크기 조절" : ""}
          </span>
        )}
      </footer>

      <div
        className={`toasts${sideOpen && mode === "grid" ? " with-panel" : ""}`}
        style={{ "--side-w": `${sideWidth}px` } as CSSProperties}
        aria-live="polite"
      >
        {s.notices.map((n) => (
          <Toast
            key={n.id}
            notice={n}
            onDone={s.dismiss}
            onAction={(a) => ("chat" in a ? openTab("chat") : revealRange(a.range, a.highlight, a.sheet ?? active))}
          />
        ))}
      </div>
      {bench && (
        <pre className="bench" data-testid="bench">
          {JSON.stringify(bench)}
        </pre>
      )}
    </div>
  );
}

/**
 * 연결이 끊긴 동안 그리드 위에 띠로 알린다(입력은 이 기기에 모아 두었다가 다시 연결되면 보낸다). 다시 연결되면
 * 2초 동안 알리고 접는다. 하단 상태줄은 그대로 둔다.
 */
function ConnectionBar({ connected, waiting }: { connected: boolean; waiting: number }) {
  const [back, setBack] = useState(false);
  const wasOffline = useRef(false);
  useEffect(() => {
    if (!connected) {
      wasOffline.current = true;
      setBack(false);
      return;
    }
    if (!wasOffline.current) return;
    wasOffline.current = false;
    setBack(true);
    const id = window.setTimeout(() => setBack(false), 2000);
    return () => window.clearTimeout(id);
  }, [connected]);
  if (connected && !back) return null;
  return (
    <div className={`connection-bar${connected ? " back" : ""}`} role="status">
      {connected ? (
        <b>다시 연결되었습니다</b>
      ) : (
        <>
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" aria-hidden>
            <path d="M2 8.8a15 15 0 0 1 4.2-2.6M22 8.8a15 15 0 0 0-11.3-3.7M5 12.6a10 10 0 0 1 5.2-2.6M19 12.6a10 10 0 0 0-2.5-1.6M8.5 16.3a5 5 0 0 1 7 0M12 20h.01M3 3l18 18" />
          </svg>
          <b>오프라인</b>
          <span>
            {waiting > 0
              ? `입력 ${waiting.toLocaleString()}건은 이 기기에 보관 중이에요. 다시 연결되면 자동으로 보냅니다.`
              : "다시 연결하는 중…"}
          </span>
        </>
      )}
    </div>
  );
}

/** 엑셀처럼 그리드 아래의 시트 탭: 시트 이름, 내가 아직 채우지 않은 필수 칸 수, 확인하지 않은 강조. */
function SheetTabs({
  s,
  onSelect,
  keepFocus,
}: {
  s: SessionState;
  onSelect: (sheet: number) => void;
  /** 채팅에 넣을 범위를 고르는 중: 탭을 눌러도 초점을 채팅 입력 칸에 둔다(다른 시트에서 이어 고른다). */
  keepFocus?: boolean;
}) {
  return (
    <div className="sheet-tabs" role="tablist" aria-label="시트">
      {s.sheets.map((x, i) => {
        const p = s.sheetProgress[i];
        const left = p ? p.required - p.filled : 0;
        const alert = s.highlights.some((h) => h.sheet === i && !h.acked);
        return (
          <button
            key={x.model.sheet}
            role="tab"
            aria-selected={i === s.active}
            title={[x.model.sheet, left > 0 && `내 필수 미입력 ${left}칸`, alert && "확인하지 않은 강조 있음"]
              .filter(Boolean)
              .join(" · ")}
            onMouseDown={(e) => keepFocus && e.preventDefault()}
            onClick={() => onSelect(i)}
          >
            {alert && <span className="dot" aria-hidden />}
            <span className="name">{x.model.sheet}</span>
            {left > 0 && <span className="left">{left.toLocaleString()}</span>}
          </button>
        );
      })}
    </div>
  );
}

/** 토스트가 흐려지며 사라지는 시간(edit.css .toast.leaving과 같다). */
const LEAVE_MS = 150;

function Toast({
  notice,
  onDone,
  onAction,
}: {
  notice: Notice;
  onDone: (id: number) => void;
  onAction: (a: NonNullable<Notice["action"]>) => void;
}) {
  const { id, text, tone, action, kind } = notice;
  // 시간이 지나 사라질 때는 잠깐(LEAVE_MS) 흐려진 뒤 없앤다. 누르면 바로 없앤다.
  const [leaving, setLeaving] = useState(false);
  useEffect(() => {
    const t = window.setTimeout(() => setLeaving(true), tone === "error" ? 7000 : 5000);
    return () => window.clearTimeout(t);
  }, [tone]);
  useEffect(() => {
    if (!leaving) return;
    const t = window.setTimeout(() => onDone(id), LEAVE_MS);
    return () => window.clearTimeout(t);
  }, [leaving, id, onDone]);
  return (
    <div
      className={`toast ${tone}${kind ? ` ${kind}` : ""}${leaving ? " leaving" : ""}`}
      role={tone === "error" ? "alert" : "status"}
    >
      <span className="toast-text">{text}</span>
      {action && (
        <button
          className="small"
          onClick={() => {
            onAction(action);
            onDone(id);
          }}
        >
          {action.label}
        </button>
      )}
      <button className="ghost small" aria-label="닫기" onClick={() => onDone(id)}>
        ✕
      </button>
    </div>
  );
}
