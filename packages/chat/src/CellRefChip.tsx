import type { CellRange } from "@mm/shared";
import { splitCellRefs } from "@mm/shared";
import { useId } from "react";

import { Popover } from "./Popover";
import type { RefPreview } from "./preview";
import { useHoverIntent } from "./useHoverIntent";

/** 채팅의 셀 참조로 간다: 시트 주소 범위와(적었으면) 시트 이름. */
export type OnRef = (range: CellRange, sheet?: string) => void;
/** 셀 참조 미리보기(시트 주소 범위, 시트 이름). 그릴 것이 없으면 `null`. */
export type PreviewOf = (range: CellRange, sheet?: string) => RefPreview | null;

/** 칸 모양 아이콘(셀블럭 표시). */
export function GridIcon({ size = 11 }: { size?: number }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 12 12"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.3"
      aria-hidden="true"
    >
      <rect x="1" y="1" width="10" height="10" rx="1.5" />
      <path d="M1 4.5h10M4.5 1v10" />
    </svg>
  );
}

/** 채팅 글: `#F7`·`#인력계획!F7:H7` 같은 셀 참조는 셀블럭 칩으로(누르면 그 범위를 고른다). HTML은 해석하지 않는다. */
export function ChatText({ text, onRef, preview }: { text: string; onRef: OnRef; preview?: PreviewOf }) {
  return (
    <>
      {splitCellRefs(text).map((p, i) =>
        "ref" in p ? (
          <CellRefChip key={i} refText={p.ref} range={p.range} sheet={p.sheet} onRef={onRef} preview={preview} />
        ) : (
          <span key={i}>{p.text}</span>
        ),
      )}
    </>
  );
}

/**
 * 셀블럭 칩: `#` 없이 `시트!주소`(시트 이름은 흐리게). 머물거나 초점을 주면 미리보기 카드, 누르면 그 범위로 간다.
 * 접근 이름은 보이는 글자 그대로(`F9`, `인력계획!F7`).
 */
export function CellRefChip({
  refText,
  range,
  sheet,
  onRef,
  preview,
}: {
  /** 보이는 참조(`인력계획!F7`, `'1분기 예산'!F7:H7`, `F9`). */
  refText: string;
  range: CellRange;
  sheet?: string;
  onRef: OnRef;
  preview?: PreviewOf;
}) {
  const hover = useHoverIntent();
  const id = useId();
  const card = hover.anchor && preview ? preview(range, sheet) : null;
  const bang = sheet !== undefined ? refText.lastIndexOf("!") + 1 : 0;
  return (
    <>
      <button
        type="button"
        className="chat-ref"
        // 칩 안이 flex라 화면 낭독기가 시트 이름과 주소 사이를 띄운다: 보이는 참조 그대로 읽게 한다.
        aria-label={refText}
        aria-describedby={card ? id : undefined}
        {...hover.handlers}
        onClick={() => {
          if (hover.consumeLongPress()) return;
          hover.close();
          onRef(range, sheet);
        }}
      >
        <GridIcon />
        {bang > 0 && <span className="sheet">{refText.slice(0, bang)}</span>}
        {refText.slice(bang)}
      </button>
      {card && hover.anchor && (
        <Popover anchor={hover.anchor} id={id} className="chat-preview">
          <RefPreviewCard p={card} />
        </Popover>
      )}
    </>
  );
}

/** 미리보기 카드: 시트·주소·크기, 한 칸이면 열 제목과 값을 크게, 아니면 처음 몇 칸의 표. */
export function RefPreviewCard({ p }: { p: RefPreview }) {
  const single = p.rows.length === 1 && p.rows[0]!.cells.length === 1 && !p.more ? p.rows[0]!.cells[0]! : null;
  return (
    <>
      <div className="chat-preview-head">
        <GridIcon size={13} />
        <b>{p.sheet}</b>
        <span className="addr">{p.addr}</span>
        <span className="size">{p.size}</span>
      </div>
      {single ? (
        <div className="chat-preview-one">
          <span className="label">
            {p.heads[0]!.letter} {p.heads[0]!.label}
          </span>
          <b>{single.hidden ? "볼 수 없음" : single.text || "비어 있음"}</b>
        </div>
      ) : (
        p.rows.length > 0 && (
          <div className="chat-preview-table">
            <div className="tr head">
              <span className="n" />
              {p.heads.map((h, i) => (
                <span key={i} className="td">
                  <span className="letter">{h.letter}</span> {h.label}
                </span>
              ))}
            </div>
            {p.rows.map((r) =>
              r.cells[0]?.hidden ? (
                <div key={r.n} className="tr hidden">
                  <span className="n">{r.n}</span>
                  <span className="td">볼 수 없음</span>
                </div>
              ) : (
                <div key={r.n} className="tr">
                  <span className="n">{r.n}</span>
                  {r.cells.map((c, i) => (
                    <span key={i} className={`td${c.numeric ? " num" : ""}${c.text ? "" : " empty"}`}>
                      {c.text || "비어 있음"}
                    </span>
                  ))}
                </div>
              ),
            )}
          </div>
        )
      )}
      {p.more && <div className="chat-preview-note">{p.more}</div>}
      <div className="chat-preview-note">누르면 이 범위를 선택합니다</div>
    </>
  );
}
