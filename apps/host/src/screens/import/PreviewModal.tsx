import type { GridSelection } from "@mm/grid";
import { SheetGrid } from "@mm/grid";
import type { SheetModel } from "@mm/shared";
import { colSpanLabel } from "@mm/shared";
import type { ReactNode } from "react";
import { useEffect } from "react";

interface Props {
  model: SheetModel;
  title: string;
  /** 범위·항목이 바뀌어 다시 읽는 중(옛 결과를 흐리게 보인다). */
  stale: boolean;
  selection: GridSelection | null;
  onSelectionChange: (sel: GridSelection | null) => void;
  /** 머리글 행 수·틀 고정 열(다시 읽지 않고 바로 반영된다). */
  headerRows: number;
  frozenCols: number;
  frozenMax: number;
  onHeaderRows: (n: number) => void;
  onFrozenCols: (n: number) => void;
  /** 머리 줄 아래에 둘 것(고른 시트 탭). */
  tabs?: ReactNode;
  onClose: () => void;
}

/** 1a 미리보기를 창 가득 크게 본다. 머리글 행 수·틀 고정은 여기서도 바로 고친다. */
export function PreviewModal(p: Props) {
  const { onClose } = p;
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const m = p.model;
  return (
    <div className="modal-back" onClick={onClose}>
      <div
        className="modal card preview-modal"
        role="dialog"
        aria-label="미리보기 크게 보기"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="card-head">
          <b>미리보기 · {p.title}</b>
          <span className="muted small">
            {(m.rows - m.headerRows).toLocaleString()}행 · {m.cols}열
          </span>
          {p.stale && <span className="muted small">설정이 바뀌어 다시 읽는 중입니다…</span>}
          <span className="spacer" />
          <label className="field inline">
            머리글
            <input
              type="number"
              min={0}
              max={m.rows}
              aria-label="머리글 행 수"
              value={p.headerRows}
              onChange={(e) => p.onHeaderRows(Math.max(0, Math.min(m.rows, Number(e.target.value))))}
            />
            행
          </label>
          <label className="field inline">
            틀 고정
            <select
              aria-label="틀 고정 열"
              value={Math.min(p.frozenCols, p.frozenMax)}
              onChange={(e) => p.onFrozenCols(Number(e.target.value))}
            >
              {Array.from({ length: p.frozenMax + 1 }, (_, n) => (
                <option key={n} value={n}>
                  {colSpanLabel(m.origin.col, n)}
                </option>
              ))}
            </select>
          </label>
          <button className="ghost small" onClick={onClose}>
            닫기
          </button>
        </div>
        <div className="preview-main">
          <SheetGrid
            key={m.sheet}
            model={m}
            selection={p.selection}
            onSelectionChange={p.onSelectionChange}
            className={`preview-grid${p.stale ? " stale" : ""}`}
          />
          {p.tabs}
        </div>
      </div>
    </div>
  );
}
