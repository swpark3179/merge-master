import type { CellDecoration, EditorState, GridSelection, Move, SheetGridHandle } from "@mm/grid";
import { CellEditor, fitRowHeights, parseTsv, SheetGrid, toTsv } from "@mm/grid";
import type { CellRange, CellRef, SheetModel, StylePatch } from "@mm/shared";
import { a1, dday, displayValue, editText, FLAG, formatDeadline, normalize, personColor } from "@mm/shared";
import type { ClipboardEvent, KeyboardEvent } from "react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import type { BenchResult } from "./bench";
import { runScrollBench } from "./bench";
import type { RowFilter } from "./filters";
import { filteredOut, filterOptions, isFiltering, myRowsOf, NO_FILTER, withHiddenRows } from "./filters";
import type { BulkResult, EditContext } from "./gridOps";
import { canEdit, clearRange, copyRange, pasteRange } from "./gridOps";
import { RowForm } from "./RowForm";
import type { PanelTab } from "./SidePanel";
import { SidePanel } from "./SidePanel";
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
  const slug = location.pathname.split("/").filter(Boolean)[0] ?? "root";
  return `mm.${slug}.side`;
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

function clock(ms: number): string {
  const d = new Date(ms);
  return [d.getHours(), d.getMinutes(), d.getSeconds()].map((n) => String(n).padStart(2, "0")).join(":");
}

/** 1f 참여자 그리드(셀 단위 동시 편집)와 1g 행 단위 입력 폼. */
export function Workspace({ s }: { s: SessionState }) {
  const w = s.welcome!;
  const sheet = s.sheet!;
  const model = sheet.model;
  const { columns, labels } = w;
  const flags = s.flags;
  const [mode, setMode] = useState<"grid" | "form">("grid");
  const [selection, setSelection] = useState<GridSelection | null>(null);
  const [filter, setFilter] = useState<RowFilter>(NO_FILTER);
  const [editor, setEditor] = useState<ActiveEdit | null>(null);
  const [formRow, setFormRow] = useState<number | null>(null);
  const [waitFree, setWaitFree] = useState<Set<number>>(() => new Set());
  const [bench, setBench] = useState<BenchResult | null>(null);
  /** 오른쪽 사이드바(셀 내용·알림·채팅·셀 이력)와 그 탭. */
  const [sideOpen, setSideOpenState] = useState(loadSide);
  const [panel, setPanelTab] = useState<PanelTab>("cell");
  const setSideOpen = useCallback((open: boolean) => {
    setSideOpenState(open);
    saveSide(open);
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

  const myRows = useMemo(() => myRowsOf(model, flags, w.rowOwner, w.color), [model, flags, w.rowOwner, w.color]);
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
    () => (w.permissions.filterSort ? columns.map((c, i) => (c.filter ? i : -1)).filter((i) => i >= 0) : []),
    [columns, w.permissions.filterSort],
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
        if (!h.acked && row >= r.row && row < r.row + r.rows && col >= r.col && col < r.col + r.cols)
          s.ackHighlight(h.id);
      }
    },
    [leave, s],
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
  const lockState = useRef({ waitFree, model });
  lockState.current = { waitFree, model };
  useEffect(() => {
    const { waitFree, model } = lockState.current;
    if (!lockEvent) return;
    const e = editorRef.current;
    const same = e !== null && e.at.row === lockEvent.at.row && e.at.col === lockEvent.at.col;
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
      const i = lockEvent.at.row * model.cols + lockEvent.at.col;
      if (waitFree.has(i)) {
        notify(
          `${a1(model.origin.row + lockEvent.at.row, model.origin.col + lockEvent.at.col)} 칸을 이제 입력할 수 있습니다`,
        );
        setWaitFree((set) => {
          const next = new Set(set);
          next.delete(i);
          return next;
        });
      }
    }
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

  const policy = w.permissions.format;
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
      if (p.host || p.name === w.name || !p.cursor) continue;
      map.set(`${p.cursor.row}:${p.cursor.col}`, { name: p.name, color: personColor(p.color) });
    }
    return map;
  }, [s.presence, w.name]);

  const colorOf = (name: string) => personColor(Math.max(0, w.roster.indexOf(name)));

  // 호스트가 강조한 칸: 칸 → (메모, 왼쪽 위 칸인지)
  const highlighted = useMemo(() => {
    const map = new Map<string, { note: string; head: boolean }>();
    for (const h of s.highlights) {
      for (let r = h.range.row; r < h.range.row + h.range.rows; r++)
        for (let c = h.range.col; c < h.range.col + h.range.cols; c++)
          map.set(`${r}:${c}`, { note: h.note, head: r === h.range.row && c === h.range.col });
    }
    return map;
  }, [s.highlights]);

  /** 그 칸(모델 좌표)으로 간다. 필터에 걸려 숨었으면 필터를 지운다. */
  const revealRange = (range: CellRange, highlight?: number) => {
    if (highlight !== undefined) s.ackHighlight(highlight);
    if (mode !== "grid") setMode("grid");
    if (hidden.has(range.row)) setFilter(NO_FILTER);
    if (model.hiddenRows.includes(range.row)) {
      notify("그 칸은 이 화면에서 볼 수 없습니다(남의 행)", "warn");
      return;
    }
    requestAnimationFrame(() => grid.current?.reveal({ row: range.row, col: range.col }));
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
      if (hl.head) {
        cls.push("deco-peer");
        adornment = <span className="callout">{hl.note || "호스트가 강조한 칸"}</span>;
      }
    }
    if (lockedBy) {
      const color = colorOf(lockedBy);
      cls.push("deco-peer", "deco-lockedby");
      style = { boxShadow: `inset 0 0 0 2px ${color}` };
      adornment = (
        <span className="peer-tag lock-tag" style={{ background: color }}>
          🔒 {lockedBy}
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
    const p = w.rowOwner[r] ?? -1;
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
    if (rowsAdded.by !== w.name) return;
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
  const p = s.progress ?? w.progress;
  const pct = p.required > 0 ? Math.round((p.filled / p.required) * 100) : 100;
  const due = dday(w.session.deadline);
  const waiting = sheet.outbox.reduce((n, b) => n + b.cells.length, 0);
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
          <div className="eyebrow">{w.session.requester ? `${w.session.requester} 님의 취합 요청` : "취합 요청"}</div>
          <h1>{w.session.title}</h1>
        </div>
        {due && (
          <span className="pill" title={formatDeadline(w.session.deadline)}>
            마감 {due}
          </span>
        )}
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
        {w.permissions.addRows && (
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
                at={at}
                labels={labels}
                columns={columns}
                flags={flags}
                canEdit={(r, c) => canEdit(ctx, r, c)}
                locker={locker}
                waiting={at !== null && waitFree.has(idx(at.row, at.col))}
                onWait={() => at && setWaitFree((set) => new Set(set).add(idx(at.row, at.col)))}
                unreadChat={unreadChat}
                onReveal={revealRange}
                onChatSeen={onChatSeen}
              />
            )}
          </div>
        </>
      ) : (
        <RowForm s={s} myRows={myRows} row={formRow} onRow={setFormRow} onShowInGrid={showInGrid} />
      )}

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
          {w.name} 님 · 내 변경 {sheet.myChanges.toLocaleString()}건
          {sheet.lastSaved ? ` · 마지막 저장 ${clock(sheet.lastSaved)}` : ""}
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

      <div className={`toasts${sideOpen && mode === "grid" ? " with-panel" : ""}`} aria-live="polite">
        {s.notices.map((n) => (
          <Toast key={n.id} notice={n} onDone={s.dismiss} onAction={(a) => revealRange(a.range, a.highlight)} />
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
