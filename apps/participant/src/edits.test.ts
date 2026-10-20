import type { CellStyle, SheetModel, SheetView } from "@mm/shared";
import { describe, expect, it } from "vitest";

import { EditState, sameValue } from "./edits";

function welcome(over: Partial<SheetView> = {}): SheetView {
  const model: SheetModel = {
    workbook: "a.xlsx",
    sheet: "s",
    origin: { row: 0, col: 0 },
    rows: 3,
    cols: 2,
    cells: Array.from({ length: 6 }, () => ({ s: 0 })),
    styles: [],
    merges: [],
    colWidths: [60, 60],
    rowHeights: [15, 15, 15],
    headerRows: 1,
    frozenRows: 1,
    frozenCols: 0,
    validations: [],
    hiddenRows: [],
    hiddenCols: [],
  };
  return {
    model,
    flags: "",
    versions: [],
    locks: [],
    columns: [],
    labels: [],
    permissions: {
      format: "none",
      othersRows: "readOnly",
      filterSort: true,
      addRows: false,
      deleteRows: false,
      deadlineAction: "readOnly",
      resize: false,
      overwriteFormulas: true,
    },
    rowOwner: [-1, 2, 2],
    progress: { name: "김민지", rows: 2, required: 0, filled: 0 },
    assigned: true,
    ...over,
  };
}

/** 표 아래에 행이 붙은 새 시트(같은 통합문서·시트, 행만 늘어남). */
function grown(w: SheetView, extra: number): SheetView {
  const m = w.model;
  const model: SheetModel = {
    ...m,
    rows: m.rows + extra,
    cells: [...m.cells.map((c) => ({ ...c })), ...Array.from({ length: extra * m.cols }, () => ({ s: 0 }))],
    rowHeights: [...m.rowHeights, ...Array(extra).fill(15)],
  };
  return { ...w, model, rowOwner: [...w.rowOwner, ...Array(extra).fill(2)] };
}

describe("EditState", () => {
  it("표 아래에 행이 붙어도 되돌리기 기록은 남고, 다른 시트로 바뀌면 지운다", () => {
    const w = welcome();
    const s = new EditState(w, "c");
    s.applied(s.edit([{ row: 1, col: 1, value: 5 }])!.batch, [{ row: 1, col: 1, value: 5, ver: 1 }], [], []);
    const next = grown(
      { ...w, model: { ...w.model, cells: w.model.cells.map((c, i) => (i === 3 ? { ...c, v: 5 } : c)) } },
      2,
    );
    s.load(next);
    expect(s.model.rows).toBe(5);
    expect(s.canUndo()).toBe(true);
    s.load({ ...next, model: { ...next.model, sheet: "다른 시트", rows: 2, cells: next.model.cells.slice(0, 4) } });
    expect(s.canUndo()).toBe(false);
  });

  it("행·열을 지워 구조 판이 바뀌면 되돌리기를 지우고, 그 전 묶음은 새 시트에 그리지 않는다", () => {
    const w = welcome();
    const s = new EditState(w, "c");
    const b = s.edit([{ row: 2, col: 1, value: 7 }])!;
    expect(b.layout).toBe(0);
    // 오른쪽에 열만 붙은 것(같은 판)은 좌표가 그대로다: 되돌리기·대기 칸 모두 남는다.
    const wide: SheetView = {
      ...w,
      model: { ...w.model, cols: 3, cells: Array.from({ length: 9 }, () => ({ s: 0 })), colWidths: [60, 60, 60] },
    };
    s.load(wide);
    expect(s.canUndo()).toBe(true);
    expect(s.value(2, 1)).toBe(7);
    // 앱에서 붙인 행을 지웠다(판 1): 이 묶음은 서버가 받지 않으니 밀린 칸에 그리지 않는다.
    s.load({ ...wide, model: { ...wide.model, layout: 1, cells: Array.from({ length: 9 }, () => ({ s: 0 })) } });
    expect(s.canUndo()).toBe(false);
    expect(s.value(2, 1)).toBeNull();
    expect(s.edit([{ row: 1, col: 1, value: 1 }])!.layout).toBe(1);
  });

  it("낙관적으로 그리고, 답이 오면 서버가 정규화한 값으로 바꾼다", () => {
    const s = new EditState(welcome(), "c");
    const b = s.edit([{ row: 1, col: 1, value: "9,800" }])!;
    expect(b.cells).toEqual([{ row: 1, col: 1, value: "9,800", base: 0 }]);
    expect(s.value(1, 1)).toBe("9,800");
    expect(s.pending.size).toBe(1);
    s.applied(b.batch, [{ row: 1, col: 1, value: 9800, ver: 1 }], [], []);
    expect(s.value(1, 1)).toBe(9800);
    expect(s.pending.size).toBe(0);
    expect(s.outbox).toEqual([]);
    expect(s.versions.get(3)).toBe(1);
    expect(s.myChanges).toBe(1);
    // 같은 값이면 보내지 않는다.
    expect(s.edit([{ row: 1, col: 1, value: 9800 }])).toBeNull();
  });

  it("수식 칸은 다른 값을 넣을 때만 보낸다(칸을 거쳐 가기만 하면 수식이 그대로)", () => {
    const w = welcome();
    w.model.cells[3] = { s: 0, v: 0.1, f: "=A2*2" };
    const s = new EditState(w, "c");
    expect(s.edit([{ row: 1, col: 1, value: 0.1 }])).toBeNull();
    expect(s.edit([{ row: 1, col: 1, value: 0.2 }])?.cells).toEqual([{ row: 1, col: 1, value: 0.2, base: 0 }]);
  });

  it("답을 기다리는 칸은 다른 사람의 수정이 와도 내 값을 보인다", () => {
    const s = new EditState(welcome(), "c");
    const b = s.edit([{ row: 1, col: 1, value: 1 }])!;
    s.cells([
      { row: 1, col: 1, value: 2, ver: 1 },
      { row: 2, col: 1, value: 3, ver: 1 },
    ]);
    expect(s.value(1, 1)).toBe(1);
    expect(s.value(2, 1)).toBe(3);
    s.applied(b.batch, [{ row: 1, col: 1, value: 1, ver: 2 }], [{ row: 1, col: 1 }], []);
    expect(s.value(1, 1)).toBe(1);
  });

  it("거절되면 서버 값으로 되돌리고 되돌리기 목록에서도 뺀다", () => {
    const s = new EditState(welcome(), "c");
    const b = s.edit([{ row: 1, col: 0, value: "x" }])!;
    s.applied(b.batch, [], [], [{ row: 1, col: 0, reason: "입력할 수 없는 칸입니다", value: null, ver: 0 }]);
    expect(s.value(1, 0)).toBeNull();
    expect(s.undo()).toBeNull();
  });

  it("되돌리기·다시 실행은 묶음 단위이고, 그 뒤 남이 바꾼 칸은 건너뛴다", () => {
    const s = new EditState(welcome(), "c");
    const b1 = s.edit([
      { row: 1, col: 1, value: 1 },
      { row: 2, col: 1, value: 2 },
    ])!;
    s.applied(
      b1.batch,
      [
        { row: 1, col: 1, value: 1, ver: 1 },
        { row: 2, col: 1, value: 2, ver: 1 },
      ],
      [],
      [],
    );
    s.cells([{ row: 2, col: 1, value: 99, ver: 2 }]); // 다른 사람이 바꿈
    const u = s.undo()!;
    expect(u.cells).toEqual([{ row: 1, col: 1, value: null, base: 1 }]);
    expect(s.value(2, 1)).toBe(99);
    const r = s.redo()!;
    expect(r.cells.map((c) => c.value)).toEqual([1]);
  });

  it("다시 연결하면 보내지 못한 입력을 새 시트 위에 다시 그린다", () => {
    const s = new EditState(welcome(), "c");
    const b = s.edit([{ row: 1, col: 1, value: 5 }])!;
    // 끊긴 사이 다른 사람이 같은 칸을 바꿨다: 새 시트에는 그 값과 버전이 있다.
    const w = welcome({ versions: [[3, 4]] });
    w.model.cells[3]!.v = 7;
    s.load(w);
    expect(s.value(1, 1)).toBe(5);
    expect(s.outbox).toEqual([b]);
    expect(b.cells[0]!.base).toBe(0); // 처음 본 버전으로 보내 충돌을 알 수 있게 한다
    // 새로 고침해 저장해 둔 대기열로 다시 시작해도 같다.
    const again = new EditState(welcome(), "c", 0, s.outbox);
    expect(again.value(1, 1)).toBe(5);
    expect(again.pending.get(3)).toBe(b.batch);
  });

  it("묶음은 시트 번호와 함께 보내고, 되돌리기는 시트마다", () => {
    const a = new EditState(welcome(), "c", 0);
    const b = new EditState(welcome(), "c", 1);
    const sent = b.edit([{ row: 1, col: 0, value: 1 }])!;
    expect(sent.sheet).toBe(1);
    expect(a.canUndo()).toBe(false);
    expect(b.undo()?.sheet).toBe(1);
  });

  it("잠금과 서식", () => {
    const s = new EditState(welcome({ locks: [{ at: { row: 1, col: 1 }, by: "이서연" }] }), "c");
    expect(s.locks.get(3)).toBe("이서연");
    s.lockChanged({ row: 1, col: 1 }, undefined);
    expect(s.locks.size).toBe(0);
    const before = s.model;
    const style: CellStyle = {
      fontName: "맑은 고딕",
      fontSize: 11,
      bold: true,
      italic: false,
      underline: false,
      color: 0,
      numberFormat: "General",
      hAlign: "general",
      vAlign: "bottom",
      wrap: false,
      borders: {},
    };
    s.styles([style], [{ row: 1, col: 1, s: 0 }]);
    expect(s.model).not.toBe(before);
  });

  it("크기 조절: 숨긴 행(남의 행 숨김)은 그대로", () => {
    const view = welcome();
    view.model.hiddenRows = [2];
    view.model.rowHeights = [15, 15, 0];
    const s = new EditState(view, "c");
    const rev = s.rev;
    s.resized("row", 1, 2, 30);
    expect(s.model.rowHeights).toEqual([15, 30, 0]);
    s.resized("col", 0, 1, 90);
    expect(s.model.colWidths).toEqual([90, 60]);
    expect(s.rev).toBeGreaterThan(rev);
  });

  it("빈 값 비교", () => {
    expect(sameValue(undefined, null)).toBe(true);
    expect(sameValue("", null)).toBe(true);
    expect(sameValue({ err: 7 }, { err: 7 })).toBe(true);
    expect(sameValue(0, null)).toBe(false);
  });
});
