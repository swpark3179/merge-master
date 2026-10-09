import type { Page } from "@playwright/test";
import { expect, test } from "@playwright/test";

// 참여자 웹(1e 접속 · 1f 작업 화면)을 개발 서버(collect-dev-server --dev-admin)에 붙여 확인한다.
// 개발 서버는 테스트 사이에 상태가 남으므로, 명단 밖 이름은 실행마다 새로 만든다.

const unique = (prefix: string) => `${prefix}${Date.now() % 100000}`;

async function joinAs(page: Page, name: string, path = "/q4") {
  await page.goto(path);
  await page.getByLabel("이름").fill(name);
  await page.getByRole("button", { name: "참여하기" }).click();
}

async function inWorkspace(page: Page) {
  await expect(page.getByRole("heading", { name: "2026년 부서별 예산 신청" })).toBeVisible();
  await expect(page.locator(".status")).toContainText("실시간 동기화");
}

test("명단 이름으로 입장하고, 새로 고침하면 이름 없이 바로 들어온다", async ({ page }) => {
  await page.goto("/q4");
  await expect(page.getByText("박지훈 님의 엑셀 취합 요청")).toBeVisible();
  await expect(page.getByText("부서별 담당 비목의 신청 금액")).toBeVisible();
  await joinAs(page, "김민지");
  await inWorkspace(page);
  await expect(page.locator(".mine-progress")).toContainText("내 필수 입력");
  await expect(page.getByTestId("row-count")).toContainText("내 담당");
  // 남의 행은 회색, 내 행의 필수 빈칸은 빨간 표시
  expect(await page.locator(".mmg-cell.deco-others").count()).toBeGreaterThan(20);
  await page.locator(".mmg-rowhead", { hasText: /^13$/ }).first().scrollIntoViewIfNeeded();
  await page.reload();
  await inWorkspace(page);
  await expect(page.getByRole("button", { name: "참여하기" })).toHaveCount(0);
});

test("시트 그리드: 머리글·병합·틀 고정이 원본대로 보인다", async ({ page }) => {
  await joinAs(page, "한지우");
  await inWorkspace(page);
  await expect(page.locator(".mmg-cell", { hasText: "기본 정보" })).toBeVisible();
  const merged = page.locator('.mmg-cell[data-r="0"][data-c="0"]');
  const widths = await page
    .locator(".mmg-top .mmg-colhead, .mmg-corner .mmg-colhead:not(.mmg-origin)")
    .evaluateAll((els) => els.slice(0, 4).map((e) => e.getBoundingClientRect().width));
  const box = await merged.boundingBox();
  expect(Math.abs(box!.width - widths.reduce((a, b) => a + b, 0))).toBeLessThan(2);
  const head = page.locator(".mmg-cell", { hasText: "신청 금액" });
  const before = await head.boundingBox();
  await page.locator(".mmg-scroller").evaluate((el) => (el.scrollTop = 12_000));
  await page.waitForTimeout(100);
  expect((await head.boundingBox())!.y).toBeCloseTo(before!.y, 0);
  await page.screenshot({ path: "test-results/participant-grid.png" });
});

test("가상화 스크롤 성능", async ({ page }) => {
  await joinAs(page, "오세린");
  await inWorkspace(page);
  await page.goto("/q4?bench");
  const bench = page.getByTestId("bench");
  await expect(bench).toBeVisible({ timeout: 15_000 });
  const result = JSON.parse((await bench.textContent()) ?? "{}");
  console.log("scroll bench", result);
  expect(result.maxCells).toBeLessThan(1500);
  expect(result.avgFps).toBeGreaterThanOrEqual(45);
});

test("선택과 키보드 이동이 수식 입력줄에 반영된다", async ({ page }) => {
  await joinAs(page, "정하늘");
  await inWorkspace(page);
  await page.locator('.mmg-cell[data-r="2"][data-c="6"]').click();
  await expect(page.locator(".formula-bar .addr")).toHaveText("G3");
  await page.keyboard.press("ArrowLeft");
  await expect(page.locator(".formula-bar .addr")).toHaveText("F3");
  await expect(page.locator(".formula-bar .cell-tags")).toContainText("필수");
  await page.locator('.mmg-cell[data-r="2"][data-c="2"]').click();
  await page.keyboard.press("ArrowDown");
  await expect(page.locator(".formula-bar .addr")).toHaveText("C8");
});

test("다른 사람의 커서가 이름표로 보인다", async ({ browser }) => {
  const a = await (await browser.newContext()).newPage();
  const b = await (await browser.newContext()).newPage();
  await joinAs(a, "최도윤");
  await joinAs(b, "오세린");
  await inWorkspace(a);
  await inWorkspace(b);
  await expect(b.locator('.avatar[title^="최도윤"]')).toBeVisible();
  await a.locator('.mmg-cell[data-r="9"][data-c="5"]').click();
  await expect(b.locator('.mmg-cell[data-r="9"][data-c="5"] .peer-tag')).toHaveText("최도윤");
});

test("같은 이름이 접속 중이면 확인 후 이 기기로 전환한다", async ({ browser }) => {
  const a = await (await browser.newContext()).newPage();
  const b = await (await browser.newContext()).newPage();
  await joinAs(a, "이서연");
  await inWorkspace(a);
  await joinAs(b, "이서연");
  await expect(b.getByText("같은 이름이 이미 접속 중")).toBeVisible();
  await b.getByRole("button", { name: "이 기기로 전환" }).click();
  await inWorkspace(b);
  await expect(a.getByText("다른 기기(창)에서 같은 이름으로 접속해")).toBeVisible();
  await expect(a.getByRole("button", { name: "이 기기로 다시 전환" })).toBeVisible();
});

test("명단에 없는 이름은 호스트 승인을 기다렸다가 들어온다", async ({ page, request }) => {
  const name = unique("신규");
  await joinAs(page, name);
  await expect(page.getByText(`‘${name}’ 님, 호스트 승인을 기다리는 중입니다…`)).toBeVisible();
  const snap = await (await request.get("/__dev/snapshot")).json();
  expect(snap.pending.map((p: { name: string }) => p.name)).toContain(name);
  expect((await request.post("/__dev/approve", { data: { name, range: "H500:H620" } })).ok()).toBe(true);
  await inWorkspace(page);
  await expect(page.locator(".avatar.me")).toBeVisible();
});

test("호스트가 거절하면 이름 입력으로 돌아간다", async ({ page, request }) => {
  const name = unique("거절");
  await joinAs(page, name);
  await expect(page.getByText("호스트 승인을 기다리는 중")).toBeVisible();
  await request.post("/__dev/reject", { data: { name } });
  await expect(page.getByText("호스트가 참여를 거절했습니다")).toBeVisible();
  await expect(page.getByRole("button", { name: "참여하기" })).toBeVisible();
});

test("호스트가 연결을 끊으면 알리고, 다시 참여할 수 있다", async ({ page, request }) => {
  await joinAs(page, "김민지");
  await inWorkspace(page);
  await request.post("/__dev/kick", { data: { name: "김민지" } });
  await expect(page.getByText("호스트가 연결을 끊었습니다.")).toBeVisible();
  await page.getByRole("button", { name: "다시 참여" }).click();
  await inWorkspace(page);
});

test("‘남의 행 숨김’이면 내 행만 받는다", async ({ page }) => {
  await joinAs(page, "김민지", "http://127.0.0.1:8098/q4");
  await inWorkspace(page);
  const heads = await page.locator(".mmg-rowhead").allTextContents();
  expect(heads).toContain("13"); // 김민지 행(총무팀)
  expect(heads).not.toContain("3"); // 이서연 행
  // 남의 행 값은 아예 오지 않는다: 이서연 담당 행의 담당자 칸(병합)이 그리드에 없다.
  expect(await page.locator(".mmg-cell", { hasText: "이서연" }).count()).toBe(0);
});
