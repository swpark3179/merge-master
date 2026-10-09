import { JoinScreen } from "./JoinScreen";
import { useSession } from "./useSession";
import { Workspace } from "./Workspace";

/** 참여자 웹: 입장 전에는 1e 접속 화면, 입장하면 1f 작업 화면. */
export function App() {
  const s = useSession();
  if (s.phase.kind === "in" && s.welcome) return <Workspace s={s} />;
  return <JoinScreen s={s} />;
}
