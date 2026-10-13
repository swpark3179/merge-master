import type { GridSelection } from "@mm/grid";
import { selectionRange, buildLayout, withLineSize } from "@mm/grid";
import type { CellRange, ConfigEdit, ConfigView, FormatEdit, HostEvent, SheetModel, StructureEdit } from "@mm/shared";
import { decodeFlags } from "@mm/shared";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import type { CellWrite } from "../../api";
import { errorText, jobEvent } from "../../api";
import { useApi, useJobId } from "../../job";

/** 칸 값을 바꾼 새 시트(원래 시트는 그대로 둔다). */
function withValues(m: SheetModel, cells: { row: number; col: number; value?: CellWrite["value"] }[]): SheetModel {
  const out = m.cells.slice();
  for (const u of cells) {
    const i = u.row * m.cols + u.col;
    const cell = out[i];
    if (cell) out[i] = { ...cell, v: u.value ?? null };
  }
  return { ...m, cells: out };
}

/** 1b 상태(시트 하나): 설정 보기(Rust가 계산), 시트(서식 편집으로 바뀜), 선택, 참여자 시점. `sheet`는 세션 안 시트 순번. */
export function useConfig(initial: SheetModel, sheet: number, active: boolean) {
  const api = useApi();
  const job = useJobId();
  const [model, setModel] = useState(initial);
  const [view, setView] = useState<ConfigView | null>(null);
  const [viewer, setViewerState] = useState<string | null>(null);
  const [selection, setSelection] = useState<GridSelection | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // 처음 열 때, 다시 보일 때(1c에서 명단·안내를 바꿨을 수 있다), 승인으로 설정이 바뀌었을 때 새로 읽는다.
  const refresh = useCallback(() => {
    api
      .configView(sheet, viewer)
      .catch(() => api.configView(sheet, null))
      .then((v) => {
        setView(v);
        if ((v.viewer ?? null) !== viewer) setViewerState(null);
      })
      .catch((e) => setError(errorText(e)));
  }, [sheet, viewer]);

  useEffect(() => {
    if (active) refresh();
  }, [active, refresh]);

  // 시트 탭을 오가면 이 화면을 새로 만든다: 확정한 뒤 바뀐 값·서식(참여자 입력·행 추가)이 있으니 세션에서 다시 읽는다.
  useEffect(() => {
    void api
      .monitorGet(sheet)
      .then((d) => setModel(d.model))
      .catch(() => {});
  }, [sheet]);

  useEffect(() => {
    const off = listen(jobEvent("config-changed", job), refresh);
    return () => void off.then((f) => f());
  }, [refresh]);

  // 공유 중 참여자 입력·되돌리기·크기 조절도 이 화면 시트에 반영한다(옛 값 위에 고쳐 쓰지 않게). `reset`은 대개 이 화면의
  // 동작이 결과(시트·설정 보기)를 직접 돌려받으므로, 참여자가 표 구조를 바꾼 때(행 추가·삭제, 취소 행)만 다시 읽는다.
  const shape = useRef(shapeOf(initial));
  shape.current = shapeOf(model);
  useEffect(() => {
    const off = listen<HostEvent>(jobEvent("sheet-changes", job), (e) => {
      const ev = e.payload;
      if (ev.type === "deadlineClose") return;
      if (ev.type !== "reset" && ev.sheet !== sheet) return; // 다른 시트의 변경
      if (ev.type === "reset") {
        void api
          .monitorGet(sheet)
          .then((d) => {
            if (shapeOf(d.model) === shape.current) return;
            setModel(d.model);
            refresh();
          })
          .catch(() => {});
      } else if (ev.type === "cells") setModel((m) => withValues(m, ev.cells));
      else if (ev.type === "resized") setModel((m) => withLineSize(m, ev.axis, ev.from, ev.count, ev.size));
      else if (ev.type === "styles")
        setModel((m) => {
          const cells = m.cells.slice();
          for (const ch of ev.changes) {
            const i = ch.row * m.cols + ch.col;
            const cell = cells[i];
            if (cell) cells[i] = { ...cell, s: ch.s };
          }
          return { ...m, styles: ev.styles, cells };
        });
    });
    return () => void off.then((f) => f());
  }, [refresh, sheet]);

  const run = useCallback(async <T>(f: () => Promise<T>): Promise<T | null> => {
    setBusy(true);
    setError(null);
    try {
      return await f();
    } catch (e) {
      setError(errorText(e));
      return null;
    } finally {
      setBusy(false);
    }
  }, []);

  const apply = useCallback(
    async (edit: ConfigEdit) => {
      const v = await run(() => api.configApply(sheet, edit, viewer));
      if (v) {
        setView(v);
        if (viewer && v.viewer !== viewer) setViewerState(null); // 미리보던 사람이 명단에서 빠짐
      }
    },
    [run, sheet, viewer],
  );

  const format = useCallback(
    async (edit: FormatEdit) => {
      const r = await run(() => api.sheetFormat(sheet, edit, viewer));
      if (r) {
        setModel(r.model);
        setView(r.view);
      }
    },
    [run, sheet, viewer],
  );

  /** 표 구조 바꾸기(오른쪽 클릭 메뉴). 행·열을 지웠으면 선택을 푼다(좌표가 당겨졌다). 됐으면 `true`. */
  const structure = useCallback(
    async (edit: StructureEdit) => {
      const r = await run(() => api.sheetStructure(sheet, edit, viewer));
      if (!r) return false;
      setModel(r.model);
      setView(r.view);
      if (edit.op === "deleteRows" || edit.op === "deleteCols") setSelection(null);
      return true;
    },
    [run, sheet, viewer],
  );

  /** 호스트가 칸 내용을 바꾼다: 바로 그리고, 세션에 보낸 뒤 설정 보기(범례·명단·열 이름)를 새로 읽는다. */
  const setCells = useCallback(
    async (cells: CellWrite[]) => {
      setModel((m) => withValues(m, cells));
      setError(null);
      try {
        await api.sheetSetCells(sheet, cells);
        refresh();
      } catch (e) {
        setError(errorText(e));
      }
    },
    [refresh, sheet],
  );

  const setViewer = useCallback(
    async (name: string | null) => {
      const v = await run(() => api.configView(sheet, name));
      if (v) {
        setView(v);
        setViewerState(name);
      }
    },
    [run, sheet],
  );

  const layout = useMemo(() => buildLayout(model), [model]);
  const flags = useMemo(() => (view ? decodeFlags(view.flags) : new Uint8Array()), [view]);
  const range: CellRange | null = selection ? selectionRange(layout, selection) : null;

  return {
    sheet,
    model,
    view,
    flags,
    viewer,
    setViewer,
    selection,
    setSelection,
    range,
    layout,
    apply,
    format,
    structure,
    setCells,
    setError,
    busy,
    error,
  };
}

export type ConfigState = ReturnType<typeof useConfig>;

/** 표 구조(행·열 수·구조 판·취소 행·추가한 행): 바뀌었으면 시트를 다시 읽는다. */
function shapeOf(m: SheetModel): string {
  return `${m.rows}x${m.cols}:${m.layout ?? 0}:${(m.cancelledRows ?? []).join(",")}:${m.addedRows?.length ?? 0}`;
}
