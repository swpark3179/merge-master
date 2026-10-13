import { withLineSize } from "@mm/grid";
import type {
  Axis,
  BatchOrigin,
  CellEdit,
  CellRef,
  CellStyle,
  CellUpdate,
  CellValue,
  Rejection,
  SheetModel,
  SheetView,
  StyleChange,
} from "@mm/shared";

/**
 * 참여자 화면의 편집 상태(React와 분리한 순수 로직). 시트 하나에 하나씩(되돌리기도 시트마다).
 *
 * - 낙관적 반영: 입력하면 바로 그리고, 서버 답(`applied`)이 오면 서버가 정규화한 값으로 바꾼다.
 * - 대기 중 칸(`pending`): 답을 못 받은 칸은 다른 사람의 수정이 와도 내 값을 계속 보인다(나중 입력이 이긴다).
 * - 보낼 묶음(`outbox`): 끊긴 동안 쌓이고, 다시 연결되면 처음 본 버전(`base`)과 함께 다시 보낸다.
 * - 되돌리기: 내가 바꾼 묶음 단위. 그 뒤 다른 사람이 바꾼 칸은 건드리지 않는다.
 */

export interface Batch {
  batch: string;
  /** 세션 안 시트 순번(불러온 순서). */
  sheet: number;
  cells: CellEdit[];
  /** 히스토리·활동 피드 표시용(붙여넣기·범위 지우기·실행 취소·내 변경 되돌리기). */
  origin?: BatchOrigin;
  /**
   * 묶음을 만들 때 본 시트의 구조 판(`model.layout`). 그 뒤 앱에서 붙인 행·열을 지워 좌표가 밀렸으면 서버가 받지 않는다.
   * 옛 대기열에는 없다(검사하지 않는다).
   */
  layout?: number;
}

export interface CellChange {
  row: number;
  col: number;
  value: CellValue;
}

interface UndoCell {
  row: number;
  col: number;
  before: CellValue;
  after: CellValue;
}

interface UndoEntry {
  batch: string;
  cells: UndoCell[];
}

export interface AppliedResult {
  conflicts: CellRef[];
  rejected: Rejection[];
}

/** 빈 값은 모두 `null`로 본다. */
export function sameValue(a: CellValue | undefined, b: CellValue | undefined): boolean {
  const x = a === undefined || a === "" ? null : a;
  const y = b === undefined || b === "" ? null : b;
  if (x !== null && y !== null && typeof x === "object" && typeof y === "object") return x.err === y.err;
  return x === y;
}

export class EditState {
  model!: SheetModel;
  versions = new Map<number, number>();
  /** 다른 사람이 편집 중인 칸 → 이름. */
  locks = new Map<number, string>();
  /** 칸 → 아직 답을 못 받은 마지막 묶음. */
  pending = new Map<number, string>();
  outbox: Batch[] = [];
  private undoStack: UndoEntry[] = [];
  private redoStack: UndoEntry[] = [];
  /** 서버가 반영한 내 변경 칸 수(상태 바 '내 변경 N건'). */
  myChanges = 0;
  lastSaved: number | null = null;
  /** 바뀔 때마다 늘어난다(React 다시 그리기용). */
  rev = 0;
  private seq = 0;

  constructor(
    view: SheetView,
    private readonly clientId: string,
    /** 세션 안 시트 순번. 묶음에 함께 보낸다. */
    public sheet = 0,
    outbox: Batch[] = [],
  ) {
    this.outbox = outbox.map((b) => ({ ...b, sheet, cells: b.cells.map((c) => ({ ...c })) }));
    this.load(view);
  }

  get cols(): number {
    return this.model.cols;
  }

  index(row: number, col: number): number {
    return row * this.model.cols + col;
  }

  value(row: number, col: number): CellValue {
    return this.model.cells[this.index(row, col)]?.v ?? null;
  }

  canUndo(): boolean {
    return this.undoStack.length > 0;
  }

  /** 입장·재접속·설정 변경으로 새 시트를 받았다. 아직 답을 못 받은 내 입력은 그 위에 다시 그린다. */
  load(welcome: SheetView) {
    // 호스트가 다른 시트로 다시 불러왔거나 앱에서 붙인 행·열을 지웠으면(구조 판이 바뀜) 좌표가 달라져 되돌리기 기록은
    // 의미가 없다. 표 아래·오른쪽에 행·열이 붙은 것(늘어나기만 함)은 같은 시트라 좌표가 그대로이므로 기록을 남긴다.
    const same =
      this.model &&
      this.model.workbook === welcome.model.workbook &&
      this.model.sheet === welcome.model.sheet &&
      (this.model.layout ?? 0) === (welcome.model.layout ?? 0);
    const grown = same && welcome.model.rows >= this.model.rows && welcome.model.cols >= this.model.cols;
    if (this.model && !grown) {
      this.undoStack = [];
      this.redoStack = [];
    }
    this.model = welcome.model;
    this.versions = new Map(welcome.versions);
    this.locks = new Map(welcome.locks.map((l) => [this.index(l.at.row, l.at.col), l.by]));
    this.pending.clear();
    const layout = this.model.layout ?? 0;
    for (const b of this.outbox) {
      // 구조가 바뀌기 전에 만든 묶음은 서버가 받지 않는다(밀린 칸에 그리지 않는다).
      if (b.layout !== undefined && b.layout !== layout) continue;
      for (const c of b.cells) {
        const cell = this.model.cells[this.index(c.row, c.col)];
        if (!cell) continue;
        cell.v = c.value ?? null;
        this.pending.set(this.index(c.row, c.col), b.batch);
      }
    }
    this.bump();
  }

  private bump() {
    this.rev++;
  }

  private newBatch(): string {
    return `${this.clientId}-${Date.now().toString(36)}-${++this.seq}`;
  }

  /** 칸을 바꾼다(값은 이미 열 형식대로 정규화한 것). 보낼 묶음을 돌려준다. 바뀐 칸이 없으면 `null`. */
  edit(changes: CellChange[], record: "undo" | "redo" | "none" = "undo", origin?: BatchOrigin): Batch | null {
    const batch = this.newBatch();
    const cells: CellEdit[] = [];
    const undo: UndoCell[] = [];
    for (const ch of changes) {
      const i = this.index(ch.row, ch.col);
      const cell = this.model.cells[i];
      if (!cell) continue;
      const before = cell.v ?? null;
      const after = ch.value ?? null;
      if (sameValue(before, after)) continue;
      cells.push({ row: ch.row, col: ch.col, value: after, base: this.versions.get(i) ?? 0 });
      undo.push({ row: ch.row, col: ch.col, before, after });
      cell.v = after;
      this.pending.set(i, batch);
    }
    if (cells.length === 0) return null;
    const layout = this.model.layout ?? 0;
    const b: Batch = origin
      ? { batch, sheet: this.sheet, cells, origin, layout }
      : { batch, sheet: this.sheet, cells, layout };
    this.outbox.push(b);
    if (record === "undo") {
      this.undoStack.push({ batch, cells: undo });
      if (this.undoStack.length > 200) this.undoStack.shift();
    } else if (record === "redo") {
      this.redoStack.push({ batch, cells: undo });
    }
    this.bump();
    return b;
  }

  /** 마지막 내 변경을 되돌리는 묶음. 그 뒤 다른 사람이 바꾼 칸은 건너뛴다. */
  undo(): Batch | null {
    return this.replayInverse(this.undoStack, "redo", "undo");
  }

  redo(): Batch | null {
    return this.replayInverse(this.redoStack, "undo", "redo");
  }

  private replayInverse(stack: UndoEntry[], record: "undo" | "redo", origin: BatchOrigin): Batch | null {
    for (;;) {
      const entry = stack.pop();
      if (!entry) return null;
      const back = entry.cells
        .filter((c) => sameValue(this.value(c.row, c.col), c.after))
        .map((c) => ({ row: c.row, col: c.col, value: c.before }));
      const b = this.edit(back, record, origin);
      if (b) return b;
    }
  }

  /** 내 묶음에 대한 서버 답. */
  applied(batch: string, cells: CellUpdate[], conflicts: CellRef[], rejected: Rejection[]): AppliedResult {
    this.outbox = this.outbox.filter((b) => b.batch !== batch);
    for (const u of cells) {
      const i = this.index(u.row, u.col);
      this.versions.set(i, Math.max(u.ver, this.versions.get(i) ?? 0));
      if (this.pending.get(i) === batch) {
        this.pending.delete(i);
        const cell = this.model.cells[i];
        if (cell) cell.v = u.value ?? null;
      }
    }
    for (const r of rejected) {
      const i = this.index(r.row, r.col);
      this.versions.set(i, Math.max(r.ver, this.versions.get(i) ?? 0));
      if (this.pending.get(i) === batch) {
        this.pending.delete(i);
        const cell = this.model.cells[i];
        if (cell) cell.v = r.value ?? null;
      }
    }
    if (rejected.length > 0) {
      const gone = new Set(rejected.map((r) => this.index(r.row, r.col)));
      for (const e of [...this.undoStack, ...this.redoStack]) {
        if (e.batch === batch) e.cells = e.cells.filter((c) => !gone.has(this.index(c.row, c.col)));
      }
    }
    this.myChanges += cells.length;
    this.lastSaved = Date.now();
    this.bump();
    return { conflicts, rejected };
  }

  /** 다른 사람(또는 수식 재계산)이 바꾼 칸. 답을 기다리는 내 칸은 내 값을 그대로 보인다. */
  cells(updates: CellUpdate[]) {
    for (const u of updates) {
      const i = this.index(u.row, u.col);
      this.versions.set(i, Math.max(u.ver, this.versions.get(i) ?? 0));
      if (this.pending.has(i)) continue;
      const cell = this.model.cells[i];
      if (cell) cell.v = u.value ?? null;
    }
    this.bump();
  }

  lockChanged(at: CellRef, by: string | undefined) {
    const i = this.index(at.row, at.col);
    if (by) this.locks.set(i, by);
    else this.locks.delete(i);
    this.bump();
  }

  /** 누군가(호스트 포함) 열 너비·행 높이를 바꿨다. 숨긴 줄(남의 행 숨김 포함)은 그대로 둔다. */
  resized(axis: Axis, from: number, count: number, size: number) {
    this.model = withLineSize(this.model, axis, from, count, size);
    this.bump();
  }

  styles(styles: CellStyle[], changes: StyleChange[]) {
    // 새 개체로 바꿔 그리드가 서식 표를 다시 읽게 한다(값 변경은 같은 개체를 고친다).
    this.model = { ...this.model, styles };
    for (const ch of changes) {
      const cell = this.model.cells[this.index(ch.row, ch.col)];
      if (cell) cell.s = ch.s;
    }
    this.bump();
  }
}
