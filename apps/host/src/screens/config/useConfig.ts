import type { GridSelection } from "@mm/grid";
import { selectionRange, buildLayout } from "@mm/grid";
import type { CellRange, ConfigEdit, ConfigView, FormatEdit, SheetModel } from "@mm/shared";
import { decodeFlags } from "@mm/shared";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useMemo, useState } from "react";

import { api, errorText } from "../../api";

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

  return { model, view, flags, viewer, setViewer, selection, setSelection, range, layout, apply, format, busy, error };
}

export type ConfigState = ReturnType<typeof useConfig>;
