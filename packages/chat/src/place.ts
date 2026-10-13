/** 화면 좌표의 사각형(getBoundingClientRect와 같은 모양). */
export interface Rect {
  left: number;
  top: number;
  width: number;
  height: number;
}

export interface Placement {
  left: number;
  top: number;
  /** 기준의 아래에 띄웠는지 위에 띄웠는지(나타나는 방향). */
  side: "below" | "above";
}

/**
 * 팝오버 자리: 기준(칩) 아래에 띄우고, 아래가 모자라고 위가 넉넉하면 위로 뒤집는다. 가로는 기준 왼쪽에 맞추되 화면 안으로
 * 당긴다(오른쪽 사이드바의 칩도 잘리지 않게).
 */
export function placePopover(
  anchor: Rect,
  size: { width: number; height: number },
  viewport: { width: number; height: number },
  gap = 6,
  edge = 8,
): Placement {
  const below = anchor.top + anchor.height + gap;
  const above = anchor.top - gap - size.height;
  const roomBelow = viewport.height - edge - below;
  const roomAbove = anchor.top - gap - edge;
  const side = size.height <= roomBelow || roomBelow >= roomAbove ? "below" : "above";
  const top = side === "below" ? below : Math.max(edge, above);
  const left = Math.max(edge, Math.min(anchor.left, viewport.width - size.width - edge));
  return { left, top, side };
}
