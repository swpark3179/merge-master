export interface BenchResult {
  frames: number;
  avgFps: number;
  p95FrameMs: number;
  maxFrameMs: number;
  /** 측정 중 그리드에 있던 셀 요소 수의 최댓값(가상화가 동작하는지). */
  maxCells: number;
}

/**
 * 스크롤 성능 측정: 그리드를 매 프레임 일정량씩 아래로(끝에 닿으면 위로) 굴리며 프레임 간격을 잰다.
 * `?bench` 로 열면 자동 실행된다.
 */
export function runScrollBench(scroller: HTMLElement, durationMs = 4000, pxPerFrame = 60): Promise<BenchResult> {
  return new Promise((resolve) => {
    const deltas: number[] = [];
    let maxCells = 0;
    let dir = 1;
    let last = performance.now();
    const start = last;
    const tick = (now: number) => {
      deltas.push(now - last);
      last = now;
      maxCells = Math.max(maxCells, scroller.querySelectorAll(".mmg-cell").length);
      const max = scroller.scrollHeight - scroller.clientHeight;
      if (scroller.scrollTop >= max) dir = -1;
      else if (scroller.scrollTop <= 0) dir = 1;
      scroller.scrollTop += dir * pxPerFrame;
      if (now - start < durationMs) {
        requestAnimationFrame(tick);
        return;
      }
      const sorted = deltas.slice(1).sort((a, b) => a - b);
      const avg = sorted.reduce((s, d) => s + d, 0) / Math.max(sorted.length, 1);
      resolve({
        frames: sorted.length,
        avgFps: Math.round(1000 / avg),
        p95FrameMs: Math.round(sorted[Math.floor(sorted.length * 0.95)] ?? 0),
        maxFrameMs: Math.round(sorted[sorted.length - 1] ?? 0),
        maxCells,
      });
    };
    requestAnimationFrame(tick);
  });
}
