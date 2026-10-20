import type { Browser, BrowserContext, Page } from "@playwright/test";
import { expect, test } from "@playwright/test";

// 에이전트 PC의 자동 승인: 개발 서버(포트 8095, --auto-approve)에 명단 밖(127.0.0.1)에서 접속하면 입력한 이름 그대로 바로
// 들어온다. 같은 이름이 접속 중이면 그 연결이 끊길 때까지, 호스트가 끊은 이름은 승인할 때까지 기다린다.

const ORIGIN = "http://127.0.0.1:8095";
const unique = (prefix: string) => `${prefix}${Date.now() % 100000}`;

const contexts: BrowserContext[] = [];
test.afterEach(async () => {
  for (const c of contexts.splice(0)) await c.close();
});

async function page(browser: Browser): Promise<Page> {
  const context = await browser.newContext();
  contexts.push(context);
  return context.newPage();
}

async function joinAs(p: Page, name: string) {
  await p.goto(`${ORIGIN}/q4`);
  await p.getByLabel("이름").fill(name);
  await p.getByRole("button", { name: "참여하기" }).click();
}

async function inWorkspace(p: Page) {
  await expect(p.getByRole("heading", { name: "2026년 부서별 예산 신청" })).toBeVisible();
  await expect(p.locator(".status")).toContainText("실시간 동기화");
}

test("명단 밖에서도 이름을 입력하면 승인 없이 바로 들어온다", async ({ browser }) => {
  const a = await page(browser);
  await a.goto(`${ORIGIN}/q4`);
  await expect(a.getByText("이름을 입력하면 바로 참여합니다")).toBeVisible();
  const name = unique("자동");
  await joinAs(a, name);
  await inWorkspace(a);
  await expect(a.locator(".status")).toContainText(`${name} 님`);
});

test("같은 이름이 접속 중이면 그 창이 닫힐 때까지 기다렸다가 들어온다", async ({ browser }) => {
  const name = unique("같은");
  const a = await page(browser);
  await joinAs(a, name);
  await inWorkspace(a);
  const b = await page(browser);
  await joinAs(b, name);
  await expect(b.getByText("같은 이름이 접속 중")).toBeVisible();
  await expect(b.getByText("그 기기의 연결이 끊기면 바로 들어갑니다")).toBeVisible();
  await a.context().close();
  await inWorkspace(b);
});

test("호스트가 연결을 끊은 이름은 다시 승인을 받아야 들어온다", async ({ browser, request }) => {
  const name = unique("끊김");
  const a = await page(browser);
  await joinAs(a, name);
  await inWorkspace(a);
  await request.post(`${ORIGIN}/__dev/kick?s=q4`, { data: { name } });
  await expect(a.getByText("호스트가 연결을 끊었습니다.")).toBeVisible();
  await a.getByRole("button", { name: "다시 참여" }).click();
  await expect(a.getByText("호스트 승인을 기다리는 중")).toBeVisible();
  expect((await request.post(`${ORIGIN}/__dev/approve?s=q4`, { data: { name } })).ok()).toBe(true);
  await inWorkspace(a);
});
