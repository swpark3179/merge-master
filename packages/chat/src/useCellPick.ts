import type { GridPick, GridRange, GridSelection } from "@mm/grid";
import type { CellRange, CellRef } from "@mm/shared";
import { useCallback, useMemo, useRef, useState } from "react";

/** 고르기 대상(그리드를 가진 쪽이 준다). 좌표는 모두 지금 시트의 모델 좌표. */
export interface CellPickTarget {
  /** 처음 방향키를 누를 때 출발할 칸(보통 지금 선택의 활성 칸). */
  start(): CellRef;
  /** 그 칸에서 한 칸 옮긴 칸(그리드 핸들의 `moveFrom`: 병합·숨긴 행을 건너뛰고 스크롤한다). */
  move(from: CellRef, dRow: -1 | 0 | 1, dCol: -1 | 0 | 1): CellRef;
  /** 범위 → 채팅에 넣을 참조 글자(`#시트!F6:H8`)와 보일 이름(시트 접두·주소). */
  describe(range: CellRange): { token: string; sheet: string; addr: string };
  /** 친 주소(시트 주소) → 모델 좌표. */
  fromSheet(range: CellRange): CellRange;
  /** 고르기를 시작할 때(열린 칸 편집을 닫는 등). */
  onBegin?(): void;
}

/** 지금 고른 범위의 참조. */
export interface PickedRef {
  token: string;
  sheet: string;
  addr: string;
  count: number;
}

/** 채팅 입력 칸과 그리드가 함께 쓰는 '@ 셀 선택' 세션. */
export interface CellPick {
  active: boolean;
  /** 고른 범위(모델 좌표). 아직 고르지 않았으면 `null`. */
  range: CellRange | null;
  /** 어떻게 골랐나: 그리드(끌기·방향키)인지 주소를 쳤는지. */
  source: "grid" | "typed" | null;
  current: PickedRef | null;
  begin(): void;
  end(): void;
  /** 고른 범위를 비운다(시트를 바꿀 때). */
  reset(): void;
  /** 방향키: Shift면 끝 칸을 늘리고, 아니면 한 칸을 옮긴다. 처음이면 출발 칸에서. */
  move(dRow: -1 | 0 | 1, dCol: -1 | 0 | 1, extend: boolean): void;
  /** 친 주소(시트 주소, 없으면 `null`)를 미리 보인다. */
  typed(range: CellRange | null): void;
  /** 친 주소(시트 주소)의 참조 글자. */
  tokenOf(range: CellRange): string;
  /** 그리드에 줄 것(고르는 중일 때만). */
  grid: GridPick | undefined;
  /** 그리드에 겹쳐 그릴 고르는 범위. */
  ranges: GridRange[];
  /** 그리드에서 마우스를 놓았을 때 부를 것(입력 칸이 참조를 넣는다). */
  bindCommit(fn: (() => void) | null): void;
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

/** 고르기 세션 상태. `target`은 그릴 때마다 새로 줘도 된다(늘 마지막 것을 쓴다). */
export function useCellPick(target: CellPickTarget): CellPick {
  const t = useRef(target);
  t.current = target;
  const commit = useRef<(() => void) | null>(null);
  const [active, setActive] = useState(false);
  const [sel, setSel] = useState<GridSelection | null>(null);
  const [source, setSource] = useState<"grid" | "typed" | null>(null);
  const selRef = useRef(sel);
  selRef.current = sel;

  const begin = useCallback(() => {
    t.current.onBegin?.();
    setActive(true);
    setSel(null);
    setSource(null);
  }, []);
  const end = useCallback(() => {
    setActive(false);
    setSel(null);
    setSource(null);
  }, []);
  const reset = useCallback(() => {
    setSel(null);
    setSource(null);
  }, []);
  const move = useCallback((dRow: -1 | 0 | 1, dCol: -1 | 0 | 1, extend: boolean) => {
    const cur = selRef.current;
    const from = cur ? cur.focus : t.current.start();
    const to = t.current.move(from, dRow, dCol);
    const anchor = extend ? (cur?.anchor ?? from) : to;
    setSel({ anchor, focus: to });
    setSource("grid");
  }, []);
  const typed = useCallback((range: CellRange | null) => {
    if (!range) return;
    const r = t.current.fromSheet(range);
    setSel({ anchor: { row: r.row, col: r.col }, focus: { row: r.row + r.rows - 1, col: r.col + r.cols - 1 } });
    setSource("typed");
  }, []);
  const grid = useMemo<GridPick>(
    () => ({
      onChange: (s) => {
        setSel(s);
        setSource("grid");
      },
      onEnd: () => commit.current?.(),
    }),
    [],
  );

  const range = active && sel ? rectOf(sel) : null;
  const current = range ? { ...target.describe(range), count: range.rows * range.cols } : null;
  return {
    active,
    range,
    source,
    current,
    begin,
    end,
    reset,
    move,
    typed,
    tokenOf: (r) => t.current.describe(t.current.fromSheet(r)).token,
    grid: active ? grid : undefined,
    ranges: range ? [{ key: "pick", range, kind: "pick" }] : [],
    bindCommit: (fn) => {
      commit.current = fn;
    },
  };
}
