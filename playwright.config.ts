import { defineConfig } from "@playwright/test";

const PORT = 8099;
/** '남의 행 숨김' 설정으로 띄운 두 번째 서버. */
export const HIDDEN_PORT = 8098;
/** 시트 두 개(예산신청서 + 인력계획)를 함께 취합하는 세 번째 서버. */
export const SHEETS_PORT = 8097;

const server = (port: number, extra: string) => ({
  command: `cargo run -q --release -p collect-server --bin collect-dev-server -- --fixture fixtures/budget.sheet.json --port ${port} --bind 127.0.0.1 --required F --dev-admin ${extra}`,
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
    server(SHEETS_PORT, "--fixture fixtures/staff.sheet.json --required 2:E"),
  ],
});
