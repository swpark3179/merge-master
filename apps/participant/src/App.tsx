import { JoinScreen } from "./JoinScreen";
import { SessionPicker } from "./SessionPicker";
import { currentSlug } from "./slug";
import { useSession } from "./useSession";
import { Workspace } from "./Workspace";

/**
 * 참여자 웹: 맨 위 주소(`/`)는 호스트가 공개한 업무 목록, 업무 주소(`/q4`)는 입장 전 1e 접속 화면 → 입장하면 1f 작업 화면.
 * 업무를 고르면 그 주소로 새로 연다(업무마다 이름·토큰·대기열을 따로 기억한다).
 */
export function App() {
  const slug = currentSlug();
  if (slug === null) return <SessionPicker />;
  return <Collect slug={slug} />;
}

function Collect({ slug }: { slug: string }) {
  const s = useSession(slug);
  if (s.phase.kind === "in" && s.welcome) return <Workspace s={s} />;
  return <JoinScreen s={s} />;
}
