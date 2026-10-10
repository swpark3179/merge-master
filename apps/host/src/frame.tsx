import { createContext, useContext, useEffect, useRef } from "react";

/** 화면이 아래쪽 '다음' 버튼을 대신 정할 때(예: 1 불러오기 = 확정 후 이동). */
export interface FooterOverride {
  /** 기본은 다음 단계로 이동. */
  onNext?: () => void;
  nextDisabled?: boolean;
  hint?: string;
}

interface Frame {
  setFooter: (o: FooterOverride | null) => void;
  /** 화면 아래 가운데에 잠깐 뜨는 알림. */
  toast: (msg: string) => void;
}

export const FrameContext = createContext<Frame>({ setFooter: () => {}, toast: () => {} });

export function useToast() {
  return useContext(FrameContext).toast;
}

/** 화면이 보이는 동안 단계 아래쪽 버튼·안내를 바꾼다. */
export function useStepFooter(active: boolean, o: FooterOverride) {
  const { setFooter } = useContext(FrameContext);
  const onNext = useRef(o.onNext);
  onNext.current = o.onNext;
  const hasNext = o.onNext !== undefined;
  useEffect(() => {
    if (!active) return;
    setFooter({
      onNext: hasNext ? () => onNext.current?.() : undefined,
      nextDisabled: o.nextDisabled,
      hint: o.hint,
    });
    return () => setFooter(null);
  }, [active, hasNext, o.nextDisabled, o.hint, setFooter]);
}
