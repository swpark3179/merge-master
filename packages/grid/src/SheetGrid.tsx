import type { BorderEdge, Cell, CellRange, CellRef, CellStyle, SheetModel } from "@mm/shared";
import { colLetters, formatValue, PT_TO_PX, rgbHex } from "@mm/shared";
import type { CSSProperties, KeyboardEvent, MouseEvent, ReactNode, Ref } from "react";
import { useCallback, useEffect, useImperativeHandle, useLayoutEffect, useMemo, useRef, useState } from "react";

import "./grid.css";
import { boxRect, buildLayout, cellBox, GUTTER_W, HEADER_H, isHidden, nextVisible, visibleSpan } from "./layout";
import type { GridLayout } from "./layout";
import type { GridSelection } from "./selection";
import { cellSelection, columnSelection, containsCell, intersects, rowSelection, selectionRange } from "./selection";

/** 화면에 겹쳐 그릴 표시(입력 금지 빗금, 필수 미입력 표시, 편집 중 이름표 …). */
export interface CellDecoration {
  className?: string;
  style?: CSSProperties;
  /** 셀 안 오른쪽에 붙는 요소(배지·잠금 표시 등). */
  adornment?: ReactNode;
}

/** 바깥에서 그리드를 움직일 때(편집을 확정하고 다음 칸으로 등). */
export interface SheetGridHandle {
  focus(): void;
  /** 활성 칸에서 한 칸 움직인다(병합·숨긴 행 고려). 움직인 칸을 돌려준다. */
  step(dRow: -1 | 0 | 1, dCol: -1 | 0 | 1): CellRef | null;
  /** 칸이 보이도록 스크롤하고 그 칸을 선택한다. */
  reveal(at: CellRef): void;
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
  /** 방향키 이동보다 먼저 받는다. `preventDefault()`하면 그리드는 이동하지 않는다. */
  onKey?: (e: KeyboardEvent) => void;
  onCellDoubleClick?: (row: number, col: number) => void;
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
    textDecoration: s.underline ? "underline" : undefined,
    color: s.color ? rgbHex(s.color) : undefined,
    // background 단축 속성은 장식 클래스의 background-image(빗금)까지 지우므로 색만 지정한다.
    backgroundColor: s.fill !== undefined ? rgbHex(s.fill) : undefined,
    alignItems: s.vAlign === "top" ? "flex-start" : s.vAlign === "center" ? "center" : "flex-end",
    whiteSpace: s.wrap ? "normal" : undefined,
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

type Drag = { kind: "cells" | "cols" | "rows"; anchor: number; anchorCell: { row: number; col: number } };

export function SheetGrid({
  model,
  handle,
  decorate,
  selection,
  onSelectionChange,
  rowMarker,
  cellText,
  overlay,
  onKey,
  onCellDoubleClick,
  overscan = 6,
  className,
  style,
}: SheetGridProps) {
  const layout = useMemo(() => buildLayout(model), [model]);
  const styleCss = useMemo(() => model.styles.map(baseCss), [model]);
  const scroller = useRef<HTMLDivElement>(null);
  const [vp, setVp] = useState<Viewport>({ top: 0, left: 0, width: 800, height: 600 });
  const frame = useRef(0);
  const drag = useRef<Drag | null>(null);

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

  useEffect(() => {
    const up = () => (drag.current = null);
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
    const active = selection != null && containsCell(box, selection.anchor.row, selection.anchor.col);
    const inSel = multi && sel !== null && intersects(sel, box);
    const css: CSSProperties = {
      ...boxRect(layout, box),
      ...styleCss[sid],
      justifyContent: justify(s, typeof cell?.v === "number"),
      borderBottom: borderCss(s?.borders.bottom ?? below?.borders.top),
      borderRight: borderCss(s?.borders.right ?? right?.borders.left),
      ...deco?.style,
    };
    const cls = `mmg-cell${active ? " mmg-selected" : ""}${inSel ? " mmg-insel" : ""}${deco?.className ? ` ${deco.className}` : ""}`;
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
      out.push(
        <div
          key={`g${r}`}
          className={`mmg-rowhead${after}${on}`}
          data-row-head={r}
          style={{ top: HEADER_H + y, height: layout.rowY[r + 1]! - y }}
        >
          {marker && <span className="mmg-dot" style={{ background: marker }} />}
          {model.origin.row + r + 1}
        </div>,
      );
    }
    return out;
  };

  const emit = (s: GridSelection) => onSelectionChange?.(s);

  const hit = (e: MouseEvent) =>
    (e.target as HTMLElement).closest<HTMLElement>("[data-r],[data-col-head],[data-row-head]");

  const onMouseDown = (e: MouseEvent) => {
    if (e.button !== 0 || !onSelectionChange) return;
    const el = hit(e);
    if (!el) return;
    const shift = e.shiftKey && selection != null;
    if (el.dataset.r !== undefined) {
      const at = { row: Number(el.dataset.r), col: Number(el.dataset.c) };
      const anchor = shift ? selection!.anchor : at;
      drag.current = { kind: "cells", anchor: 0, anchorCell: anchor };
      emit({ anchor, focus: at });
    } else if (el.dataset.colHead !== undefined) {
      const c = Number(el.dataset.colHead);
      const from = shift ? selection!.anchor.col : c;
      drag.current = { kind: "cols", anchor: from, anchorCell: { row: 0, col: from } };
      emit(columnSelection(layout, model.headerRows, from, c));
    } else if (el.dataset.rowHead !== undefined) {
      const r = Number(el.dataset.rowHead);
      const from = shift ? selection!.anchor.row : r;
      drag.current = { kind: "rows", anchor: from, anchorCell: { row: from, col: 0 } };
      emit(rowSelection(layout, from, r));
    }
  };

  const onMouseOver = (e: MouseEvent) => {
    const d = drag.current;
    if (!d || (e.buttons & 1) === 0) return;
    const el = hit(e);
    if (!el) return;
    if (d.kind === "cells" && el.dataset.r !== undefined) {
      emit({ anchor: d.anchorCell, focus: { row: Number(el.dataset.r), col: Number(el.dataset.c) } });
    } else if (d.kind === "cols") {
      const c = el.dataset.colHead ?? el.dataset.c;
      if (c !== undefined) emit(columnSelection(layout, model.headerRows, d.anchor, Number(c)));
    } else if (d.kind === "rows") {
      const r = el.dataset.rowHead ?? el.dataset.r;
      if (r !== undefined) emit(rowSelection(layout, d.anchor, Number(r)));
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

  const onDoubleClick = (e: MouseEvent) => {
    const el = (e.target as HTMLElement).closest<HTMLElement>("[data-r]");
    if (el && onCellDoubleClick) onCellDoubleClick(Number(el.dataset.r), Number(el.dataset.c));
  };

  const fr = layout.frozenRows;
  const fc = layout.frozenCols;
  return (
    <div
      ref={scroller}
      className={`mmg-scroller${className ? ` ${className}` : ""}`}
      style={style}
      tabIndex={0}
      onScroll={onScroll}
      onMouseDown={onMouseDown}
      onMouseOver={onMouseOver}
      onKeyDown={onKeyDown}
      onDoubleClick={onDoubleClick}
    >
      <div className="mmg-content" style={{ width: totalW, height: totalH }}>
        <div className="mmg-band mmg-corner" style={{ width: cornerW, height: cornerH, marginBottom: -cornerH }}>
          <div className="mmg-colhead mmg-origin" style={{ left: 0, width: GUTTER_W }} />
          {colHeads(0, fc - 1)}
          {rowHeads(0, fr - 1)}
          {renderBlock(0, fr - 1, 0, fc - 1)}
        </div>
        <div className="mmg-band mmg-top" style={{ width: totalW, height: cornerH, marginBottom: -cornerH }}>
          {colHeads(colsSpan.first, colsSpan.last)}
          {renderBlock(0, fr - 1, colsSpan.first, colsSpan.last)}
        </div>
        <div className="mmg-band mmg-left" style={{ width: cornerW, height: totalH, marginBottom: -totalH }}>
          {rowHeads(rowsSpan.first, rowsSpan.last)}
          {renderBlock(rowsSpan.first, rowsSpan.last, 0, fc - 1)}
        </div>
        {renderBlock(rowsSpan.first, rowsSpan.last, colsSpan.first, colsSpan.last)}
        {overlay && (
          <div className="mmg-overlay" style={boxRect(layout, cellBox(layout, overlay.at.row, overlay.at.col))}>
            {overlay.node}
          </div>
        )}
      </div>
    </div>
  );
}

/** 고정 영역에 가리지 않도록 스크롤을 옮긴다. */
function scrollIntoView(el: HTMLDivElement | null, layout: GridLayout, box: CellRange) {
  if (!el) return;
  const r = boxRect(layout, box);
  if (box.row >= layout.frozenRows) {
    const minTop = r.top + r.height - el.clientHeight;
    const maxTop = r.top - HEADER_H - layout.frozenH;
    if (el.scrollTop > maxTop) el.scrollTop = maxTop;
    else if (el.scrollTop < minTop) el.scrollTop = minTop;
  }
  if (box.col >= layout.frozenCols) {
    const minLeft = r.left + r.width - el.clientWidth;
    const maxLeft = r.left - GUTTER_W - layout.frozenW;
    if (el.scrollLeft > maxLeft) el.scrollLeft = maxLeft;
    else if (el.scrollLeft < minLeft) el.scrollLeft = minLeft;
  }
}
