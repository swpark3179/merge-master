import type { CellValue, SheetModel } from "@mm/shared";
import { formatValue } from "@mm/shared";

/**
 * 행 필터 공용 도구(참여자 1f 조회조건, 호스트 1d 모니터링). 걸러진 데이터 행은 높이 0(숨김)으로 그린다.
 * 병합 영역은 그리드가 보이는 행만으로 다시 그리므로(높이 합) 따로 쪼개지 않아도 된다.
 */

/** 병합을 고려한 열 값(병합 영역 안이면 왼쪽 위 칸의 값). */
export function effectiveColumn(model: SheetModel, col: number): (CellValue | undefined)[] {
  const out: (CellValue | undefined)[] = [];
  for (let r = 0; r < model.rows; r++) out.push(model.cells[r * model.cols + col]?.v);
  for (const m of model.merges) {
    if (col < m.col || col >= m.col + m.cols) continue;
    const v = model.cells[m.row * model.cols + m.col]?.v;
    for (let r = m.row; r < m.row + m.rows && r < model.rows; r++) out[r] = v;
  }
  return out;
}

/** 칸 값의 표시 글자(그 칸의 표시 형식대로). */
export function cellText(model: SheetModel, v: CellValue | undefined, row: number, col: number): string {
  const s = model.styles[model.cells[row * model.cols + col]?.s ?? 0];
  return formatValue(v, s?.numberFormat ?? "");
}

/** 조회조건 드롭다운 항목: 보이는 데이터 행의 값(처음 나온 순서, 빈 값 제외). */
export function filterOptions(model: SheetModel, col: number): string[] {
  const values = effectiveColumn(model, col);
  const hidden = new Set(model.hiddenRows);
  const seen = new Set<string>();
  for (let r = model.headerRows; r < model.rows; r++) {
    if (hidden.has(r)) continue;
    const t = cellText(model, values[r], r, col);
    if (t !== "") seen.add(t);
  }
  return [...seen];
}

/** 조회조건(열 → 고른 값)에 맞지 않는 데이터 행. */
export function rowsNotMatching(model: SheetModel, values: Record<number, string>): Set<number> {
  const out = new Set<number>();
  const cols = Object.entries(values).map(([c, want]) => ({
    col: Number(c),
    want,
    values: effectiveColumn(model, Number(c)),
  }));
  if (cols.length === 0) return out;
  for (let r = model.headerRows; r < model.rows; r++) {
    if (!cols.every((c) => cellText(model, c.values[r], r, c.col) === c.want)) out.add(r);
  }
  return out;
}

/** 걸러진 행을 높이 0으로 만든 보기용 시트(값·셀 배열은 원본과 같은 것을 쓴다). */
export function withHiddenRows(model: SheetModel, hide: Set<number>): SheetModel {
  if (hide.size === 0) return model;
  const rowHeights = model.rowHeights.map((h, r) => (hide.has(r) ? 0 : h));
  const hiddenRows = [...new Set([...model.hiddenRows, ...hide])].sort((a, b) => a - b);
  return { ...model, rowHeights, hiddenRows };
}
