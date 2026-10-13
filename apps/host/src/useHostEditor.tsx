import type { EditorState, GridSelection, Move, SheetGridHandle } from "@mm/grid";
import { CellEditor } from "@mm/grid";
import type { CellRef, CellValue, ColumnRule, SheetModel } from "@mm/shared";
import { editText, FLAG, normalize } from "@mm/shared";
import type { KeyboardEvent, RefObject } from "react";
import { useCallback, useEffect, useRef, useState } from "react";

import type { CellWrite } from "./api";

interface ActiveEdit {
  at: CellRef;
  state: EditorState;
}

export interface HostEditorOptions {
  /** 그리드에 그리는 시트(값·머리글 행 수). */
  model: SheetModel;
  /** 호스트 시점 셀 상태 비트(병합에 덮인 칸·숨김). 아직 없으면 빈 배열. */
  flags: Uint8Array;
  columns: ColumnRule[] | undefined;
  grid: RefObject<SheetGridHandle | null>;
  /** 그리드를 감싼 요소(포커스가 그리드 안에 있을 때만 편집기로 옮긴다). */
  host: RefObject<HTMLElement | null>;
  selection: GridSelection | null;
  setSelection: (sel: GridSelection) => void;
  /** 이 칸을 고치면 안 되는 까닭(예: 참여자가 입력 중). 고칠 수 있으면 `null`. */
  blockedBy?: (row: number, col: number) => string | null;
  /** 바꾼 값을 보낸다(값이 같은 칸은 빼고 부른다). */
  commit: (cells: CellWrite[]) => void;
  notify: (msg: string) => void;
}

const BLOCKED = FLAG.COVERED | FLAG.HIDDEN;

/**
 * 호스트가 칸 내용을 고칠 때 바꿀 값. 머리글은 글자 그대로, 데이터 칸은 열 형식에 맞으면 그 값(숫자·날짜·목록 항목),
 * 맞지 않으면 글자 그대로 둔다(호스트는 형식 검사로 막지 않는다).
 */
export function hostValue(
  text: string,
  row: number,
  col: number,
  model: SheetModel,
  columns?: ColumnRule[],
): CellValue {
  const t = text.replace(/[\r\n]+$/, "");
  if (t.trim() === "") return null;
  if (row < model.headerRows) return t;
  const format = columns?.[col]?.format;
  if (format) {
    const n = normalize(format, t);
    if (n.ok) return n.value;
  }
  return t;
}

function same(a: CellValue | undefined, b: CellValue | undefined): boolean {
  const empty = (v: CellValue | undefined) => v === undefined || v === null || v === "";
  if (empty(a) && empty(b)) return true;
  return JSON.stringify(a) === JSON.stringify(b);
}

/**
 * 호스트 그리드(1b 설정·4 모니터링)의 칸 편집. 참여자 화면과 같은 편집기([`CellEditor`])를 활성 칸에 대기시켜
 * 바로 입력(한글 첫 글자 포함)·F2·두 번 누르기로 고치고, Enter/Tab으로 확정해 옮긴다. Delete는 고른 범위를 지운다.
 * 수식 칸·병합에 덮인 칸·숨긴 칸은 고칠 수 없다. 잠금·되돌리기는 없다(히스토리 화면에서 되돌린다).
 */
export function useHostEditor(o: HostEditorOptions) {
  const { model, flags, columns, grid, host, selection, setSelection, blockedBy, commit, notify } = o;
  const [editor, setEditor] = useState<ActiveEdit | null>(null);
  const editorRef = useRef<ActiveEdit | null>(null);
  editorRef.current = editor;
  const input = useRef<HTMLTextAreaElement>(null);
  const idx = (r: number, c: number) => r * model.cols + c;

  /** 고칠 수 없으면 그 까닭, 고칠 수 있으면 `null`. */
  const reason = (r: number, c: number): string | null => {
    const cell = model.cells[idx(r, c)];
    if (!cell) return "표 밖입니다";
    if (cell.f) return "수식 칸은 고칠 수 없습니다(Excel이 계산합니다)";
    if ((flags[idx(r, c)] ?? 0) & BLOCKED) return "고칠 수 없는 칸입니다";
    return blockedBy?.(r, c) ?? null;
  };

  const at = selection?.anchor ?? null;
  const editable = at !== null && reason(at.row, at.col) === null;

  const startEdit = useCallback((target: CellRef, text: string, arrowsMove: boolean) => {
    const next: ActiveEdit = { at: target, state: { text, arrowsMove, listOpen: false } };
    editorRef.current = next;
    setEditor(next);
    requestAnimationFrame(() => {
      const el = input.current;
      if (!el || editorRef.current !== next) return;
      el.focus({ preventScroll: true });
      el.setSelectionRange(el.value.length, el.value.length);
    });
  }, []);

  const close = useCallback(() => {
    editorRef.current = null;
    setEditor(null);
  }, []);

  const finish = useCallback(
    (move: Move, override?: string) => {
      const e = editorRef.current;
      if (!e) return;
      close();
      const value = hostValue(override ?? e.state.text, e.at.row, e.at.col, model, columns);
      if (!same(model.cells[idx(e.at.row, e.at.col)]?.v, value)) commit([{ row: e.at.row, col: e.at.col, value }]);
      if (move) grid.current?.step(move[0], move[1]);
    },
    // idx는 model로 정해진다.
    [close, columns, commit, grid, model],
  );

  const onSelectionChange = useCallback(
    (sel: GridSelection) => {
      const e = editorRef.current;
      if (e && (sel.anchor.row !== e.at.row || sel.anchor.col !== e.at.col)) finish(null);
      setSelection(sel);
    },
    [finish, setSelection],
  );

  // 활성 칸을 고칠 수 있으면 투명 편집기에, 아니면 그리드에 포커스(그리드 안에서 움직일 때만).
  const overlayOn = at !== null && (editable || editor !== null);
  const atKey = at ? `${at.row}:${at.col}` : null;
  useEffect(() => {
    if (!atKey) return;
    // 마우스로 칸을 누르면 브라우저가 이벤트가 끝난 뒤 그리드에 포커스를 주므로 한 프레임 뒤에 옮긴다.
    const id = requestAnimationFrame(() => {
      const active = document.activeElement;
      if (active && active !== document.body && !host.current?.contains(active)) return;
      if (overlayOn) input.current?.focus({ preventScroll: true });
      else grid.current?.focus();
    });
    return () => cancelAnimationFrame(id);
  }, [atKey, overlayOn, grid, host]);

  const blocked = (r: number, c: number) => {
    const why = reason(r, c);
    if (why) notify(why);
  };

  /** 고른 범위에서 값이 있는, 고칠 수 있는 칸을 비운다. */
  const clearSelection = (sel: GridSelection) => {
    const r0 = Math.min(sel.anchor.row, sel.focus.row);
    const r1 = Math.max(sel.anchor.row, sel.focus.row);
    const c0 = Math.min(sel.anchor.col, sel.focus.col);
    const c1 = Math.max(sel.anchor.col, sel.focus.col);
    const cells: CellWrite[] = [];
    let skipped = 0;
    for (let r = r0; r <= r1; r++) {
      for (let c = c0; c <= c1; c++) {
        const v = model.cells[idx(r, c)]?.v;
        if (same(v, null)) continue;
        if (reason(r, c) !== null) skipped++;
        else cells.push({ row: r, col: c, value: null });
      }
    }
    if (cells.length > 0) commit(cells);
    if (skipped > 0) notify(`고칠 수 없는 ${skipped}칸(수식·입력 중인 칸 등)은 그대로 두었습니다`);
  };

  /** 그리드의 키(편집 중이 아닐 때). 방향키 이동은 그리드가 한다. */
  const onKey = (e: KeyboardEvent) => {
    if (editorRef.current || !at || !selection) return;
    const mod = e.ctrlKey || e.metaKey;
    if (e.key === "Delete") {
      e.preventDefault();
      clearSelection(selection);
      return;
    }
    if (e.key === "Backspace" || e.key === "F2") {
      e.preventDefault();
      if (!editable) return blocked(at.row, at.col);
      const cell = model.cells[idx(at.row, at.col)];
      const format = at.row < model.headerRows ? undefined : columns?.[at.col]?.format;
      if (e.key === "Backspace") startEdit(at, "", true);
      else startEdit(at, editText(cell?.v, format), false);
      return;
    }
    if (e.key.length === 1 && !mod && !e.altKey) {
      if (!editable) return blocked(at.row, at.col);
      // 편집기가 포커스를 받기 전에 그리드로 온 첫 글자(칸을 누르자마자 입력)
      e.preventDefault();
      startEdit(at, e.key, true);
    }
  };

  const onCellDoubleClick = (row: number, col: number) => {
    if (reason(row, col) !== null) return blocked(row, col);
    const format = row < model.headerRows ? undefined : columns?.[col]?.format;
    startEdit({ row, col }, editText(model.cells[idx(row, col)]?.v, format), false);
  };

  const setText = (text: string) => {
    const e = editorRef.current;
    if (!e) return;
    const next = { ...e, state: { ...e.state, text, error: undefined } };
    editorRef.current = next;
    setEditor(next);
  };

  const node =
    overlayOn && at ? (
      <CellEditor
        inputRef={input}
        state={editor?.state ?? null}
        onStart={(text) => startEdit(at, text, true)}
        onText={setText}
        onCommit={finish}
        onLeave={() => finish(null)}
        onCancel={close}
        onToggleList={() => {}}
      />
    ) : null;

  return {
    overlay: node ? { at: editor?.at ?? at!, node } : null,
    onKey,
    onCellDoubleClick,
    onSelectionChange,
    /** 편집 중이면 확정한다(오른쪽 클릭 메뉴를 열 때). */
    leave: () => finish(null),
    /** 편집 중인 칸과 글(수식 줄에 보인다). */
    editing: editor ? { at: editor.at, text: editor.state.text } : null,
  };
}
