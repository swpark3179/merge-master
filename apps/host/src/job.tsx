import { listen } from "@tauri-apps/api/event";
import type { ReactNode } from "react";
import { createContext, useContext, useEffect, useMemo, useRef } from "react";

import type { Api } from "./api";
import { jobEvent, makeApi } from "./api";

interface JobContextValue {
  job: string;
  api: Api;
}

const JobContext = createContext<JobContextValue | null>(null);

/** 그 업무의 화면들: 안에서 [`useApi`]·[`useJobListen`]이 이 업무를 가리킨다. */
export function JobProvider({ job, children }: { job: string; children: ReactNode }) {
  const value = useMemo(() => ({ job, api: makeApi(job) }), [job]);
  return <JobContext.Provider value={value}>{children}</JobContext.Provider>;
}

function useJobContext(): JobContextValue {
  const v = useContext(JobContext);
  if (!v) throw new Error("업무 화면(JobProvider) 밖에서 불렀습니다");
  return v;
}

/** 지금 업무의 API(앱 공통 + 업무). 업무를 바꾸면 화면이 새로 그려지므로 늦게 끝난 호출도 원래 업무로 간다. */
export function useApi(): Api {
  return useJobContext().api;
}

export function useJobId(): string {
  return useJobContext().job;
}

/** 지금 업무의 이벤트(`participants`, `sheet-changes`, `comms`, `recalc-status`, `config-changed`)를 받는다. */
export function useJobListen<T>(name: string, handler: (payload: T) => void, enabled = true) {
  const job = useJobId();
  const ref = useRef(handler);
  ref.current = handler;
  useEffect(() => {
    if (!enabled) return;
    const off = listen<T>(jobEvent(name, job), (e) => ref.current(e.payload));
    return () => void off.then((f) => f());
  }, [name, job, enabled]);
}
