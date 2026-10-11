import type { CellDecoration, EditorState, GridSelection, Move, SheetGridHandle } from "@mm/grid";
import { CellEditor, fitRowHeights, parseTsv, SheetGrid, toTsv } from "@mm/grid";
import type { CellRange, CellRef, SheetModel, StylePatch } from "@mm/shared";
import {
  a1,
  displayValue,
  editText,
  FLAG,
  formatDeadline,
  normalize,
  parseDeadline,
  personColor,
  remaining,
  sheetPrefix,
} from "@mm/shared";
import type { ClipboardEvent, CSSProperties, KeyboardEvent } from "react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import type { BenchResult } from "./bench";
import { runScrollBench } from "./bench";
import type { RowFilter } from "./filters";
import { filteredOut, filterOptions, isFiltering, myRowsOf, NO_FILTER, withHiddenRows } from "./filters";
import type { BulkResult, EditContext } from "./gridOps";
import { canEdit, clearRange, copyRange, pasteRange } from "./gridOps";
import { RowForm } from "./RowForm";
import type { PanelTab } from "./SidePanel";
import { clampSideWidth, SIDE_WIDTH, SidePanel } from "./SidePanel";
import { storageSlug } from "./slug";
import type { Notice, SessionState } from "./useSession";

const FILLS: { label: string; rgb: number | null }[] = [
  { label: "노랑", rgb: 0xfff3b0 },
  { label: "초록", rgb: 0xd9f2e3 },
  { label: "파랑", rgb: 0xdbe8f8 },
  { label: "분홍", rgb: 0xfadbe6 },
  { label: "채우기 없음", rgb: null },
];

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
  const [panel, setPanelTab] = useState<PanelTab>("cell");
  const setSideOpen = useCallback((open: boolean) => {
    setSideOpenState(open);
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
  /** 마감까지 남은 시간(분 단위로 보이므로 30초마다 다시 센다). */
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!w.session.deadline) return;
    const id = window.setInterval(() => setNow(Date.now()), 30_000);
    return () => window.clearInterval(id);
  }, [w.session.deadline]);
  const onChatSeen = useCallback((id: number) => setChatSeen((x) => Math.max(x, id)), []);
  const editorRef = useRef<ActiveEdit | null>(null);
  editorRef.current = editor;
  const input = useRef<HTMLTextAreaElement>(null);
  const grid = useRef<SheetGridHandle>(null);
  const gridHost = useRef<HTMLDivElement>(null);
  const lastBlocked = useRef(0);
  const { notify } = s;
  const idx = (r: number, c: number) => r * model.cols + c;
  const addr = (at: CellRef) => a1(model.origin.row + at.row, model.origin.col + at.col);

  const myRows = useMemo(() => myRowsOf(model, flags, sv.rowOwner, w.color), [model, flags, sv.rowOwner, w.color]);
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

  const onSelection = useCallback(
    (sel: GridSelection) => {
      const e = editorRef.current;
      if (e && (sel.anchor.row !== e.at.row || sel.anchor.col !== e.at.col)) leave();
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
    [leave, s, active],
  );

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
    else notify("이 칸은 입력할 수 없습니다. 흰 바탕의 내 담당 칸에 입력해 주세요", "warn");
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

  // ------------------------------------------------------------ 그리기

  const peers = useMemo(() => {
    const map = new Map<string, { name: string; color: string }>();
    for (const p of s.presence) {
      if (p.host || p.name === w.name || !p.cursor || p.cursor.sheet !== active) continue;
      map.set(`${p.cursor.row}:${p.cursor.col}`, { name: p.name, color: personColor(p.color) });
    }
    return map;
  }, [s.presence, w.name, active]);

  const colorOf = (name: string) => personColor(Math.max(0, w.roster.indexOf(name)));

  // 강조된 칸: 칸 → (메모, 왼쪽 위 칸인지)
  const highlighted = useMemo(() => {
    const map = new Map<string, { note: string; head: boolean }>();
    for (const h of s.highlights) {
      if (h.sheet !== active) continue;
      for (let r = h.range.row; r < h.range.row + h.range.rows; r++)
        for (let c = h.range.col; c < h.range.col + h.range.cols; c++)
          map.set(`${r}:${c}`, { note: h.note, head: r === h.range.row && c === h.range.col });
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
    memory.current.set(active, { selection, filter });
    const back = memory.current.get(to);
    setSelection(back?.selection ?? null);
    setFilter(back?.filter ?? NO_FILTER);
    setFormRow(null);
    s.setActive(to);
    s.cursor(back?.selection?.anchor ?? null);
  };

  /** 그 칸(그 시트의 모델 좌표)으로 간다. 다른 시트면 시트를 바꾸고, 필터에 걸려 숨었으면 필터를 지운다. */
  const revealRange = (range: CellRange, highlight?: number, target: number = active) => {
    if (highlight !== undefined) s.ackHighlight(highlight);
    if (mode !== "grid") setMode("grid");
    if (target !== active) {
      if (!s.sheets[target]) return;
      switchSheet(target);
      setPendingReveal({ sheet: target, range });
      return;
    }
    if (hidden.has(range.row)) setFilter(NO_FILTER);
    if (model.hiddenRows.includes(range.row)) {
      notify("그 칸은 이 화면에서 볼 수 없습니다(남의 행)", "warn");
      return;
    }
    requestAnimationFrame(() => grid.current?.reveal({ row: range.row, col: range.col }));
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
    const peer = peers.get(`${r}:${c}`);
    const lockedBy = sheet.locks.get(i);
    if (f & FLAG.HEADER) {
      return peer && { className: "deco-peer", style: { boxShadow: `inset 0 0 0 2px ${peer.color}` } };
    }
    const cls: string[] = [];
    if (!myRows[r]) cls.push("deco-others");
    else if (!(f & FLAG.INPUT)) cls.push("deco-locked");
    else if (!(f & FLAG.MINE)) cls.push("deco-others");
    const v = model.cells[i]?.v;
    if (f & FLAG.MINE && f & FLAG.REQUIRED && (v === undefined || v === null || v === "")) cls.push("deco-missing");
    if (sheet.pending.has(i)) cls.push("deco-pending");
    let style: CellDecoration["style"];
    let adornment: CellDecoration["adornment"];
    const hl = highlighted.get(`${r}:${c}`);
    if (hl) {
      cls.push("deco-highlight");
      // 메모가 있으면 왼쪽 위 칸에 말풍선으로 보인다(없으면 강조색만).
      if (hl.head && hl.note) {
        cls.push("deco-peer");
        adornment = <span className="callout">{hl.note}</span>;
      }
    }
    if (lockedBy) {
      const color = colorOf(lockedBy);
      cls.push("deco-peer", "deco-lockedby");
      style = { boxShadow: `inset 0 0 0 2px ${color}` };
      adornment = (
        <span className="peer-tag lock-tag" style={{ background: color }}>
          {lockedBy}
        </span>
      );
    } else if (peer) {
      cls.push("deco-peer");
      style = { boxShadow: `inset 0 0 0 2px ${peer.color}` };
      adornment = (
        <span className="peer-tag" style={{ background: peer.color }}>
          {peer.name}
        </span>
      );
    }
    return { className: cls.join(" "), style, adornment };
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
      // '+ 행 추가' 버튼에 있던 포커스를 놓아 새 칸으로 가면 곧바로 입력할 수 있게 한다.
      if (document.activeElement instanceof HTMLElement && !gridHost.current?.contains(document.activeElement))
        document.activeElement.blur();
      showInGrid(row);
    }
    // 새 행을 받은 시트(model)가 바뀐 뒤 한 번만 움직인다.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [rowsAdded, model]);

  const unreadAlerts = s.highlights.filter((h) => !h.acked).length + s.announcements.filter((n) => !n.read).length;
  const unreadChat = s.chat.filter((m) => m.id > chatSeen && !(m.from === w.name && !m.host)).length;
  // 열려 있는 탭의 것은 이미 보고 있다.
  const unread =
    (sideOpen && mode === "grid" && panel === "alerts" ? 0 : unreadAlerts) +
    (sideOpen && mode === "grid" && panel === "chat" ? 0 : unreadChat);
  const others = s.presence.filter((p) => p.name !== w.name);
  const p = s.progress ?? { name: w.name, rows: 0, required: 0, filled: 0 };
  const pct = p.required > 0 ? Math.round((p.filled / p.required) * 100) : 100;
  const left = remaining(w.session.deadline, new Date(now));
  const overdue = left === "마감 지남";
  const dueAt = parseDeadline(w.session.deadline)?.getTime();
  const urgent = !overdue && dueAt !== undefined && dueAt - now < 3_600_000;
  const waiting = s.sheets.reduce((n, x) => n + x.outbox.reduce((k, b) => k + b.cells.length, 0), 0);
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
        <div>
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
          <span className="mine-progress">
            내 필수 입력{" "}
            <b>
              {p.filled.toLocaleString()} / {p.required.toLocaleString()}
            </b>
            <span className="bar">
              <span style={{ width: `${pct}%` }} />
            </span>
          </span>
        )}
        <span className="avatars" aria-label="접속 중">
          {others.map((x) => (
            <span
              key={x.name}
              className={`avatar${x.status === "away" ? " away" : ""}`}
              title={`${x.name}${x.host ? " (호스트)" : ""}${x.status === "away" ? " · 자리 비움" : ""}`}
              style={{ borderColor: x.host ? "#222" : personColor(x.color) }}
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
        {left && (
          <div
            className={`deadline-left${overdue || urgent ? " urgent" : ""}`}
            role="timer"
            aria-label={overdue ? "마감 지남" : `마감까지 ${left} 남음`}
          >
            <div className="eyebrow">마감 {formatDeadline(w.session.deadline)}</div>
            <b>{overdue ? left : `${left} 남음`}</b>
          </div>
        )}
      </header>

      {s.readOnly && (
        <div className="readonly-bar" role="status">
          {s.readOnlyReason ?? "마감되었습니다"} — 읽기 전용입니다. 입력한 값은 호스트가 취합합니다.
        </div>
      )}
      {s.announcements
        .filter((n) => n.important && !n.read)
        .map((n) => (
          <div key={n.id} className="important-bar" role="alert">
            <b>중요 공지</b>
            <span className="muted small">{clock(n.atMs).slice(0, 5)}</span>
            <span className="grow">{n.text}</span>
            <button className="small" onClick={() => s.ackNotice(n.id)}>
              확인
            </button>
          </div>
        ))}

      <div className="toolbar">
        <button className="ghost" onClick={undo} disabled={!sheet.canUndo()} title="내 변경 되돌리기 (Ctrl+Z)">
          ↶ 실행 취소
        </button>
        <span className="sep" />
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
                title={x.label}
                aria-label={x.label}
                style={{ background: x.rgb === null ? "#fff" : `#${x.rgb.toString(16).padStart(6, "0")}` }}
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
        <span className="muted small">
          {policy === "none" ? "서식은 호스트가 관리합니다" : policy === "fillOnly" ? "배경색만 바꿀 수 있습니다" : ""}
        </span>
        <span className="spacer" />
        {sv.permissions.addRows && (
          <button
            className="ghost small"
            disabled={s.readOnly}
            title="표 맨 아래에 빈 행을 추가합니다"
            onClick={() => {
              if (editorRef.current) leave();
              s.addRows(1);
            }}
          >
            + 행 추가
          </button>
        )}
        <button
          className={`ghost small side-toggle${sideOpen && mode === "grid" ? " on" : ""}`}
          aria-pressed={sideOpen && mode === "grid"}
          aria-label={`사이드바${unread > 0 ? ` (안 읽은 알림·채팅 ${unread})` : ""}`}
          title="오른쪽 사이드바: 셀 내용 · 알림 · 채팅 · 셀 이력"
          onClick={() => {
            if (mode !== "grid") {
              setMode("grid");
              setSideOpen(true);
            } else setSideOpen(!sideOpen);
          }}
        >
          ☰ 사이드바
          {unread > 0 && <span className="badge">{unread}</span>}
        </button>
        <div className="mode-tabs" role="tablist" aria-label="입력 방식">
          <button role="tab" aria-selected={mode === "grid"} onClick={() => setMode("grid")}>
            그리드
          </button>
          <button role="tab" aria-selected={mode === "form"} onClick={openForm}>
            행 단위
          </button>
        </div>
      </div>

      <ConnectionBar connected={s.connected} waiting={waiting} />

      {mode === "grid" ? (
        <>
          <div className="filter-bar">
            {filterCols.length > 0 && <span className="tag tag-filter">조회조건</span>}
            {filterCols.map((c) => (
              <label key={c} className="filter">
                {labels[c] || `열 ${c + 1}`}:
                <select
                  aria-label={`${labels[c]} 조회조건`}
                  value={filter.values[c] ?? ""}
                  onChange={(e) => {
                    const values = { ...filter.values };
                    if (e.target.value) values[c] = e.target.value;
                    else delete values[c];
                    setFilter({ ...filter, values });
                  }}
                >
                  <option value="">전체</option>
                  {options.get(c)?.map((o) => (
                    <option key={o} value={o}>
                      {o}
                    </option>
                  ))}
                </select>
              </label>
            ))}
            <label className="check">
              <input
                type="checkbox"
                checked={filter.mineOnly}
                onChange={(e) => setFilter({ ...filter, mineOnly: e.target.checked })}
              />
              내 담당만
            </label>
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
              {(model.rows - model.headerRows).toLocaleString()}행 · 내 담당 {myRowCount.toLocaleString()}행
              {isFiltering(filter) ? ` · 표시 ${shownRows.toLocaleString()}행` : ""}
            </span>
          </div>

          <div className="work-area">
            <div
              className="grid-host"
              ref={gridHost}
              // 오른쪽 클릭 → 셀 이력
              onContextMenu={(e) => {
                const el = (e.target as HTMLElement).closest<HTMLElement>("[data-r]");
                if (!el) return;
                e.preventDefault();
                const target = { row: Number(el.dataset.r), col: Number(el.dataset.c) };
                grid.current?.reveal(target);
                showPanel("history");
              }}
              // 칸을 누르면 브라우저가 그리드에 준 포커스를 바로 편집기로 옮긴다(곧바로 입력해도 첫 글자를 받는다).
              onClick={() => {
                if (overlayOn && !editorRef.current) input.current?.focus({ preventScroll: true });
              }}
              onCopy={(e) => onCopy(e, false)}
              onCut={(e) => onCopy(e, true)}
              onPaste={onPaste}
            >
              <SheetGrid
                key={active}
                handle={grid}
                model={view}
                decorate={decorate}
                rowMarker={rowMarker}
                cellText={cellText}
                selection={selection}
                onSelectionChange={onSelection}
                overlay={editorNode ? { at: editor?.at ?? at!, node: editorNode } : null}
                onKey={onKey}
                onCellDoubleClick={onDouble}
              />
            </div>
            {sideOpen && (
              <SidePanel
                s={s}
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
                unreadChat={unreadChat}
                onReveal={revealRange}
                onRef={revealRef}
                onChatSeen={onChatSeen}
              />
            )}
          </div>
        </>
      ) : (
        <RowForm s={s} myRows={myRows} row={formRow} onRow={setFormRow} onShowInGrid={showInGrid} />
      )}

      {s.sheets.length > 1 && <SheetTabs s={s} onSelect={switchSheet} />}

      <footer className="status">
        {!s.connected ? (
          <span className="warn">
            ○ 오프라인
            {waiting > 0 ? ` — 입력 ${waiting}건이 대기 중입니다(다시 연결되면 보냅니다)` : " — 다시 연결하는 중…"}
          </span>
        ) : waiting > 0 ? (
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
            onAction={(a) => revealRange(a.range, a.highlight, a.sheet ?? active)}
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
function SheetTabs({ s, onSelect }: { s: SessionState; onSelect: (sheet: number) => void }) {
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

function Toast({
  notice,
  onDone,
  onAction,
}: {
  notice: Notice;
  onDone: (id: number) => void;
  onAction: (a: NonNullable<Notice["action"]>) => void;
}) {
  const { id, text, tone, action } = notice;
  useEffect(() => {
    const t = window.setTimeout(() => onDone(id), tone === "error" ? 7000 : 5000);
    return () => window.clearTimeout(t);
  }, [id, onDone, tone]);
  return (
    <div className={`toast ${tone}`} role={tone === "error" ? "alert" : "status"}>
      <span>{text}</span>
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
