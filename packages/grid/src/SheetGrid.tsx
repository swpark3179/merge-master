import type { BorderEdge, Cell, CellRange, CellRef, CellStyle, SheetModel } from "@mm/shared";
import { colLetters, formatValue, PT_TO_PX, rgbHex } from "@mm/shared";
import type { CSSProperties, KeyboardEvent, MouseEvent, ReactNode, Ref } from "react";
import { useCallback, useEffect, useImperativeHandle, useLayoutEffect, useMemo, useRef, useState } from "react";

import "./grid.css";
import {
  boxRect,
  buildLayout,
  cellBox,
  GUTTER_W,
  HEADER_H,
  isHidden,
  layerOf,
  nextVisible,
  splitByFrozen,
  visibleSpan,
} from "./layout";
import type { GridLayout, Layer } from "./layout";
import type { GridSelection } from "./selection";
import { cellSelection, columnSelection, rowSelection, selectionRange } from "./selection";

/** 화면에 겹쳐 그릴 표시(입력 금지 빗금, 필수 미입력 표시, 편집 중 이름표 …). */
export interface CellDecoration {
  className?: string;
  style?: CSSProperties;
  /** 셀 안 오른쪽에 붙는 요소(배지·잠금 표시 등). */
  adornment?: ReactNode;
}

/**
 * 다른 사람의 커서·편집 잠금 표시. 칸 위의 겹에 사람마다 하나 그린다(같은 `key`면 칸을 옮길 때 미끄러진다).
 * 칸 장식(`decorate`)과 달리 고정 머리 밑에 깔리지 않게 이름표 위치를 스스로 바꾼다.
 */
export interface GridMark {
  /** 사람마다 하나(보통 이름). */
  key: string;
  row: number;
  col: number;
  color: string;
  /** 이름표 글자(없으면 테두리만). */
  label?: string;
  /** 편집 잠금(이름표 앞 자물쇠). */
  lock?: boolean;
}

/**
 * 칸 위에 겹쳐 그릴 범위 표시: 채팅에 넣을 범위를 고르는 중(`pick`, 흐르는 점선), 채팅 참조로 간 범위(`ping`, 테두리가
 * 두 번 깜빡이고 사라짐 — 다시 깜빡이려면 `key`를 바꾼다).
 */
export interface GridRange {
  key: string;
  range: CellRange;
  kind: "pick" | "ping";
}

/**
 * 채팅에 넣을 범위를 고르는 중: 칸·머리글을 누르고 끌면 선택을 바꾸는 대신 `onChange`로 알리고, 놓으면 `onEnd`.
 * 누를 때 포커스를 가져가지 않는다(채팅 입력 칸에 그대로 둔다). 더블클릭·오른쪽 클릭 메뉴는 받지 않는다.
 */
export interface GridPick {
  onChange(sel: GridSelection): void;
  onEnd(): void;
}

/** 오른쪽 클릭한 곳: 칸(병합이면 그 칸 좌표), 열 머리글, 행 머리글. */
export type GridHit =
  { kind: "cell"; row: number; col: number } | { kind: "col"; col: number } | { kind: "row"; row: number };

/** 바깥에서 그리드를 움직일 때(편집을 확정하고 다음 칸으로 등). */
export interface SheetGridHandle {
  focus(): void;
  /** 활성 칸에서 한 칸 움직인다(병합·숨긴 행 고려). 움직인 칸을 돌려준다. */
  step(dRow: -1 | 0 | 1, dCol: -1 | 0 | 1): CellRef | null;
  /** 칸이 보이도록 스크롤하고 그 칸을 선택한다. */
  reveal(at: CellRef): void;
  /** 범위를 선택하고 보이도록 스크롤한다(범위가 화면보다 크면 왼쪽 위부터). */
  select(sel: GridSelection): void;
  /** `from`에서 한 칸 옮긴 칸(병합·숨긴 행 고려)을 보이게 스크롤하고 돌려준다. 선택은 바꾸지 않는다. */
  moveFrom(from: CellRef, dRow: -1 | 0 | 1, dCol: -1 | 0 | 1): CellRef;
}

export interface SheetGridProps {
  model: SheetModel;
  handle?: Ref<SheetGridHandle>;
  /** 셀(병합이면 왼쪽 위 좌표로 한 번) 장식. */
  decorate?: (row: number, col: number) => CellDecoration | undefined;
  selection?: GridSelection | null;
  onSelectionChange?: (sel: GridSelection) => void;
  /** 행 번호 옆 색 점(행 담당자 표시). */
  rowMarker?: (row: number) => string | undefined;
  /** 셀 표시 글자를 바꾼다(예: 날짜 열은 일련번호 대신 날짜). `undefined`면 표시 형식대로. */
  cellText?: (row: number, col: number, cell: Cell | undefined, numberFormat: string) => string | undefined;
  /** 활성 칸 위에 겹쳐 그릴 요소(셀 편집기). */
  overlay?: { at: CellRef; node: ReactNode } | null;
  /** 다른 사람의 커서·편집 잠금. */
  marks?: GridMark[];
  /** 칸 위에 겹쳐 그릴 범위 표시(고르는 중인 범위, 채팅 참조로 간 범위). */
  ranges?: GridRange[];
  /** 주면 채팅에 넣을 범위를 고르는 중이다. */
  pick?: GridPick | null;
  /** 방향키 이동보다 먼저 받는다. `preventDefault()`하면 그리드는 이동하지 않는다. */
  onKey?: (e: KeyboardEvent) => void;
  onCellDoubleClick?: (row: number, col: number) => void;
  /**
   * 주면 열 머리글 오른쪽 끝·행 머리글 아래 끝을 끌어 크기를 바꿀 수 있다. 놓을 때 한 번, 새 크기(pt)로 부른다.
   * 그리드는 크기를 직접 바꾸지 않는다(쓰는 쪽이 `model`을 바꿔 다시 그린다).
   */
  onResize?: (axis: "col" | "row", index: number, sizePt: number) => void;
  /**
   * 칸·머리글을 오른쪽 클릭했다. 쓰는 쪽이 메뉴를 띄우려면 `e.preventDefault()`로 브라우저 메뉴를 막는다. 선택은 바꾸지
   * 않는다(필요하면 쓰는 쪽이 고른다).
   */
  onContextMenu?: (hit: GridHit, e: MouseEvent) => void;
  /** 보이는 마지막 행 번호칸 바로 아래에 그릴 것(예: 행 추가 단추). 행 번호 띠와 함께 가로로는 고정된다. */
  rowFooter?: ReactNode;
  /** 위·아래로 더 그려 둘 행 수. */
  overscan?: number;
  className?: string;
  style?: CSSProperties;
}

const GRIDLINE = "1px solid #e2e0da";

function borderCss(edge: BorderEdge | undefined): string {
  if (!edge) return GRIDLINE;
  const width = edge.weight === 4 ? 3 : edge.weight === -4138 ? 2 : 1;
  const kind =
    edge.style === -4115 ? "dashed" : edge.style === -4118 ? "dotted" : edge.style === -4119 ? "double" : "solid";
  return `${width}px ${kind} ${rgbHex(edge.color)}`;
}

function baseCss(s: CellStyle | undefined): CSSProperties {
  if (!s) return {};
  return {
    fontFamily: s.fontName ? `"${s.fontName}", system-ui, sans-serif` : undefined,
    fontSize: s.fontSize ? `${s.fontSize * PT_TO_PX}px` : undefined,
    fontWeight: s.bold ? 700 : undefined,
    fontStyle: s.italic ? "italic" : undefined,
    textDecoration: [s.underline && "underline", s.strike && "line-through"].filter(Boolean).join(" ") || undefined,
    color: s.color ? rgbHex(s.color) : undefined,
    // background 단축 속성은 장식 클래스의 background-image(빗금)까지 지우므로 색만 지정한다.
    backgroundColor: s.fill !== undefined ? rgbHex(s.fill) : undefined,
    alignItems: s.vAlign === "top" ? "flex-start" : s.vAlign === "center" ? "center" : "flex-end",
    whiteSpace: s.wrap ? "pre-wrap" : undefined,
  };
}

function justify(s: CellStyle | undefined, numeric: boolean): CSSProperties["justifyContent"] {
  switch (s?.hAlign) {
    case "left":
      return "flex-start";
    case "center":
      return "center";
    case "right":
      return "flex-end";
    default:
      return numeric ? "flex-end" : "flex-start"; // 일반: 숫자 오른쪽, 글자 왼쪽
  }
}

interface Viewport {
  top: number;
  left: number;
  width: number;
  height: number;
}

type Drag = {
  kind: "cells" | "cols" | "rows";
  anchor: number;
  anchorCell: { row: number; col: number };
  /** 채팅에 넣을 범위를 고르는 중(선택 대신 `pick`에 알린다). */
  pick: boolean;
};

/** 머리글 경계를 끄는 중: 처음 크기·마우스 위치와 지금 크기(px). */
interface Resize {
  axis: "col" | "row";
  index: number;
  start: number;
  from: number;
  size: number;
}

/** 끌어서 줄일 수 있는 가장 작은 크기(px). 0이면 숨김과 구별할 수 없다. */
const MIN_RESIZE_PX = 4;

/** 이름표 높이(grid.css .peer-tag의 line-height). 고정 머리 바로 아래 칸이면 이름표를 칸 아래로 보낸다. */
const TAG_H = 16;

/** 행 번호 아래 자리(`rowFooter`) 높이. */
const FOOTER_H = 26;

export function SheetGrid({
  model,
  handle,
  decorate,
  selection,
  onSelectionChange,
  rowMarker,
  cellText,
  overlay,
  marks,
  ranges,
  pick,
  onKey,
  onCellDoubleClick,
  onResize,
  onContextMenu,
  rowFooter,
  overscan = 6,
  className,
  style,
}: SheetGridProps) {
  const layout = useMemo(() => buildLayout(model), [model]);
  const styleCss = useMemo(() => model.styles.map(baseCss), [model]);
  const cancelled = useMemo(() => new Set(model.cancelledRows ?? []), [model]);
  const scroller = useRef<HTMLDivElement>(null);
  const [vp, setVp] = useState<Viewport>({ top: 0, left: 0, width: 800, height: 600 });
  const frame = useRef(0);
  const drag = useRef<Drag | null>(null);
  const [resize, setResize] = useState<Resize | null>(null);

  const measure = useCallback(() => {
    const el = scroller.current;
    if (!el) return;
    setVp({ top: el.scrollTop, left: el.scrollLeft, width: el.clientWidth, height: el.clientHeight });
  }, []);

  useLayoutEffect(() => {
    measure();
    const el = scroller.current;
    if (!el) return;
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, [measure]);

  const pickRef = useRef(pick);
  pickRef.current = pick;
  useEffect(() => {
    const up = () => {
      const d = drag.current;
      drag.current = null;
      if (d?.pick) pickRef.current?.onEnd();
    };
    window.addEventListener("mouseup", up);
    return () => {
      window.removeEventListener("mouseup", up);
      cancelAnimationFrame(frame.current);
    };
  }, []);

  const onScroll = useCallback(() => {
    cancelAnimationFrame(frame.current);
    frame.current = requestAnimationFrame(measure);
  }, [measure]);

  const sel = selection ? selectionRange(layout, selection) : null;
  const multi = sel !== null && (sel.rows > 1 || sel.cols > 1);

  const rowsSpan = visibleSpan(layout.rowY, layout.frozenRows, layout.frozenH, vp.top, vp.height - HEADER_H, overscan);
  const colsSpan = visibleSpan(layout.colX, layout.frozenCols, layout.frozenW, vp.left, vp.width - GUTTER_W, 2);

  const totalW = GUTTER_W + layout.colX[layout.cols]!;
  const totalH = HEADER_H + layout.rowY[layout.rows]!;
  const contentH = totalH + (rowFooter ? FOOTER_H : 0);
  const cornerW = GUTTER_W + layout.frozenW;
  const cornerH = HEADER_H + layout.frozenH;

  const renderBlock = (r0: number, r1: number, c0: number, c1: number): ReactNode[] => {
    const out: ReactNode[] = [];
    const seen = new Set<number>();
    for (let r = r0; r <= r1; r++) {
      if (isHidden(layout.rowY, r)) continue;
      for (let c = c0; c <= c1; c++) {
        if (isHidden(layout.colX, c)) continue;
        const m = layout.mergeAt[r * layout.cols + c]!;
        if (m >= 0) {
          if (seen.has(m)) continue;
          seen.add(m);
        }
        out.push(renderCell(cellBox(layout, r, c)));
      }
    }
    return out;
  };

  const renderCell = (box: CellRange): ReactNode => {
    const { row: r, col: c } = box;
    const cell = model.cells[r * model.cols + c];
    const sid = cell?.s ?? 0;
    const s = model.styles[sid];
    const lastR = box.row + box.rows - 1;
    const lastC = box.col + box.cols - 1;
    // 아래·오른쪽 선은 이웃 셀의 위·왼쪽 선과 같은 선이다. 어느 쪽에 저장돼 있든 그린다.
    const below = lastR + 1 < model.rows ? model.styles[model.cells[(lastR + 1) * model.cols + c]?.s ?? 0] : undefined;
    const right = lastC + 1 < model.cols ? model.styles[model.cells[r * model.cols + lastC + 1]?.s ?? 0] : undefined;
    const deco = decorate?.(r, c);
    const css: CSSProperties = {
      ...boxRect(layout, box),
      ...styleCss[sid],
      justifyContent: justify(s, typeof cell?.v === "number"),
      borderBottom: borderCss(s?.borders.bottom ?? below?.borders.top),
      borderRight: borderCss(s?.borders.right ?? right?.borders.left),
      ...deco?.style,
    };
    const cls = `mmg-cell${cancelled.has(r) ? " mmg-cancelled" : ""}${deco?.className ? ` ${deco.className}` : ""}`;
    return (
      <div key={`${r}:${c}`} className={cls} data-r={r} data-c={c} style={css}>
        <span className="mmg-text">
          {cellText?.(r, c, cell, s?.numberFormat ?? "") ?? formatValue(cell?.v, s?.numberFormat ?? "")}
        </span>
        {deco?.adornment}
      </div>
    );
  };

  const colHeads = (c0: number, c1: number) => {
    const out: ReactNode[] = [];
    for (let c = c0; c <= c1; c++) {
      if (isHidden(layout.colX, c)) continue;
      const x = layout.colX[c]!;
      // Excel처럼 숨긴 열 바로 뒤 머리글에 굵은 경계를 그려 숨김이 있음을 알린다.
      const after = c > 0 && isHidden(layout.colX, c - 1) ? " mmg-after-hidden" : "";
      const on = sel && c >= sel.col && c < sel.col + sel.cols ? " mmg-head-sel" : "";
      out.push(
        <div
          key={`h${c}`}
          className={`mmg-colhead${after}${on}`}
          data-col-head={c}
          style={{ left: GUTTER_W + x, width: layout.colX[c + 1]! - x }}
        >
          {colLetters(model.origin.col + c)}
          {onResize && <span className="mmg-resize-col" data-resize-col={c} title="끌어서 열 너비 바꾸기" />}
        </div>,
      );
    }
    return out;
  };

  const rowHeads = (r0: number, r1: number) => {
    const out: ReactNode[] = [];
    for (let r = r0; r <= r1; r++) {
      if (isHidden(layout.rowY, r)) continue;
      const y = layout.rowY[r]!;
      const after = r > 0 && isHidden(layout.rowY, r - 1) ? " mmg-after-hidden" : "";
      const on = sel && r >= sel.row && r < sel.row + sel.rows ? " mmg-head-sel" : "";
      const marker = rowMarker?.(r);
      const gone = cancelled.has(r) ? " mmg-cancelled-head" : "";
      out.push(
        <div
          key={`g${r}`}
          className={`mmg-rowhead${after}${on}${gone}`}
          data-row-head={r}
          style={{ top: HEADER_H + y, height: layout.rowY[r + 1]! - y }}
        >
          {marker && <span className="mmg-dot" style={{ background: marker }} />}
          {model.origin.row + r + 1}
          {onResize && <span className="mmg-resize-row" data-resize-row={r} title="끌어서 행 높이 바꾸기" />}
        </div>,
      );
    }
    return out;
  };

  const emit = (s: GridSelection) => onSelectionChange?.(s);
  /** 끄는 동안의 선택: 고르는 중이면 `pick`으로, 아니면 선택으로. */
  const send = (d: Drag | null, s: GridSelection) => (d?.pick ? pickRef.current?.onChange(s) : emit(s));

  const hit = (e: MouseEvent) =>
    (e.target as HTMLElement).closest<HTMLElement>("[data-r],[data-col-head],[data-row-head]");

  /** 머리글 경계 손잡이를 눌렀으면 크기 바꾸기를 시작한다(선택은 바꾸지 않는다). */
  const startResize = (e: MouseEvent): boolean => {
    if (!onResize) return false;
    const handle = (e.target as HTMLElement).closest<HTMLElement>("[data-resize-col],[data-resize-row]");
    if (!handle) return false;
    e.preventDefault();
    e.stopPropagation();
    const axis = handle.dataset.resizeCol !== undefined ? "col" : "row";
    const index = Number(axis === "col" ? handle.dataset.resizeCol : handle.dataset.resizeRow);
    const offsets = axis === "col" ? layout.colX : layout.rowY;
    const start = offsets[index + 1]! - offsets[index]!;
    const from = axis === "col" ? e.clientX : e.clientY;
    setResize({ axis, index, start, from, size: start });
    return true;
  };

  // 끄는 동안은 창 전체에서 마우스를 따라가고, 놓으면 새 크기를 알린다.
  const resizing = resize !== null;
  const resizeRef = useRef(resize);
  resizeRef.current = resize;
  const onResizeRef = useRef(onResize);
  onResizeRef.current = onResize;
  useEffect(() => {
    if (!resizing) return;
    const axis = resizeRef.current!.axis;
    const cursor = document.body.style.cursor;
    document.body.style.cursor = axis === "col" ? "col-resize" : "row-resize";
    const move = (ev: globalThis.MouseEvent) => {
      const r = resizeRef.current;
      if (!r) return;
      const pos = r.axis === "col" ? ev.clientX : ev.clientY;
      setResize({ ...r, size: Math.max(MIN_RESIZE_PX, Math.round(r.start + pos - r.from)) });
    };
    const up = () => {
      const r = resizeRef.current;
      setResize(null);
      if (r && r.size !== r.start) onResizeRef.current?.(r.axis, r.index, r.size / PT_TO_PX);
    };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
    return () => {
      document.body.style.cursor = cursor;
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
    };
  }, [resizing]);

  const onMouseDown = (e: MouseEvent) => {
    if (e.button !== 0 || startResize(e)) return;
    const picking = pick != null;
    // 고르는 중에는 포커스를 채팅 입력 칸에 둔다(그리드·편집기로 옮기지 않는다, 스크롤 막대를 눌러도).
    if (picking) e.preventDefault();
    else if (!onSelectionChange) return;
    const el = hit(e);
    if (!el) return;
    const shift = !picking && e.shiftKey && selection != null;
    let d: Drag | null = null;
    let s: GridSelection | null = null;
    if (el.dataset.r !== undefined) {
      const at = { row: Number(el.dataset.r), col: Number(el.dataset.c) };
      const anchor = shift ? selection!.anchor : at;
      d = { kind: "cells", anchor: 0, anchorCell: anchor, pick: picking };
      s = { anchor, focus: at };
    } else if (el.dataset.colHead !== undefined) {
      const c = Number(el.dataset.colHead);
      const from = shift ? selection!.anchor.col : c;
      d = { kind: "cols", anchor: from, anchorCell: { row: 0, col: from }, pick: picking };
      s = columnSelection(layout, model.headerRows, from, c);
    } else if (el.dataset.rowHead !== undefined) {
      const r = Number(el.dataset.rowHead);
      const from = shift ? selection!.anchor.row : r;
      d = { kind: "rows", anchor: from, anchorCell: { row: from, col: 0 }, pick: picking };
      s = rowSelection(layout, from, r);
    }
    if (!d || !s) return;
    drag.current = d;
    send(d, s);
  };

  const onMouseOver = (e: MouseEvent) => {
    const d = drag.current;
    if (!d || (e.buttons & 1) === 0) return;
    const el = hit(e);
    if (!el) return;
    if (d.kind === "cells" && el.dataset.r !== undefined) {
      send(d, { anchor: d.anchorCell, focus: { row: Number(el.dataset.r), col: Number(el.dataset.c) } });
    } else if (d.kind === "cols") {
      const c = el.dataset.colHead ?? el.dataset.c;
      if (c !== undefined) send(d, columnSelection(layout, model.headerRows, d.anchor, Number(c)));
    } else if (d.kind === "rows") {
      const r = el.dataset.rowHead ?? el.dataset.r;
      if (r !== undefined) send(d, rowSelection(layout, d.anchor, Number(r)));
    }
  };

  /** `from`에서 한 칸: 병합 영역의 가장자리에서 움직이고 숨긴 행·열은 건너뛴다. 더 갈 곳이 없으면 그대로. */
  const stepFrom = (from: CellRef, dRow: number, dCol: number): CellRef => {
    const box = cellBox(layout, from.row, from.col);
    const lastRow = box.row + box.rows - 1;
    const lastCol = box.col + box.cols - 1;
    let { row, col } = from;
    if (dRow > 0 && nextVisible(layout.rowY, lastRow, 1) !== lastRow) row = nextVisible(layout.rowY, lastRow, 1);
    if (dRow < 0 && nextVisible(layout.rowY, box.row, -1) !== box.row) row = nextVisible(layout.rowY, box.row, -1);
    if (dCol > 0 && nextVisible(layout.colX, lastCol, 1) !== lastCol) col = nextVisible(layout.colX, lastCol, 1);
    if (dCol < 0 && nextVisible(layout.colX, box.col, -1) !== box.col) col = nextVisible(layout.colX, box.col, -1);
    const target = cellBox(layout, row, col);
    return { row: target.row, col: target.col };
  };

  useImperativeHandle(
    handle,
    () => ({
      focus: () => scroller.current?.focus({ preventScroll: true }),
      step: (dRow, dCol) => {
        if (!selection) return null;
        const at = stepFrom(selection.anchor, dRow, dCol);
        emit(cellSelection(at));
        scrollIntoView(scroller.current, layout, cellBox(layout, at.row, at.col));
        return at;
      },
      reveal: (at) => {
        emit(cellSelection(at));
        scrollIntoView(scroller.current, layout, cellBox(layout, at.row, at.col));
      },
      select: (s) => {
        emit(s);
        scrollIntoView(scroller.current, layout, selectionRange(layout, s));
      },
      moveFrom: (from, dRow, dCol) => {
        const at = stepFrom(from, dRow, dCol);
        scrollIntoView(scroller.current, layout, cellBox(layout, at.row, at.col));
        return at;
      },
    }),
    // stepFrom·emit은 layout·selection·onSelectionChange로 정해진다.
    [layout, selection, onSelectionChange],
  );

  const onKeyDown = (e: KeyboardEvent) => {
    onKey?.(e);
    if (e.defaultPrevented || !selection || !onSelectionChange) return;
    const down = e.key === "ArrowDown" || (e.key === "Enter" && !e.shiftKey);
    const up = e.key === "ArrowUp" || (e.key === "Enter" && e.shiftKey);
    const right = e.key === "ArrowRight" || (e.key === "Tab" && !e.shiftKey);
    const left = e.key === "ArrowLeft" || (e.key === "Tab" && e.shiftKey);
    if (!(down || up || right || left)) return;
    e.preventDefault();
    // Shift+방향키는 끝 칸(focus)을 늘리고, 그냥 방향키는 활성 칸을 옮긴다.
    const extend = e.shiftKey && e.key.startsWith("Arrow");
    const from = extend ? selection.focus : selection.anchor;
    const at = stepFrom(from, down ? 1 : up ? -1 : 0, right ? 1 : left ? -1 : 0);
    emit(extend ? { anchor: selection.anchor, focus: at } : cellSelection(at));
    scrollIntoView(scroller.current, layout, cellBox(layout, at.row, at.col));
  };

  const onContext = (e: MouseEvent) => {
    if (pick) {
      e.preventDefault();
      return;
    }
    if (!onContextMenu) return;
    const el = hit(e);
    if (!el) return;
    if (el.dataset.r !== undefined)
      onContextMenu({ kind: "cell", row: Number(el.dataset.r), col: Number(el.dataset.c) }, e);
    else if (el.dataset.colHead !== undefined) onContextMenu({ kind: "col", col: Number(el.dataset.colHead) }, e);
    else if (el.dataset.rowHead !== undefined) onContextMenu({ kind: "row", row: Number(el.dataset.rowHead) }, e);
  };

  const onDoubleClick = (e: MouseEvent) => {
    if (pick) return;
    const el = (e.target as HTMLElement).closest<HTMLElement>("[data-r]");
    if (el && onCellDoubleClick) onCellDoubleClick(Number(el.dataset.r), Number(el.dataset.c));
  };

  const fr = layout.frozenRows;
  const fc = layout.frozenCols;

  // 칸 위의 겹: 선택(범위 틴트·활성 칸)과 다른 사람 표시. 고정 영역은 띠마다 따로 고정되므로 칸과 같은 띠에 그린다.
  // 칸 장식보다 위에 있어 강조·잠금 칸에서도 선택이 보인다.
  const layers: Record<Layer, ReactNode[]> = { corner: [], top: [], left: [], body: [] };
  if (selection && sel) {
    if (multi)
      for (const piece of splitByFrozen(layout, sel))
        layers[piece.layer].push(<div key="sel" className="mmg-sel" style={boxRect(layout, piece.range)} />);
    const a = cellBox(layout, selection.anchor.row, selection.anchor.col);
    layers[layerOf(layout, a.row, a.col)].push(<div key="active" className="mmg-active" style={boxRect(layout, a)} />);
  }
  for (const g of ranges ?? []) {
    const range = clampRange(layout, g.range);
    if (!range) continue;
    for (const [i, piece] of splitByFrozen(layout, range).entries()) {
      const css = boxRect(layout, piece.range);
      if (css.width === 0 || css.height === 0) continue;
      layers[piece.layer].push(
        g.kind === "pick" ? (
          <div key={`r:${g.key}:${i}`} className="mmg-pick" style={css}>
            <span className="t" />
            <span className="r" />
            <span className="b" />
            <span className="l" />
          </div>
        ) : (
          <div key={`r:${g.key}:${i}`} className="mmg-ping" style={css} />
        ),
      );
    }
  }
  for (const m of marks ?? []) {
    const box = cellBox(layout, m.row, m.col);
    const r = boxRect(layout, box);
    if (r.width === 0 || r.height === 0) continue; // 숨긴 행·열
    // 스크롤되는 행이 고정 머리 바로 아래에 있으면 위쪽 이름표가 머리에 가린다: 칸 아래로 보낸다.
    const below = box.row >= fr && r.top < vp.top + cornerH + TAG_H;
    layers[layerOf(layout, box.row, box.col)].push(
      <div
        key={`m:${m.key}`}
        className={`mmg-mark${m.lock ? " lock" : ""}${below ? " below" : ""}`}
        data-r={box.row}
        data-c={box.col}
        style={
          {
            transform: `translate(${r.left}px, ${r.top}px)`,
            width: r.width,
            height: r.height,
            "--mark": m.color,
          } as CSSProperties
        }
      >
        {m.label && <span className={`peer-tag${m.lock ? " lock-tag" : ""}`}>{m.label}</span>}
      </div>,
    );
  }

  return (
    <div
      ref={scroller}
      className={`mmg-scroller${pick ? " mmg-picking" : ""}${className ? ` ${className}` : ""}`}
      style={style}
      tabIndex={0}
      onScroll={onScroll}
      onMouseDown={onMouseDown}
      onMouseOver={onMouseOver}
      onKeyDown={onKeyDown}
      onDoubleClick={onDoubleClick}
      onContextMenu={onContext}
    >
      <div className="mmg-content" style={{ width: totalW, height: contentH }}>
        <div className="mmg-band mmg-corner" style={{ width: cornerW, height: cornerH, marginBottom: -cornerH }}>
          <div className="mmg-colhead mmg-origin" style={{ left: 0, width: GUTTER_W }} />
          {colHeads(0, fc - 1)}
          {rowHeads(0, fr - 1)}
          {renderBlock(0, fr - 1, 0, fc - 1)}
          {layers.corner}
        </div>
        <div className="mmg-band mmg-top" style={{ width: totalW, height: cornerH, marginBottom: -cornerH }}>
          {colHeads(colsSpan.first, colsSpan.last)}
          {renderBlock(0, fr - 1, colsSpan.first, colsSpan.last)}
          {layers.top}
        </div>
        <div className="mmg-band mmg-left" style={{ width: cornerW, height: contentH, marginBottom: -contentH }}>
          {rowHeads(rowsSpan.first, rowsSpan.last)}
          {rowFooter && (
            // 그리드가 이 안의 클릭·키를 칸 선택·이동으로 받지 않게 한다.
            <div
              className="mmg-rowfoot"
              style={{ top: totalH, height: FOOTER_H }}
              onMouseDown={(e) => e.stopPropagation()}
              onKeyDown={(e) => e.stopPropagation()}
              onDoubleClick={(e) => e.stopPropagation()}
            >
              {rowFooter}
            </div>
          )}
          {renderBlock(rowsSpan.first, rowsSpan.last, 0, fc - 1)}
          {layers.left}
        </div>
        {renderBlock(rowsSpan.first, rowsSpan.last, colsSpan.first, colsSpan.last)}
        {layers.body}
        {overlay && (
          <div className="mmg-overlay" style={boxRect(layout, cellBox(layout, overlay.at.row, overlay.at.col))}>
            {overlay.node}
          </div>
        )}
        {resize && <ResizeGuide resize={resize} layout={layout} vp={vp} />}
      </div>
    </div>
  );
}

/** 크기를 바꾸는 동안의 안내선과 크기(pt) 표시. 고정 영역의 줄은 스크롤해도 제자리에 있으므로 스크롤만큼 옮긴다. */
function ResizeGuide({ resize, layout, vp }: { resize: Resize; layout: GridLayout; vp: Viewport }) {
  const pt = Math.round((resize.size / PT_TO_PX) * 4) / 4;
  if (resize.axis === "col") {
    const frozen = resize.index < layout.frozenCols ? vp.left : 0;
    const x = GUTTER_W + layout.colX[resize.index]! + resize.size + frozen;
    return (
      <>
        <div className="mmg-guide mmg-guide-col" style={{ left: x - 1, top: vp.top, height: vp.height }} />
        <div className="mmg-guide-label" style={{ left: x + 4, top: vp.top + HEADER_H + 4 }}>
          너비 {pt}pt
        </div>
      </>
    );
  }
  const frozen = resize.index < layout.frozenRows ? vp.top : 0;
  const y = HEADER_H + layout.rowY[resize.index]! + resize.size + frozen;
  return (
    <>
      <div className="mmg-guide mmg-guide-row" style={{ top: y - 1, left: vp.left, width: vp.width }} />
      <div className="mmg-guide-label" style={{ top: y + 4, left: vp.left + GUTTER_W + 4 }}>
        높이 {pt}pt
      </div>
    </>
  );
}

/** 범위를 시트 안으로 줄인다(채팅에 적힌 주소가 시트보다 클 수 있다). 시트 밖이면 `null`. */
function clampRange(layout: GridLayout, r: CellRange): CellRange | null {
  const row = Math.max(0, r.row);
  const col = Math.max(0, r.col);
  const lastRow = Math.min(layout.rows, r.row + r.rows);
  const lastCol = Math.min(layout.cols, r.col + r.cols);
  if (lastRow <= row || lastCol <= col) return null;
  return { row, col, rows: lastRow - row, cols: lastCol - col };
}

/** 고정 영역에 가리지 않도록 스크롤을 옮긴다. 화면보다 큰 범위는 위·왼쪽을 맞춘다. */
function scrollIntoView(el: HTMLDivElement | null, layout: GridLayout, box: CellRange) {
  if (!el) return;
  const range = clampRange(layout, box);
  if (!range) return;
  const r = boxRect(layout, range);
  if (range.row >= layout.frozenRows) {
    const maxTop = r.top - HEADER_H - layout.frozenH;
    const minTop = Math.min(r.top + r.height - el.clientHeight, maxTop);
    if (el.scrollTop > maxTop) el.scrollTop = maxTop;
    else if (el.scrollTop < minTop) el.scrollTop = minTop;
  }
  if (range.col >= layout.frozenCols) {
    const maxLeft = r.left - GUTTER_W - layout.frozenW;
    const minLeft = Math.min(r.left + r.width - el.clientWidth, maxLeft);
    if (el.scrollLeft > maxLeft) el.scrollLeft = maxLeft;
    else if (el.scrollLeft < minLeft) el.scrollLeft = minLeft;
  }
}
