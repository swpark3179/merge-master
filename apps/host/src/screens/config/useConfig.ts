import type { GridSelection } from "@mm/grid";
import { selectionRange, buildLayout } from "@mm/grid";
import type { CellRange, ConfigEdit, ConfigView, FormatEdit, HostEvent, SheetModel } from "@mm/shared";
import { decodeFlags } from "@mm/shared";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import type { CellWrite } from "../../api";
import { api, errorText } from "../../api";

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

/** 1b 상태: 설정 보기(Rust가 계산), 시트(서식 편집으로 바뀜), 선택, 참여자 시점. */
export function useConfig(initial: SheetModel, active: boolean) {
  const [model, setModel] = useState(initial);
  const [view, setView] = useState<ConfigView | null>(null);
  const [viewer, setViewerState] = useState<string | null>(null);
  const [selection, setSelection] = useState<GridSelection | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // 처음 열 때, 다시 보일 때(1c에서 명단·안내를 바꿨을 수 있다), 승인으로 설정이 바뀌었을 때 새로 읽는다.
  const refresh = useCallback(() => {
    api
      .configView(viewer)
      .catch(() => api.configView(null))
      .then((v) => {
        setView(v);
        if ((v.viewer ?? null) !== viewer) setViewerState(null);
      })
      .catch((e) => setError(errorText(e)));
  }, [viewer]);

  useEffect(() => {
    if (active) refresh();
  }, [active, refresh]);

  useEffect(() => {
    const off = listen("config-changed", refresh);
    return () => void off.then((f) => f());
  }, [refresh]);

  // 공유 중 참여자 입력·되돌리기도 이 화면 시트에 반영한다(옛 값 위에 고쳐 쓰지 않게). `reset`은 대개 이 화면의
  // 동작이 결과(시트·설정 보기)를 직접 돌려받으므로, 참여자가 행을 추가해 시트 크기가 바뀐 때만 다시 읽는다.
  const rows = useRef(initial.rows);
  rows.current = model.rows;
  useEffect(() => {
    const off = listen<HostEvent>("sheet-changes", (e) => {
      const ev = e.payload;
      if (ev.type === "reset") {
        void api
          .monitorGet()
          .then((d) => {
            if (d.model.rows === rows.current) return;
            setModel(d.model);
            refresh();
          })
          .catch(() => {});
      } else if (ev.type === "cells") setModel((m) => withValues(m, ev.cells));
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
  }, [refresh]);

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
      const v = await run(() => api.configApply(edit, viewer));
      if (v) {
        setView(v);
        if (viewer && v.viewer !== viewer) setViewerState(null); // 미리보던 사람이 명단에서 빠짐
      }
    },
    [run, viewer],
  );

  const format = useCallback(
    async (edit: FormatEdit) => {
      const r = await run(() => api.sheetFormat(edit, viewer));
      if (r) {
        setModel(r.model);
        setView(r.view);
      }
    },
    [run, viewer],
  );

  /** 호스트가 칸 내용을 바꾼다: 바로 그리고, 세션에 보낸 뒤 설정 보기(범례·명단·열 이름)를 새로 읽는다. */
  const setCells = useCallback(
    async (cells: CellWrite[]) => {
      setModel((m) => withValues(m, cells));
      setError(null);
      try {
        await api.sheetSetCells(cells);
        refresh();
      } catch (e) {
        setError(errorText(e));
      }
    },
    [refresh],
  );

  const setViewer = useCallback(
    async (name: string | null) => {
      const v = await run(() => api.configView(name));
      if (v) {
        setView(v);
        setViewerState(name);
      }
    },
    [run],
  );

  const layout = useMemo(() => buildLayout(model), [model]);
  const flags = useMemo(() => (view ? decodeFlags(view.flags) : new Uint8Array()), [view]);
  const range: CellRange | null = selection ? selectionRange(layout, selection) : null;

  return {
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
    setCells,
    setError,
    busy,
    error,
  };
}

export type ConfigState = ReturnType<typeof useConfig>;
