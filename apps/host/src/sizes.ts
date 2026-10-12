import { resizeRuns } from "@mm/grid";
import type { Axis, CellRange, FormatEdit, SheetModel } from "@mm/shared";

/**
 * 머리글 경계를 끈 결과(1b·1d): 머리글로 여러 열(행)을 골라 두고 그중 하나를 끌면 고른 열(행) 모두를 같은 크기로(Excel처럼).
 * `shown`은 화면에 그리는 시트라 필터·참여자 시점으로 숨은 행은 건너뛴다(보이는 구간마다 하나씩). 세션은 바뀐 크기만
 * 참여자·호스트 화면에 알린다(`resized`).
 */
export function sizeEdits(
  shown: SheetModel,
  range: CellRange | null,
  axis: Axis,
  index: number,
  size: number,
): FormatEdit[] {
  return resizeRuns(shown, range, axis, index).map(({ from, count }) =>
    axis === "col" ? { op: "colWidth", from, count, width: size } : { op: "rowHeight", from, count, height: size },
  );
}
