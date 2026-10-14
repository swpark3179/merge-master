import { defineConfig } from "@playwright/test";

const PORT = 8099;
/** '남의 행 숨김' 설정으로 띄운 두 번째 서버. */
export const HIDDEN_PORT = 8098;
/** 시트 두 개(예산신청서 + 인력계획)를 함께 취합하는 세 번째 서버. */
export const SHEETS_PORT = 8097;
/** 업무 두 개(q4 예산 + hr 인력)를 한 포트로 서비스하는 네 번째 서버. */
export const JOBS_PORT = 8096;

// 처음엔 모든 열이 입력이라 호스트가 1b에서 하듯 키 열을 잠근다(--locked): 예산신청서 A~E(키·실적)·G(수식), 인력계획 A~D.
const server = (port: number, extra: string) => ({
  command: `cargo run -q --release -p collect-server --bin collect-dev-server -- --fixture fixtures/budget.sheet.json --port ${port} --bind 127.0.0.1 --required F --locked A,B,C,D,E,G --dev-admin ${extra}`,
  url: `http://127.0.0.1:${port}/api/health`,
  reuseExistingServer: true,
  timeout: 300_000,
});

// 설치된 Chrome을 쓴다(브라우저를 따로 내려받지 않음). Chrome이 없는 곳에서는 `PW_EXECUTABLE`에 Chromium 실행 파일을 준다.
// 참여자 웹은 먼저 빌드돼 있어야 한다: pnpm e2e
const executablePath = process.env.PW_EXECUTABLE;

export default defineConfig({
  testDir: "e2e",
  timeout: 30_000,
  reporter: [["list"]],
  workers: 1,
  use: {
    baseURL: `http://127.0.0.1:${PORT}`,
    ...(executablePath ? { launchOptions: { executablePath } } : { channel: "chrome" }),
    viewport: { width: 1280, height: 800 },
  },
  webServer: [
    server(PORT, ""),
    server(HIDDEN_PORT, "--others-hidden"),
    server(SHEETS_PORT, "--fixture fixtures/staff.sheet.json --required 2:E --locked 2:A,2:B,2:C,2:D"),
    server(
      JOBS_PORT,
      '--session hr --fixture fixtures/staff.sheet.json --locked A,B,C,D --title "2026 인력 계획 취합" --requester 김팀장',
    ),
  ],
});
