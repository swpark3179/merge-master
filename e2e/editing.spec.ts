import type { Browser, BrowserContext, Page } from "@playwright/test";
import { expect, test } from "@playwright/test";

// 4단계 동시 편집: 개발 서버(collect-dev-server --required F --dev-admin)에 여러 브라우저를 붙여 확인한다.
// 서버 상태가 테스트 사이에 남으므로 테스트마다 다른 사람(행)을 쓴다.
// 열: F 신청 금액(숫자·천원·필수) · G 증감률(수식) · H 산출 근거(글자) · I 집행 시기(목록)
// 행(0부터): 최도윤 7–11 · 정하늘 17–21 · 한지우 22–26 · 오세린 27–31 · 김민지 12–16

const F = 5;
const G = 6;
const H = 7;
const I = 8;

const unique = (prefix: string) => `${prefix}${Date.now() % 100000}`;

// 테스트마다 연 브라우저는 닫는다(남아 있으면 다음 테스트에서 같은 이름이 '접속 중'이 된다).
const contexts: BrowserContext[] = [];
test.afterEach(async () => {
  for (const c of contexts.splice(0)) await c.close();
});

async function newPage(browser: Browser): Promise<Page> {
  const context = await browser.newContext();
  contexts.push(context);
  return context.newPage();
}

async function open(browser: Browser, name: string, path = "/q4"): Promise<Page> {
  const page = await newPage(browser);
  await page.goto(path);
  await page.getByLabel("이름").fill(name);
  await page.getByRole("button", { name: "참여하기" }).click();
  await expect(page.locator(".status")).toContainText("실시간 동기화");
  return page;
}

const cell = (page: Page, r: number, c: number) => page.locator(`.mmg-cell[data-r="${r}"][data-c="${c}"]`);
const text = (page: Page, r: number, c: number) => cell(page, r, c).locator(".mmg-text");

async function typeInto(page: Page, r: number, c: number, value: string, key = "Enter") {
  await cell(page, r, c).click();
  await page.keyboard.type(value);
  await page.keyboard.press(key);
}

async function paste(page: Page, tsv: string) {
  await page.evaluate((data) => {
    const dt = new DataTransfer();
    dt.setData("text/plain", data);
    const target = document.activeElement ?? document.body;
    target.dispatchEvent(new ClipboardEvent("paste", { clipboardData: dt, bubbles: true, cancelable: true }));
  }, tsv);
}

test("그리드에서 입력하면 정규화되어 다른 사람에게 바로 보이고 진행률이 오른다", async ({ browser }) => {
  const a = await open(browser, "정하늘");
  const b = await open(browser, "최도윤");
  const before = await a.locator(".mine-progress b").textContent();
  await typeInto(a, 17, F, "9,800");
  await expect(text(a, 17, F)).toHaveText("9,800");
  await expect(a.locator(".formula-bar .addr")).toHaveText("F19"); // Enter는 아래 칸으로
  await expect(text(b, 17, F)).toHaveText("9,800");
  await expect(a.locator(".mine-progress b")).not.toHaveText(before!);
  await expect(a.locator(".status")).toContainText("내 변경 1건");
  // 남의 행은 입력할 수 없다(알림).
  await cell(a, 7, F).click();
  await a.keyboard.type("1");
  await expect(a.locator(".toast")).toContainText("이 칸은 입력할 수 없습니다");
  await expect(text(a, 7, F)).not.toHaveText("1");
});

test("형식이 맞지 않으면 편집기에 오류가 보이고, Esc로 취소한다", async ({ browser }) => {
  const a = await open(browser, "정하늘");
  await typeInto(a, 18, F, "구천팔백");
  await expect(a.locator(".cell-error")).toContainText("숫자만 입력할 수 있습니다(천원 단위)");
  await a.keyboard.press("Escape");
  await expect(a.locator(".cell-error")).toHaveCount(0);
  await expect(text(a, 18, F)).toHaveText("");
  // F2로 고치기: 숫자는 서식 없이 편집기에 들어간다.
  await cell(a, 17, F).click();
  await a.keyboard.press("F2");
  await expect(a.getByLabel("셀 입력")).toHaveValue("9800");
  await a.keyboard.press("Escape");
});

test("편집 중인 칸은 다른 사람에게 잠기고, 확정하거나 호스트가 풀면 풀린다", async ({ browser, request }) => {
  // 정하늘의 H18:H22를 새 참여자에게도 범위 배정해 두 사람이 같은 칸을 편집할 수 있게 한다.
  const helper = unique("공유");
  const b = await newPage(browser);
  await b.goto("/q4");
  await b.getByLabel("이름").fill(helper);
  await b.getByRole("button", { name: "참여하기" }).click();
  await expect(b.getByText("호스트 승인을 기다리는 중")).toBeVisible();
  expect((await request.post("/__dev/approve", { data: { name: helper, range: "H18:H22" } })).ok()).toBe(true);
  await expect(b.locator(".status")).toContainText("실시간 동기화");
  const a = await open(browser, "정하늘");

  await cell(a, 17, H).click();
  await a.keyboard.type("간담회");
  await expect(a.locator(".status")).toContainText("H18 편집 중");
  await expect(cell(b, 17, H).locator(".lock-tag")).toHaveText("🔒 정하늘");
  await cell(b, 17, H).click();
  await expect(b.locator(".hint-bar.lock")).toContainText("정하늘 님이 입력 중입니다");
  await b.keyboard.type("x");
  await expect(b.locator(".toast").last()).toContainText("정하늘 님이 입력 중입니다");
  await b.getByRole("button", { name: "끝나면 알려 주기" }).click();

  await a.keyboard.type(" 4회");
  await a.keyboard.press("Enter");
  await expect(cell(b, 17, H).locator(".lock-tag")).toHaveCount(0);
  await expect(text(b, 17, H)).toHaveText("간담회 4회");
  await expect(b.locator(".toast", { hasText: "H18 칸을 이제 입력할 수 있습니다" })).toBeVisible();

  // 편집기를 열어 둔 채 자리를 비우면 호스트가 잠금을 푼다.
  await cell(a, 18, H).click();
  await a.keyboard.type("임시");
  await expect(cell(b, 18, H).locator(".lock-tag")).toBeVisible();
  const snap = await (await request.get("/__dev/snapshot")).json();
  expect(snap.participants.find((p: { name: string }) => p.name === "정하늘").editing).toEqual({ row: 18, col: H });
  expect((await request.post("/__dev/unlock", { data: { name: "정하늘" } })).ok()).toBe(true);
  await expect(a.locator(".toast", { hasText: "호스트가 편집 잠금을 해제했습니다" })).toBeVisible();
  await expect(cell(b, 18, H).locator(".lock-tag")).toHaveCount(0);
  await expect(text(a, 18, H)).toHaveText("");
});

test("한글 입력(IME 확정)도 첫 글자부터 받는다", async ({ browser }) => {
  const a = await open(browser, "정하늘");
  await cell(a, 20, H).click();
  // IME는 keydown 없이 조합한 글자가 편집기에 들어온다(insertText가 같은 경로).
  await a.keyboard.insertText("한글 산출 근거");
  await expect(a.locator(".cell-editor.on")).toBeVisible();
  await a.keyboard.press("Enter");
  await expect(text(a, 20, H)).toHaveText("한글 산출 근거");
});

test("끊긴 사이 같은 칸을 다른 사람이 바꿨으면 내 값으로 덮어쓰고 알린다", async ({ browser, request }) => {
  const helper = unique("충돌");
  const b = await newPage(browser);
  await b.goto("/q4");
  await b.getByLabel("이름").fill(helper);
  await b.getByRole("button", { name: "참여하기" }).click();
  await expect(b.getByText("호스트 승인을 기다리는 중")).toBeVisible();
  expect((await request.post("/__dev/approve", { data: { name: helper, range: "H28:H32" } })).ok()).toBe(true);
  await expect(b.locator(".status")).toContainText("실시간 동기화");
  const a = await open(browser, "오세린");
  await a.evaluate(() => window.__mmTest!.offline(true));
  await typeInto(a, 30, H, "오프라인 값");
  await typeInto(b, 30, H, "먼저 넣은 값");
  await expect(text(b, 30, H)).toHaveText("먼저 넣은 값");
  await a.evaluate(() => window.__mmTest!.offline(false));
  await expect(
    a.locator(".toast", { hasText: "H31: 다른 사람이 먼저 바꾼 값을 내 입력으로 덮어썼습니다" }),
  ).toBeVisible();
  await expect(text(b, 30, H)).toHaveText("오프라인 값");
});

test("범위를 지우고 Ctrl+Z로 되돌리며, Ctrl+Y로 다시 실행한다", async ({ browser }) => {
  const a = await open(browser, "한지우");
  const b = await open(browser, "최도윤");
  await typeInto(a, 22, F, "100");
  await a.keyboard.type("200");
  await a.keyboard.press("Enter");
  await expect(text(b, 23, F)).toHaveText("200");
  await cell(a, 22, F).click();
  await cell(a, 23, F).click({ modifiers: ["Shift"] });
  await a.keyboard.press("Delete");
  await expect(text(a, 22, F)).toHaveText("");
  await expect(text(b, 23, F)).toHaveText("");
  await a.keyboard.press("Control+z");
  await expect(text(a, 22, F)).toHaveText("100");
  await expect(text(b, 23, F)).toHaveText("200");
  await a.keyboard.press("Control+y");
  await expect(text(b, 22, F)).toHaveText("");
  await a.keyboard.press("Control+z");
  await expect(text(b, 22, F)).toHaveText("100");
});

test("Excel에서 복사한 표를 붙여넣으면 형식을 검사해 한 번에 반영한다", async ({ browser }) => {
  const a = await open(browser, "한지우");
  const b = await open(browser, "최도윤");
  await cell(a, 22, H).click();
  await paste(a, "강사료 2회\t1분기\r\n교재 30권\t5분기\r\n");
  await expect(text(b, 22, H)).toHaveText("강사료 2회");
  await expect(text(b, 22, I)).toHaveText("1분기");
  await expect(text(b, 23, H)).toHaveText("교재 30권");
  await expect(a.locator(".toast")).toContainText("형식이 맞지 않는 1칸");
  await expect(text(a, 23, I)).toHaveText("");
  // 한 칸을 범위에 붙이면 범위를 채운다.
  await cell(a, 25, F).click();
  await cell(a, 26, F).click({ modifiers: ["Shift"] });
  await paste(a, "1,500\r\n");
  await expect(text(b, 25, F)).toHaveText("1,500");
  await expect(text(b, 26, F)).toHaveText("1,500");
});

test("목록 칸은 드롭다운에서 고른다", async ({ browser }) => {
  const a = await open(browser, "오세린");
  await cell(a, 27, I).click();
  await a.getByRole("button", { name: "목록 열기" }).click();
  await a.getByRole("option", { name: "2분기" }).click();
  await expect(text(a, 27, I)).toHaveText("2분기");
  // Alt+↓로 열면 지금 값(2분기)에서 시작한다.
  await cell(a, 28, I).click();
  await a.keyboard.press("Alt+ArrowDown");
  await a.keyboard.press("ArrowDown");
  await a.keyboard.press("Enter");
  await expect(text(a, 28, I)).toHaveText("3분기");
});

test("끊긴 동안의 입력은 대기열에 남았다가(새로 고쳐도) 다시 연결되면 보낸다", async ({ browser }) => {
  const a = await open(browser, "오세린");
  const b = await open(browser, "최도윤");
  await a.evaluate(() => window.__mmTest!.offline(true));
  await expect(a.locator(".status")).toContainText("오프라인");
  await typeInto(a, 29, F, "500");
  await typeInto(a, 30, F, "600");
  await expect(a.locator(".status")).toContainText("입력 2건이 대기 중");
  await expect(text(a, 29, F)).toHaveText("500");
  await expect(text(b, 29, F)).toHaveText("");
  // 끊긴 채 새로 고침: 대기열은 브라우저(IndexedDB)에 남아 있다가 다시 들어오면 보낸다.
  await a.reload();
  await expect(a.locator(".status")).toContainText("실시간 동기화");
  await expect(text(b, 29, F)).toHaveText("500");
  await expect(text(b, 30, F)).toHaveText("600");
  await expect(a.locator(".status")).not.toContainText("대기 중");

  // 새로 고치지 않고 다시 연결해도 보낸다.
  await a.evaluate(() => window.__mmTest!.offline(true));
  await typeInto(a, 31, F, "700");
  await a.evaluate(() => window.__mmTest!.offline(false));
  await expect(text(b, 31, F)).toHaveText("700");
});

test("수식 결과(재계산)가 모두에게 반영된다", async ({ browser, request }) => {
  const a = await open(browser, "최도윤");
  expect((await request.post("/__dev/formulas", { data: [["G11", 0.065]] })).ok()).toBe(true);
  await expect(text(a, 10, G)).toHaveText("+6.5%");
});

test("행 단위 폼(1g): 형식별 입력·자동 저장·이력·다음 미입력", async ({ browser }) => {
  const a = await open(browser, "김민지");
  const b = await open(browser, "최도윤");
  await a.getByRole("tab", { name: "행 단위" }).click();
  await expect(a.getByLabel("내 담당 행")).toContainText("내 담당 행");
  await a.getByLabel("내 담당 행").getByRole("button").filter({ hasText: /^13/ }).first().click();
  await expect(a.locator(".row-head")).toContainText("13행");
  await expect(a.locator(".summary-band")).toContainText("교육훈련비");
  const amount = a.getByLabel(/신청 금액/);
  await amount.fill("구천");
  await expect(a.locator(".field-note.error")).toContainText("숫자만");
  await amount.fill("3,500");
  await a.getByLabel(/산출 근거/).click(); // 칸을 벗어나면 저장
  await expect(text(b, 12, F)).toHaveText("3,500");
  await expect(a.locator(".row-history")).toContainText("(빈 값) → 3,500");
  await a.getByRole("radio", { name: "3분기" }).click();
  await expect(text(b, 12, I)).toHaveText("3분기");
  await expect(a.locator(".compare")).toContainText("같은 비목 비교");
  await a.getByRole("button", { name: "다음 미입력 ›" }).click();
  await expect(a.locator(".row-head")).not.toContainText("13행");
  await a.getByRole("button", { name: "그리드에서 이 행 보기 →" }).click();
  await expect(a.locator(".formula-bar .addr")).toHaveText(/^F\d+$/);
});

test("조회조건·내 담당만·필수 미입력만 필터", async ({ browser }) => {
  const a = await open(browser, "최도윤");
  await expect(a.getByTestId("row-count")).toContainText("1,000행");
  await a.getByLabel("부서 조회조건").selectOption("재무팀");
  await expect(a.getByTestId("row-count")).toContainText("표시");
  const shown = await a.locator(".mmg-left .mmg-rowhead").count();
  expect(shown).toBeLessThan(40);
  await a.getByRole("button", { name: "필터 지우기" }).click();
  await a.getByLabel("내 담당만").check();
  await expect(a.getByTestId("row-count")).toContainText("표시 170행");
  await a.getByLabel("필수 미입력만").check();
  await expect(a.getByTestId("row-count")).toContainText("표시");
});

test("서식 정책: 배경색만 허용하면 채우기만 쓸 수 있다", async ({ browser, request }) => {
  const a = await open(browser, "정하늘");
  await expect(a.getByText("서식은 호스트가 관리합니다")).toBeVisible();
  expect((await request.post("/__dev/config", { data: { format: "fillOnly" } })).ok()).toBe(true);
  await expect(a.getByText("배경색만 바꿀 수 있습니다")).toBeVisible();
  await expect(a.getByRole("button", { name: "굵게" })).toBeDisabled();
  await cell(a, 19, H).click();
  await a.getByLabel("채우기", { exact: true }).click();
  await a.getByRole("button", { name: "노랑" }).click();
  await expect(cell(a, 19, H)).toHaveCSS("background-color", "rgb(255, 243, 176)");
  await request.post("/__dev/config", { data: { format: "none" } });
});

test("마감이 지나면 읽기 전용이 된다", async ({ browser, request }) => {
  const hidden = "http://127.0.0.1:8098";
  const a = await open(browser, "김민지", `${hidden}/q4`);
  expect((await request.post(`${hidden}/__dev/config`, { data: { deadline: "2000-01-01T09:00" } })).ok()).toBe(true);
  await expect(a.locator(".readonly-bar")).toContainText("마감 시각이 지났습니다 — 읽기 전용입니다");
  await cell(a, 12, F).click();
  await a.keyboard.type("1");
  await expect(a.locator(".toast")).toContainText("마감되어 입력할 수 없습니다");
  await request.post(`${hidden}/__dev/config`, { data: { deadline: "" } });
  await expect(a.locator(".readonly-bar")).toHaveCount(0);
});

test("호스트가 취합을 마감하면 사유와 함께 읽기 전용이 되고, 다시 열 수 있다", async ({ browser, request }) => {
  const a = await open(browser, "최도윤");
  expect((await request.post("/__dev/close", { data: { closed: true } })).ok()).toBe(true);
  await expect(a.locator(".readonly-bar")).toContainText("호스트가 취합을 마감했습니다");
  await cell(a, 8, F).click();
  await a.keyboard.type("1");
  await expect(a.locator(".toast").last()).toContainText("마감되어 입력할 수 없습니다");
  // 마감 뒤에 들어온 사람도 같은 안내를 본다.
  const b = await open(browser, "한지우");
  await expect(b.locator(".readonly-bar")).toContainText("호스트가 취합을 마감했습니다");
  expect((await request.post("/__dev/close", { data: { closed: false } })).ok()).toBe(true);
  await expect(a.locator(".readonly-bar")).toHaveCount(0);
  await expect(b.locator(".readonly-bar")).toHaveCount(0);
});
