import type { GridHit, GridSelection } from "@mm/grid";
import { ContextMenu, MenuItem, MenuNote, MenuSep, SizeControl } from "@mm/grid";
import type { Axis, CellRange, ConfigView, SheetModel, StructureEdit } from "@mm/shared";
import { a1, colLetters } from "@mm/shared";

/**
 * 1b 설정·1d 모니터링 그리드의 오른쪽 클릭 메뉴(호스트): 열 너비·행 높이(슬라이더 + 값), 표 아래에 행·오른쪽에 열 추가,
 * 앱에서 붙인 행·열 지우기, 행 취소 표시. 지울 수 없는 행·열이면 까닭을 보인다(세션도 같은 규칙으로 한 번 더 검사한다):
 * 엑셀에서 불러온 행·열, 담당자가 배정된 행·열(행 배정 기준 칸에 이름이 있거나 범위 배정이 걸침), 행 배정 기준 열.
 */
export interface GridMenuState {
  x: number;
  y: number;
  hit: GridHit;
  /** 값이 있는 행·열을 지우기 전 확인 중. */
  confirm?: { edit: StructureEdit; what: string; filled: number };
}

function rectOf(sel: GridSelection): CellRange {
  return {
    row: Math.min(sel.anchor.row, sel.focus.row),
    col: Math.min(sel.anchor.col, sel.focus.col),
    rows: Math.abs(sel.anchor.row - sel.focus.row) + 1,
    cols: Math.abs(sel.anchor.col - sel.focus.col) + 1,
  };
}

/** 행 `from`부터 `count`개를 지울 수 없는 까닭(지울 수 있으면 `null`). */
export function rowBlock(view: ConfigView, from: number, count: number): string | null {
  if (from < view.baselineRows) return "엑셀에서 불러온 행은 지울 수 없습니다(앱에서 추가한 행만 지울 수 있습니다)";
  for (let r = from; r < from + count; r++) {
    const p = view.rowOwner[r] ?? -1;
    if (p >= 0) return `담당자(${view.roster[p]})가 배정된 행은 지울 수 없습니다. 행 배정 기준 칸을 비운 뒤 지우세요`;
    const a = view.config.assignment.ranges.find((x) => x.range.row <= r && r < x.range.row + x.range.rows);
    if (a) return `담당자(${a.person.trim()})가 배정된 범위가 걸친 행은 지울 수 없습니다`;
  }
  return null;
}

/** 열 `from`부터 `count`개를 지울 수 없는 까닭(지울 수 있으면 `null`). */
export function colBlock(view: ConfigView, from: number, count: number): string | null {
  if (from < view.baselineCols) return "엑셀에서 불러온 열은 지울 수 없습니다(앱에서 추가한 열만 지울 수 있습니다)";
  const key = view.config.assignment.keyCol;
  if (key !== undefined && key !== null && key >= from && key < from + count)
    return "행 배정 기준 열은 지울 수 없습니다";
  const a = view.config.assignment.ranges.find((x) => x.range.col < from + count && from < x.range.col + x.range.cols);
  if (a) return `담당자(${a.person.trim()})가 배정된 열은 지울 수 없습니다`;
  return null;
}

/** 범위 안 값이 있는 칸 수(수식 칸 빼고). */
function filledIn(model: SheetModel, range: CellRange): number {
  let n = 0;
  for (let r = range.row; r < range.row + range.rows; r++)
    for (let c = range.col; c < range.col + range.cols; c++) {
      const cell = model.cells[r * model.cols + c];
      if (cell && !cell.f && cell.v !== undefined && cell.v !== null && cell.v !== "") n++;
    }
  return n;
}

export function GridMenu({
  menu,
  model,
  shown,
  view,
  selection,
  onClose,
  onConfirm,
  onResize,
  onStructure,
}: {
  menu: GridMenuState;
  /** 세션의 시트(값·구조). */
  model: SheetModel;
  /** 화면에 그리는 시트(필터·참여자 시점으로 숨은 행은 크기 0). 크기 조절이 이것을 본다. */
  shown: SheetModel;
  view: ConfigView;
  /** 메뉴를 열 때 고른 범위(`selectForMenu`). */
  selection: GridSelection | null;
  onClose: () => void;
  /** 지우기 전 확인 단계로(같은 자리에 확인을 보인다). */
  onConfirm: (confirm: NonNullable<GridMenuState["confirm"]>) => void;
  onResize: (axis: Axis, index: number, size: number) => void;
  onStructure: (edit: StructureEdit) => void;
}) {
  const { hit } = menu;
  const r = selection ? rectOf(selection) : { row: 0, col: 0, rows: 1, cols: 1 };
  const o = model.origin;
  const rowLabel = (a: number, n: number) => (n > 1 ? `${o.row + a + 1}~${o.row + a + n}행` : `${o.row + a + 1}행`);
  const colLabel = (a: number, n: number) =>
    n > 1 ? `${colLetters(o.col + a)}~${colLetters(o.col + a + n - 1)}열` : `${colLetters(o.col + a)}열`;
  const title =
    hit.kind === "col"
      ? colLabel(r.col, r.cols)
      : hit.kind === "row"
        ? rowLabel(r.row, r.rows)
        : r.rows === 1 && r.cols === 1
          ? a1(o.row + r.row, o.col + r.col)
          : `${a1(o.row + r.row, o.col + r.col)}:${a1(o.row + r.row + r.rows - 1, o.col + r.col + r.cols - 1)}`;

  const run = (edit: StructureEdit) => {
    onClose();
    onStructure(edit);
  };
  /** 값이 있으면 먼저 묻는다. */
  const remove = (edit: StructureEdit, what: string, range: CellRange) => {
    const filled = filledIn(model, range);
    if (filled > 0) onConfirm({ edit, what, filled });
    else run(edit);
  };

  if (menu.confirm) {
    const c = menu.confirm;
    return (
      <ContextMenu x={menu.x} y={menu.y} onClose={onClose} label="지우기 확인">
        <MenuNote title>{c.what} 삭제</MenuNote>
        <MenuNote>값이 있는 칸 {c.filled.toLocaleString()}개도 함께 지워집니다. 지울까요?</MenuNote>
        <div className="mmg-menu-actions">
          <button type="button" className="danger" data-menu-focus onClick={() => run(c.edit)}>
            삭제
          </button>
          <button type="button" onClick={onClose}>
            취소
          </button>
        </div>
      </ContextMenu>
    );
  }

  const rowsData = r.row >= model.headerRows;
  const rowsEdit = (hit.kind === "row" || hit.kind === "cell") && rowsData;
  const colsEdit = hit.kind === "col" || hit.kind === "cell";
  const rowWhy = rowsEdit ? rowBlock(view, r.row, r.rows) : null;
  const colWhy = colsEdit ? colBlock(view, r.col, r.cols) : null;
  const cancelled = new Set(model.cancelledRows ?? []);
  const allCancelled = rowsEdit && Array.from({ length: r.rows }, (_, k) => r.row + k).every((x) => cancelled.has(x));
  const rowsRange = { row: r.row, col: 0, rows: r.rows, cols: model.cols };
  const colsRange = { row: 0, col: r.col, rows: model.rows, cols: r.cols };

  return (
    <ContextMenu x={menu.x} y={menu.y} onClose={onClose} label="표 메뉴">
      <MenuNote title>{title}</MenuNote>
      {hit.kind === "col" && (
        <SizeControl
          axis="col"
          value={shown.colWidths[hit.col] ?? 48}
          count={r.cols}
          onCommit={(pt) => onResize("col", hit.col, pt)}
        />
      )}
      {hit.kind === "row" && (
        <SizeControl
          axis="row"
          value={shown.rowHeights[hit.row] ?? 15}
          count={r.rows}
          onCommit={(pt) => onResize("row", hit.row, pt)}
        />
      )}
      {hit.kind !== "cell" && <MenuSep />}
      {hit.kind !== "col" && (
        <MenuItem onClick={() => run({ op: "addRows", count: 1 })}>＋ 표 맨 아래에 행 추가</MenuItem>
      )}
      {hit.kind !== "row" && (
        <MenuItem onClick={() => run({ op: "addCols", count: 1 })}>＋ 표 맨 오른쪽에 열 추가</MenuItem>
      )}
      {rowsEdit && (
        <MenuItem onClick={() => run({ op: "cancelRows", from: r.row, count: r.rows, on: !allCancelled })}>
          {allCancelled ? "취소 표시 풀기" : "취소 행으로 표시"}
        </MenuItem>
      )}
      {(rowsEdit || colsEdit) && <MenuSep />}
      {rowsEdit && (
        <>
          <MenuItem
            danger
            disabled={rowWhy !== null}
            title={rowWhy ?? undefined}
            onClick={() => remove({ op: "deleteRows", from: r.row, count: r.rows }, rowLabel(r.row, r.rows), rowsRange)}
          >
            행 삭제{r.rows > 1 ? ` (${r.rows}행)` : ""}
          </MenuItem>
          {rowWhy && <MenuNote>{rowWhy}</MenuNote>}
        </>
      )}
      {colsEdit && (
        <>
          <MenuItem
            danger
            disabled={colWhy !== null}
            title={colWhy ?? undefined}
            onClick={() => remove({ op: "deleteCols", from: r.col, count: r.cols }, colLabel(r.col, r.cols), colsRange)}
          >
            열 삭제{r.cols > 1 ? ` (${r.cols}열)` : ""}
          </MenuItem>
          {colWhy && <MenuNote>{colWhy}</MenuNote>}
        </>
      )}
    </ContextMenu>
  );
}
