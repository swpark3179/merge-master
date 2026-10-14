// 호스트 앱 스모크 테스트 (Excel 필요).
//
// Tauri 앱을 WebView2 원격 디버깅 포트와 함께 띄우고 Playwright로 붙어서 1a 불러오기 → 1b 취합 설정 → 1c 서비스 →
// 참여자(Chrome) 접속 → 동시 편집(진행률·Excel 수식 재계산·편집 잠금 해제) → 6 저장·마감(다른 이름으로 저장·원본에 반영·
// 원본 충돌·마감) → 7단계 세션 복구(앱 강제 종료 → 다시 켜면 끝내지 않은 업무를 저절로 이어서 엶 → 같은 포트 ·
// 참여자 자동 재접속 · 끊긴 동안의 입력 반영 · 앞 앱이 남긴 Excel 정리) → 9단계 여러 업무(두 번째 업무를 같은 포트에
// 다른 주소 경로로 공유 · 참여자 업무 목록 · 업무 끝내기)까지 확인한다. 저장 시험은 픽스처를 test-results로 복사한 통합문서로
// 하므로 픽스처는 바뀌지 않는다. 네이티브 파일 대화상자는 자동화할 수 없어 같은 command(excel_open_file)를 직접 부른다.
//
// 실행: pnpm smoke:host                     (Excel이 없을 때: '파일 열기'가 숨은 Excel 인스턴스를 쓴다)
//       pnpm smoke:host -- --running-excel   (보이는 Excel을 먼저 띄워 '실행 중인 Excel' 경로를 시험한다.
//                                             Excel이 이미 떠 있으면 건너뛴다. 끝나면 띄운 Excel을 저장 없이 닫는다)
//       pnpm smoke:release                   (pnpm app:build 로 만든 release 실행 파일 — 내장 화면 · 내장 참여자 웹)
import { execSync, spawn } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, rmSync } from "node:fs";
import { join, resolve } from "node:path";

import { chromium, expect } from "@playwright/test";

const root = resolve(import.meta.dirname, "..");
const budget = join(root, "fixtures/generated/budget.xlsx");
const variants = join(root, "fixtures/generated/variants.xlsx");
const release = process.argv.includes("--release");
const exe = join(root, `target/${release ? "release" : "debug"}/merge-master.exe`);
const CDP_PORT = 9333;
const SERVICE_PORT = 8091;
const shots = join(root, "test-results");
mkdirSync(shots, { recursive: true });

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
async function waitHttp(url, timeoutMs) {
  const until = Date.now() + timeoutMs;
  while (Date.now() < until) {
    try {
      if ((await fetch(url)).ok) return;
    } catch {}
    await sleep(300);
  }
  throw new Error(`응답 없음: ${url}`);
}
const excelPids = () =>
  execSync('tasklist /FI "IMAGENAME eq EXCEL.EXE" /FO CSV /NH', { encoding: "utf8" })
    .split("\n")
    .filter((l) => l.includes("EXCEL.EXE"))
    .map((l) => l.split(",")[1]);
function check(cond, msg) {
  if (!cond) throw new Error(`확인 실패: ${msg}`);
  console.log(`  ✓ ${msg}`);
}
const ps = (script) => execSync(`powershell -NoProfile -Command "${script}"`, { encoding: "utf8" });

for (const f of [budget, variants])
  if (!existsSync(f)) throw new Error("pnpm fixture 로 테스트용 통합문서를 먼저 만드세요");
if (!existsSync(exe))
  throw new Error(`${release ? "pnpm app:build" : "cargo build -p merge-master"} 를 먼저 실행하세요`);

// 앱을 띄우고 호스트 웹뷰에 붙는다(개발 빌드는 Vite 화면, release 빌드는 내장 화면).
async function launch() {
  const proc = spawn(exe, [], {
    cwd: root,
    stdio: "ignore",
    env: { ...process.env, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${CDP_PORT}` },
  });
  await waitHttp(`http://127.0.0.1:${CDP_PORT}/json/version`, 60_000);
  const browser = await chromium.connectOverCDP(`http://127.0.0.1:${CDP_PORT}`);
  let page;
  for (let i = 0; i < 40 && !page; i++) {
    page = browser
      .contexts()
      .flatMap((c) => c.pages())
      .find((p) => /localhost:1420|tauri\.localhost/.test(p.url()));
    if (!page) await sleep(500);
  }
  if (!page) throw new Error("호스트 웹뷰를 찾지 못했습니다");
  return { proc, browser, page };
}

const excelBefore = excelPids();
const runningMode = process.argv.includes("--running-excel");
let startedExcel = false;
if (runningMode) {
  if (excelBefore.length > 0) {
    console.log("Excel이 이미 실행 중이라 --running-excel 시험을 건너뜁니다(사용자 작업 보호).");
  } else {
    execSync(`cmd /c start "" excel "${budget}"`);
    startedExcel = true;
    const until = Date.now() + 60_000;
    while (excelPids().length === 0 && Date.now() < until) await sleep(500);
    await sleep(4000); // 통합문서가 열리고 ROT에 등록될 때까지
  }
}

const vite = release ? null : spawn("pnpm --filter @mm/host dev", { cwd: root, shell: true, stdio: "ignore" });
let app;
let participant;
try {
  if (vite) await waitHttp("http://localhost:1420", 60_000);
  const first = await launch();
  app = first.proc;
  const { page } = first;
  // 앞 실행이 끝내지 않고 남긴 업무는 다시 켜면 저절로 열린다: 모두 끝내고 빈 업무로 시작한다.
  await page.getByRole("group", { name: "취합 업무" }).waitFor({ timeout: 60_000 });
  await sleep(1000);
  const leftJobs = page.locator(".job-item").filter({ hasNotText: "파일 없음" });
  while ((await leftJobs.count()) > 0) {
    await leftJobs.first().locator(".job-end").click();
    await page.getByRole("alertdialog", { name: "업무 끝내기" }).getByRole("button", { name: "업무 끝내기" }).click();
    await expect(page.getByRole("alertdialog")).toHaveCount(0);
    await sleep(500);
  }
  await page.locator(".job-item").filter({ hasText: "파일 없음" }).first().locator(".job-pick").click();
  await page.getByRole("heading", { name: "불러오기" }).waitFor();
  if (release) check(page.url().includes("tauri.localhost"), `release 빌드: 내장 화면 (${page.url()})`);

  const commitButton = page.getByRole("button", { name: "다음: 설정 →" });
  const waitRead = () => expect(commitButton).toBeEnabled({ timeout: 60_000 });
  const invoke = (cmd, args) => page.evaluate(([c, a]) => window.__TAURI_INTERNALS__.invoke(c, a), [cmd, args]);
  /** 지금 보는 업무의 id(업무에 딸린 command를 직접 부를 때). */
  const jobId = () => page.locator(".job-item.active").getAttribute("data-job");
  const facts = async () =>
    Object.fromEntries(
      await page
        .locator(".facts dt")
        .evaluateAll((dts) => dts.map((dt) => [dt.textContent, dt.nextElementSibling?.textContent])),
    );
  const selectedText = (label) =>
    page.getByLabel(label).evaluate((el) => el.options[el.selectedIndex]?.textContent ?? "");
  const openBook = async (path, name) => {
    await invoke("excel_open_file", { path });
    // 이미 통합문서를 골랐으면 '다른 파일'로 목록으로 돌아간다.
    const other = page.getByRole("button", { name: "다른 파일" });
    if (await other.isVisible()) await other.click();
    await page.getByRole("button", { name: "↻ 다시 찾기" }).click();
    await page.getByRole("button", { name: new RegExp(name.replace(".", "\\.")) }).click();
  };
  const cellBorderColor = (r, c) =>
    page
      .locator(`.preview-grid .mmg-cell[data-r="${r}"][data-c="${c}"]`)
      .evaluate((el) => getComputedStyle(el).borderBottomColor);

  // ------------------------------------------------------------ 1a: 예산신청서
  console.log("[1a 예산신청서]");
  if (startedExcel) {
    await expect(page.getByText("● Excel 연결됨 (COM)")).toBeVisible({ timeout: 10_000 });
    const listed = page.getByRole("button", { name: /budget\.xlsx/ });
    check(!(await listed.textContent()).includes("백그라운드"), "실행 중인 Excel의 통합문서를 찾음(백그라운드 아님)");
    await listed.click();
  } else {
    await openBook(budget, "budget.xlsx");
  }
  const t0 = Date.now();
  await waitRead();
  console.log(`  감지 + 읽기 ${Date.now() - t0} ms`);
  check((await page.getByLabel("데이터 범위").inputValue()) === "A1:I1002", "표 범위 자동 감지 A1:I1002");
  await expect(page.getByText("자동 감지됨")).toBeVisible();
  check(
    (await page.getByLabel("머리글 시작 행").inputValue()) === "1" &&
      (await page.getByLabel("머리글 끝 행").inputValue()) === "2",
    "머리글 1~2행",
  );
  check((await selectedText("틀 고정")) === "A–D열", "틀 고정 A–D열");
  let f = await facts();
  check(f["데이터 행"] === "1,000" && f["열"] === "9" && f["병합 영역"] === "443", "데이터 1,000행 · 9열 · 병합 443");
  check(f["수식 셀 (G열)"] === "1,000" && f["목록 유효성 (I열)"] === "1", "수식 G열 1,000 · 목록 유효성 I열");
  await expect(page.getByText("C열 ‘담당자’ → 행 배정 기준 후보 · 6명")).toBeVisible();
  await expect(page.getByText("코드표!A2:A6 → ‘집행 시기’ 목록 원본으로 연결됨")).toBeVisible();
  check(true, "배정 기준 후보 · 목록 원본 표시");
  check((await cellBorderColor(2, 4)) === "rgb(191, 191, 191)", "원본 테두리색 표시");

  await page.locator("summary", { hasText: "가져올 항목" }).click(); // 접힌 '가져올 항목' 펼치기
  await page.getByLabel("테두리").uncheck();
  await expect(commitButton).toBeDisabled();
  await waitRead();
  check((await cellBorderColor(2, 4)) === "rgb(226, 224, 218)", "‘테두리’를 끄면 다시 읽고 테두리 없이 표시");
  await page.getByLabel("테두리").check();
  await waitRead();

  await page.getByLabel("머리글 끝 행").fill("1");
  f = await facts();
  check(f["데이터 행"] === "1,001" && (await commitButton.isEnabled()), "머리글 조정은 다시 읽지 않고 바로 반영");
  await page.getByLabel("머리글 끝 행").fill("2");

  await page.getByLabel("데이터 범위").fill("A1:I1001");
  await expect(page.getByText(/^읽는 중 ·/)).toBeVisible({ timeout: 5_000 });
  await page.getByRole("button", { name: "취소" }).click();
  await expect(page.getByText(/^읽는 중 ·/)).toBeHidden();
  check(
    (await commitButton.isDisabled()) && (await page.locator(".import-status .error").count()) === 0,
    "읽기 취소(오류 없이)",
  );
  await page.getByLabel("데이터 범위").fill("A1:I1002");
  await expect(commitButton).toBeEnabled();
  check(true, "이전에 읽은 범위로 되돌리면 다시 읽지 않음");
  await page.screenshot({ path: join(shots, "host-import-budget.png") });

  // 여러 시트: 시트를 더 고르면 그 시트를 미리 보고 미리보기 아래에 고른 시트 탭, 크게 보기에도 같은 탭.
  const previewTabs = page.locator(".preview").getByRole("tablist", { name: "시트" });
  await page.getByRole("checkbox", { name: "코드표 불러오기" }).check();
  await expect(page.getByRole("button", { name: "코드표", pressed: true })).toBeVisible();
  await expect(previewTabs.getByRole("tab")).toHaveText(["예산신청서", "코드표"]);
  await waitRead();
  await page.getByRole("button", { name: /크게 보기/ }).click();
  const big = page.getByRole("dialog", { name: /미리보기/ });
  await expect(big.getByRole("tab", { name: "코드표" })).toHaveAttribute("aria-selected", "true");
  await big.getByRole("tab", { name: "예산신청서" }).click();
  await expect(big.locator(".mmg-cell").first()).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(big).toHaveCount(0);
  await page.getByRole("checkbox", { name: "코드표 불러오기" }).uncheck();
  await expect(previewTabs).toHaveCount(0);
  await waitRead();
  check(
    (await page.getByLabel("데이터 범위").inputValue()) === "A1:I1002",
    "시트를 더 골랐다 빼면 탭이 사라지고 범위는 그대로",
  );

  // ------------------------------------------------------------ 1b: 예산신청서 취합 설정
  console.log("[1b 취합 설정]");
  await commitButton.click();
  const cfg = page.locator(".config-screen");
  const legend = cfg.locator(".legend");
  const assignTab = cfg.getByRole("tab", { name: /담당자 배정/ });
  /** 1b 담당자 배정 탭에서 행 배정 기준 열을 고르고 열 역할 탭으로 돌아온다(기본은 배정 없음). */
  const setKeyColumn = async (letter) => {
    await assignTab.click();
    const key = cfg.getByRole("toolbar", { name: "담당자 배정" }).getByLabel("행 배정 기준");
    const label = (await key.locator("option").allTextContents()).find((o) => o.startsWith(`${letter}열`));
    await key.selectOption({ label });
    await expect(assignTab).toContainText(`${letter}열 기준`);
    await cfg.getByRole("tab", { name: "열 역할" }).click();
  };
  await expect(legend).toContainText("쓸 수 있음 · 9열");
  check(
    (await legend.textContent()).includes("보기만 가능 · 0열") && (await legend.textContent()).includes("조회조건 3열"),
    "기본 제안: 모든 열 입력(9열) · 잠금 없음 · 조회 3열",
  );
  const headCell = (col) => cfg.locator(`.mmg-cell[data-r="1"][data-c="${col}"]`);
  check(
    (await assignTab.textContent()).includes("배정 안 함") &&
      (await cfg.locator(".mmg-rowhead .mmg-dot").count()) === 0 &&
      !(await headCell(2).textContent()).includes("배정 기준"),
    "기본은 담당자 배정 없음(탭 '배정 안 함', 색 점·배정 기준 태그 없음)",
  );
  await assignTab.click();
  const keyOptions = await cfg.getByLabel("행 배정 기준").locator("option").allTextContents();
  check(
    (await cfg.getByLabel("행 배정 기준").inputValue()) === "" &&
      keyOptions.some((o) => /^C열 담당자.*\(추천\)$/.test(o)),
    "담당자 배정 탭: '행 배정 없음', C열 담당자 (추천)",
  );
  await setKeyColumn("C");
  check(
    (await headCell(0).textContent()).includes("조회") &&
      (await headCell(2).textContent()).includes("배정 기준") &&
      (await headCell(8).textContent()).includes("목록"),
    "머리글 태그: 본부 조회 · 담당자 배정 기준 · 집행 시기 목록",
  );
  check((await cfg.locator(".mmg-rowhead .mmg-dot").count()) > 10, "C열을 기준으로 고르면 행 번호 옆 담당자 색 점");

  const colHead = (letter) => cfg.locator(".mmg-colhead", { hasText: new RegExp(`^${letter}$`) });
  const formula = cfg.locator(".formula-bar");
  const roleBar = cfg.getByRole("toolbar", { name: "역할 지정" });
  const columnPanel = cfg.getByRole("region", { name: "열 속성" });
  check(
    (await roleBar.getByRole("button", { name: "필수" }).count()) === 0 &&
      (await roleBar.getByRole("button", { name: "담당자 배정" }).count()) === 0 &&
      (await roleBar.getByRole("button", { name: "지우개" }).count()) === 0,
    "역할 줄: 입력/잠금 · 조회조건(필수·담당자 배정·지우개 버튼 없음)",
  );
  /** 처음엔 모든 열이 입력: 호스트가 키·실적 열(A~E, Shift로 함께)과 수식 열(G)을 잠근다. */
  const lockKeyColumns = async () => {
    const lock = roleBar.getByRole("button", { name: "잠금", exact: true });
    await colHead("A").click();
    await colHead("E").click({ modifiers: ["Shift"] });
    await lock.click();
    await colHead("G").click();
    await lock.click();
    await expect(legend).toContainText("보기만 가능 · 6열");
  };
  await lockKeyColumns();
  check((await legend.textContent()).includes("쓸 수 있음 · 3열"), "A~E·G열을 잠그면 입력 3열(F·H·I) · 잠금 6열");
  await colHead("F").click();
  await expect(formula.locator(".addr")).toHaveText("F3:F1002");
  await expect(formula.locator(".value")).toHaveText("1,000개 셀 선택됨 · 입력 · 숫자(천원)");
  check((await cfg.getByLabel("단위").inputValue()) === "천원", "열 머리글 클릭 = 데이터 행 전체 선택, 열 속성 표시");
  await columnPanel.getByLabel("필수").click();
  await expect(headCell(5)).toContainText("필수");
  const missingF = Number((await legend.textContent()).match(/비어 있음 ([\d,]+)/)[1].replace(/,/g, ""));
  check(missingF > 500 && (await cfg.locator(".mmg-cell.deco-missing").count()) > 0, `F열 필수 → 미입력 ${missingF}`);

  await colHead("E").click();
  check((await columnPanel.getByLabel("필수").count()) === 0, "잠금 열(E)은 열 속성에 필수 체크가 없음");

  // 칸 하나를 골라도 위쪽 입력/잠금은 열 전체: 칸 단위는 오른쪽 셀 예외에서만.
  await cfg.locator('.mmg-cell[data-r="5"][data-c="5"]').click();
  await expect(roleBar).toContainText("F열 전체에 적용");
  check(
    (await roleBar.getByRole("button", { name: "입력", exact: true }).getAttribute("aria-pressed")) === "true",
    "F6만 골라도 역할 줄은 F열 역할(입력)",
  );
  const exceptions = cfg.getByRole("region", { name: "셀 예외" });
  await exceptions.getByLabel("예외 종류").selectOption("locked");
  await exceptions.getByRole("button", { name: "+ 선택 셀을 예외로" }).click();
  await expect(legend).toContainText("예외 1");
  await expect(cfg.locator(".exception-row").first()).toContainText("F6");
  check(
    (await cfg.locator('.mmg-cell[data-r="5"][data-c="5"]').getAttribute("class")).includes("deco-exception"),
    "셀 예외 F6 입력 금지",
  );

  await assignTab.click();
  await colHead("I").click();
  await cfg.getByLabel("배정할 사람").fill("이서연");
  await cfg.getByLabel("배정 메모").fill("집행 시기 일괄");
  await cfg.getByRole("button", { name: "선택 범위 배정" }).click();
  await expect(cfg.getByRole("region", { name: "범위 배정" }).getByText("I3:I1002")).toBeVisible();
  check(true, "범위 배정 I3:I1002 → 이서연");
  await cfg.getByRole("tab", { name: "열 역할" }).click();

  // 크기 조절: H열 머리글 오른쪽 경계를 끌면 너비가 바뀐다.
  const hWidth = async () => (await colHead("H").boundingBox()).width;
  const widthBefore = await hWidth();
  const grip = await colHead("H").locator(".mmg-resize-col").boundingBox();
  await page.mouse.move(grip.x + grip.width / 2, grip.y + grip.height / 2);
  await page.mouse.down();
  await page.mouse.move(grip.x + 60, grip.y + 5, { steps: 5 });
  await page.mouse.up();
  await expect.poll(hWidth).toBeGreaterThan(widthBefore + 40);
  check(true, `열 머리글 경계를 끌어 H열 너비 변경 (${Math.round(widthBefore)} → ${Math.round(await hWidth())}px)`);

  // 칸 편집: 두 번 누르고 고친 뒤 Enter.
  const h4 = cfg.locator('.mmg-cell[data-r="3"][data-c="7"]');
  await h4.dblclick();
  await page.keyboard.press("Control+A");
  await page.keyboard.insertText("호스트가 고친 근거");
  await page.keyboard.press("Enter");
  await expect(h4).toContainText("호스트가 고친 근거");
  check(true, "1b에서 호스트가 칸 내용 고침(H4)");

  await cfg.getByLabel("참여자 시점").selectOption("김민지");
  await expect(cfg.locator(".viewer-banner")).toContainText("김민지 님 시점");
  const others = await cfg.locator(".mmg-cell.deco-others").count();
  check(others > 20, `김민지 시점: 남의 행은 회색 읽기 전용(${others}칸)`);
  const rowsBefore = await cfg.locator(".mmg-rowhead").count();
  await cfg.getByRole("button", { name: "숨김", exact: true }).click();
  await expect(cfg.locator(".viewer-banner")).toContainText("남의 행 숨김");
  const rowsAfter = await cfg.locator(".mmg-rowhead").count();
  check(rowsAfter < rowsBefore, `‘남의 행 숨김’이면 미리보기에서 남의 행이 사라짐 (${rowsBefore} → ${rowsAfter}행)`);
  await page.screenshot({ path: join(shots, "host-config-viewer.png") });
  await cfg.getByRole("button", { name: "읽기 전용", exact: true }).click();
  await cfg.getByRole("button", { name: "시점 끄기" }).click();
  await expect(cfg.locator(".viewer-banner")).toHaveCount(0);

  await cfg.locator('.mmg-cell[data-r="3"][data-c="7"]').click();
  await cfg.getByRole("button", { name: "서식 편집" }).click();
  await cfg.getByLabel("굵게").click();
  await expect(cfg.locator('.mmg-cell[data-r="3"][data-c="7"]')).toHaveCSS("font-weight", "700");
  check(true, "서식 도구: 굵게");
  await page.screenshot({ path: join(shots, "host-config.png") });
  await page.getByRole("button", { name: "← 불러오기" }).click();

  // ------------------------------------------------------------ 1a: 변형 통합문서
  console.log("[1a 변형: 제목 행 · 숨김 · 비활성 시트 틀 고정]");
  // 시트를 바꾼 직후엔 이전 결과로 버튼이 잠깐 켜져 있을 수 있어, 감지된 범위가 바뀐 뒤에 읽기 완료를 기다린다.
  await openBook(variants, "variants.xlsx");
  await expect(page.getByRole("button", { name: "다른시트", pressed: true })).toBeVisible();
  await expect(page.getByLabel("데이터 범위")).toHaveValue("A1:A2", { timeout: 30_000 });
  await waitRead();
  // 시트 이름을 누르면 그 시트도 고르고 미리 본다. 이 흐름은 '제목있음' 하나만 취합한다.
  await page.getByRole("button", { name: "제목있음", exact: true }).click();
  await page.getByRole("checkbox", { name: "다른시트 불러오기" }).uncheck();
  await expect(page.getByLabel("데이터 범위")).toHaveValue("A4:G25", { timeout: 30_000 });
  await waitRead();
  check(true, "제목·안내 행과 아래 빈 행을 뺀 표 범위 A4:G25");
  check(
    (await page.getByLabel("머리글 시작 행").inputValue()) === "4" &&
      (await page.getByLabel("머리글 끝 행").inputValue()) === "5",
    "세로 병합 머리글 4~5행",
  );
  const frozenLabel = await selectedText("틀 고정");
  if (frozenLabel !== "A–B열") {
    const books = await invoke("excel_list_workbooks", {});
    const id = books.workbooks.find((b) => b.name === "variants.xlsx").id;
    const ins = await invoke("excel_inspect_sheet", { workbookId: id, sheet: "제목있음" });
    console.log("  [진단] 화면:", frozenLabel, "· 감지:", ins.frozenRows, ins.frozenCols, "· 표:", ins.table);
  }
  check(frozenLabel === "A–B열", "비활성 시트의 틀 고정 A–B열");
  f = await facts();
  check(f["데이터 행"] === "19" && f["열"] === "6" && f["숨긴 행·열 (제외)"] === "1행 · 1열", "숨긴 12행·F열 제외");
  await expect(page.getByText("B열 ‘담당자’ → 행 배정 기준 후보 · 4명")).toBeVisible();
  const heads = await page.locator(".preview-grid .mmg-rowhead, .preview-grid .mmg-colhead").allTextContents();
  check(
    !heads.includes("12") && heads.includes("13") && !heads.includes("F") && heads.includes("G"),
    "미리보기에서 숨긴 행·열을 그리지 않음",
  );
  await page.screenshot({ path: join(shots, "host-import-variants.png") });

  await commitButton.click();
  await expect(cfg.locator(".legend")).toContainText("입력", { timeout: 10_000 });
  await setKeyColumn("B");
  await expect(cfg.locator('.mmg-cell[data-r="0"][data-c="1"]')).toContainText("배정 기준");
  const vHeads = await cfg.locator(".mmg-rowhead").allTextContents();
  check(
    !vHeads.includes("12") && vHeads.includes("13"),
    "다시 확정 → 1b가 새 시트로 바뀜(담당자 배정 기준, 숨긴 행 제외)",
  );

  // ------------------------------------------------------------ 1c 서비스
  console.log("[1c 서비스]");
  await page.getByRole("button", { name: "다음: 서비스 →" }).click();
  const svc = page.locator(".service-screen");
  // 이 PC는 IP 명단 밖이라 내 이름이 채워지지 않는다: 비어 있으면 빨간 테두리.
  await expect(svc.getByLabel("내 이름")).toHaveClass(/invalid/);
  check(true, "내 이름이 비어 있으면 빨간 테두리");
  await svc.getByLabel("내 이름").fill("박지훈 (기획팀)");
  await expect(svc.getByLabel("내 이름")).not.toHaveClass(/invalid/);
  await expect(svc.getByLabel("취합 제목")).toHaveValue("variants");
  check(true, "취합 제목 기본값 = 엑셀 파일 이름(variants)");
  await svc.getByLabel("취합 제목").fill("하반기 교육 수요 조사");
  await svc.getByRole("button", { name: /네트워크 설정/ }).click(); // 접힌 네트워크 설정 펼치기
  await svc.getByLabel("포트").fill(String(SERVICE_PORT)); // 앞 칸의 blur로 제목·내 이름 저장
  await expect(svc.getByRole("row")).toHaveCount(5, { timeout: 10_000 });
  check(true, "서비스 전: 명단 4명이 미접속으로 보임");
  await svc.getByRole("button", { name: "공유 시작", exact: true }).click();
  const url = (await svc.locator("code.url").textContent()) ?? "";
  check(url.endsWith(`:${SERVICE_PORT}/q4`), `접속 주소 ${url}`);
  const info = await (await fetch(`http://127.0.0.1:${SERVICE_PORT}/api/session?s=q4`)).json();
  check(info.requester === "박지훈 (기획팀)" && info.title === "하반기 교육 수요 조사", "공개 정보: 제목·요청자");
  const listed = await (await fetch(`http://127.0.0.1:${SERVICE_PORT}/api/sessions`)).json();
  check(listed.length === 1 && listed[0].slug === "q4", "업무 목록(/api/sessions)에 이 업무");
  check((await fetch(`http://127.0.0.1:${SERVICE_PORT}/api/sheet`)).status === 404, "시트는 입장 전에는 받을 수 없음");

  participant = await chromium.launch({ channel: "chrome" });
  const p2 = await participant.newPage();
  await p2.goto(url);
  await expect(p2.getByText("박지훈 (기획팀) 님의 엑셀 취합 요청")).toBeVisible();
  await p2.getByLabel("이름").fill("이서연");
  await p2.getByRole("button", { name: "참여하기" }).click();
  // 이 PC는 IP 명단 밖이라 명단에 있는 이름이어도 호스트가 승인해야 들어온다.
  await expect(p2.getByText("호스트 승인을 기다리는 중")).toBeVisible();
  await expect(svc.getByText("‘이서연’ — 명단에 있는 이름")).toBeVisible({ timeout: 10_000 });
  await svc.getByRole("button", { name: "배정 후 승인" }).click();
  await svc.getByRole("button", { name: "승인", exact: true }).click();
  await p2.locator(".mmg-cell", { hasText: "작성자 확인" }).waitFor();
  await p2.getByText("실시간 동기화").waitFor();
  const p2heads = await p2.locator(".mmg-rowhead").allTextContents();
  check(
    !p2heads.includes("12") && p2heads.includes("13"),
    "IP 명단 밖: 이름 입력 → 승인 → 입장 · 숨긴 행은 참여자에게도 보이지 않음",
  );
  const leeRow = svc.getByRole("row", { name: "이서연" });
  await expect(leeRow).toContainText("● 접속 중", { timeout: 10_000 });
  await expect(page.locator(".service-badge")).toContainText("접속 1 / 4");
  check((await leeRow.textContent()).includes("행 6–10"), "호스트 표: 이서연 접속 중 · 배정 행 6–10 · 사이드바 1 / 4");

  const p3 = await participant.newPage();
  await p3.goto(url);
  await p3.getByLabel("이름").fill("김민수");
  await p3.getByRole("button", { name: "참여하기" }).click();
  await expect(p3.getByText("호스트 승인을 기다리는 중")).toBeVisible();
  await expect(svc.getByText("‘김민수’ — 명단에 없는 이름")).toBeVisible({ timeout: 10_000 });
  await svc.getByRole("button", { name: "배정 후 승인" }).click();
  await svc.getByLabel("배정 범위").fill("E6:E10");
  await svc.getByRole("button", { name: "승인", exact: true }).click();
  await p3.locator(".mmg-cell", { hasText: "작성자 확인" }).waitFor();
  await expect(svc.getByRole("row", { name: "김민수" })).toContainText("E6:E10");
  check(true, "명단 밖 이름: 승인 대기 → 범위 배정(E6:E10) 후 승인 → 입장");

  await leeRow.getByRole("button", { name: "이서연 메뉴" }).click();
  await svc.getByRole("menuitem", { name: "연결 끊기" }).click();
  await expect(p2.getByText("호스트가 연결을 끊었습니다.")).toBeVisible();
  await expect(leeRow).toContainText("○ 미접속");
  check(true, "호스트가 연결 끊기 → 참여자에게 안내, 표는 미접속");

  check((await svc.getByText("방화벽").count()) === 0, "1c에 방화벽 확인 없음");
  await page.screenshot({ path: join(shots, "host-service.png") });

  // ------------------------------------------------------------ 4단계: 동시 편집 · Excel 재계산 (예산신청서)
  console.log("[1f 동시 편집 · Excel 재계산]");
  const step = (label) => page.locator(".steps").getByRole("button", { name: new RegExp(`^${label}`) });
  await step("1 불러오기").click();
  // 저장 시험(원본에 반영)을 위해 픽스처 사본으로 한다.
  const work = join(shots, "smoke-budget.xlsx");
  copyFileSync(budget, work);
  await openBook(work, "smoke-budget.xlsx");
  await expect(page.getByLabel("데이터 범위")).toHaveValue("A1:I1002", { timeout: 30_000 });
  await waitRead();
  await commitButton.click();
  await expect(cfg.locator(".legend")).toContainText("쓸 수 있음 · 9열", { timeout: 10_000 });
  await lockKeyColumns();
  await setKeyColumn("C");
  await colHead("F").click();
  await columnPanel.getByLabel("필수").click();
  await expect(headCell(5)).toContainText("필수");
  // 서비스 중에 다시 확정: 접속 중이던 명단 밖 참여자(김민수)는 새 명단에 없어 끊긴다.
  await expect(p3.getByText("명단에서 빠졌습니다")).toBeVisible({ timeout: 10_000 });
  check(true, "서비스 중 다시 확정 → 새 명단에 없는 참여자는 끊김");
  await step("3 서비스").click();
  await expect(svc.getByLabel("수식 계산")).toContainText("Excel이 바로 계산 (수식 1,000칸)", { timeout: 60_000 });
  check(true, "계산 사본(숨은 Excel) 준비 · 수식 1,000칸");

  const p4 = await participant.newPage();
  await p4.goto(url);
  await p4.getByLabel("이름").fill("이서연");
  await p4.getByRole("button", { name: "참여하기" }).click();
  await expect(svc.getByText("‘이서연’ — 명단에 있는 이름")).toBeVisible({ timeout: 10_000 });
  await svc.getByRole("button", { name: "배정 후 승인" }).click();
  await svc.getByRole("button", { name: "승인", exact: true }).click();
  await p4.getByText("실시간 동기화").waitFor();
  const lee = svc.getByRole("row", { name: "이서연" });
  await expect(lee).toContainText("● 접속 중", { timeout: 10_000 });
  const progressBefore = await lee.locator(".pprogress").textContent();
  const pcell = (r, c) => p4.locator(`.mmg-cell[data-r="${r}"][data-c="${c}"]`);
  await pcell(2, 5).click();
  await p4.keyboard.type("9,800");
  await p4.keyboard.press("Enter");
  await expect(pcell(2, 5).locator(".mmg-text")).toHaveText("9,800");
  await expect(pcell(2, 6).locator(".mmg-text")).toHaveText("+6.5%", { timeout: 15_000 });
  check(true, "참여자 입력 F3 = 9,800 → Excel 계산 사본이 G3 증감률 +6.5%로 다시 계산해 보냄");
  await expect(lee.locator(".pprogress")).not.toHaveText(progressBefore ?? "", { timeout: 10_000 });
  check(
    true,
    `호스트 표 필수 진행률 실시간 반영 (${progressBefore?.trim()} → ${(await lee.locator(".pprogress").textContent())?.trim()})`,
  );

  await pcell(3, 5).click();
  await p4.keyboard.type("44");
  await expect(lee).toContainText("✎ F4", { timeout: 10_000 });
  await lee.getByRole("button", { name: "이서연 메뉴" }).click();
  await svc.getByRole("menuitem", { name: "편집 잠금 해제" }).click();
  await expect(p4.locator(".toast", { hasText: "호스트가 편집 잠금을 해제했습니다" })).toBeVisible({ timeout: 10_000 });
  await expect(lee).not.toContainText("✎", { timeout: 10_000 });
  check(true, "편집 중 표시(✎ F4) → 호스트 ‘편집 잠금 해제’ → 참여자 편집기 닫힘");
  await p4.screenshot({ path: join(shots, "participant-editing.png") });
  await page.screenshot({ path: join(shots, "host-service-editing.png") });

  // ------------------------------------------------------------ 6단계: 4 진행 모니터링(1d) · 1i 소통
  console.log("[4 진행 모니터링 · 소통]");
  await step("4 모니터링").click();
  const mon = page.locator(".monitor-screen");
  await expect(mon.getByLabel("전체 필수 진행률")).toContainText("%", { timeout: 15_000 });
  // 뒤에서 이 시점으로 복원한다(5 히스토리).
  await mon.getByRole("button", { name: "스냅샷", exact: true }).click();
  await expect(mon.getByRole("status")).toContainText("스냅샷을 남겼습니다");
  const mcell = (r, c) => mon.locator(`.mmg-cell[data-r="${r}"][data-c="${c}"]`);
  await pcell(4, 5).click();
  await p4.keyboard.type("4,500");
  await p4.keyboard.press("Enter");
  await expect(mcell(4, 5).locator(".mmg-text")).toHaveText("4,500", { timeout: 10_000 });
  await expect(mon.getByLabel("실시간 활동")).toContainText("4,500");
  check(true, "참여자 입력 → 모니터링 그리드·활동 피드에 실시간 반영");
  await mon
    .getByRole("button", { name: /이서연/ })
    .first()
    .click();
  await expect(mon.getByTestId("mon-rows")).toContainText("1,000행 중 170행");
  await mon.getByRole("button", { name: "필터 지우기" }).click();
  check(true, "담당자별 진행률을 누르면 그 사람 행만(170행)");

  // 크기 조절: 호스트가 모니터링 그리드의 H열 머리글 경계를 끌면 참여자 화면에도 바로 보인다(참여자는 기본 금지).
  const colHeadOf = (scope, letter) => scope.locator(".mmg-colhead", { hasText: new RegExp(`^${letter}$`) });
  check(
    (await colHeadOf(p4, "H").locator(".mmg-resize-col").count()) === 0,
    "참여자 크기 조절은 기본 금지(손잡이 없음)",
  );
  const pWidth = async () => (await colHeadOf(p4, "H").boundingBox()).width;
  const pWidthBefore = await pWidth();
  const monGrip = await colHeadOf(mon, "H").locator(".mmg-resize-col").boundingBox();
  await page.mouse.move(monGrip.x + monGrip.width / 2, monGrip.y + monGrip.height / 2);
  await page.mouse.down();
  await page.mouse.move(monGrip.x + 50, monGrip.y + 5, { steps: 5 });
  await page.mouse.up();
  await expect.poll(pWidth, { timeout: 10_000 }).toBeGreaterThan(pWidthBefore + 30);
  check(
    true,
    `모니터링에서 H열 너비 변경 → 참여자 화면에 반영 (${Math.round(pWidthBefore)} → ${Math.round(await pWidth())}px)`,
  );

  // 범위 삭제 → 빨간 묶음 → 그 자리에서 되돌리기
  await pcell(2, 5).click();
  await pcell(4, 5).click({ modifiers: ["Shift"] });
  await p4.keyboard.press("Delete");
  await expect(pcell(2, 5).locator(".mmg-text")).toHaveText("");
  const alert = mon.locator(".feed-item.alert").first();
  await expect(alert).toContainText("범위 삭제 2셀", { timeout: 10_000 });
  await alert.getByRole("button", { name: "되돌리기" }).click();
  await expect(pcell(2, 5).locator(".mmg-text")).toHaveText("9,800", { timeout: 10_000 });
  await expect(pcell(4, 5).locator(".mmg-text")).toHaveText("4,500");
  check(true, "범위 삭제가 빨간 묶음으로 뜨고, 되돌리기로 참여자 화면까지 복원");

  // 공지(중요·선택한 사람) → 참여자 상단 고정 → 확인 → 읽음 1/1
  await mon.getByRole("tab", { name: "공지" }).click();
  const noticePanel = mon.getByLabel("공지 보내기");
  await noticePanel.getByRole("button", { name: "선택…" }).click();
  await noticePanel.getByLabel("이서연").check();
  await noticePanel.getByLabel("중요").check();
  await noticePanel.getByLabel("공지 내용").fill("오늘 17시까지 산출 근거 입력 부탁드립니다.");
  await noticePanel.getByRole("button", { name: "공지 보내기" }).click();
  await expect(p4.locator(".important-bar")).toContainText("오늘 17시까지", { timeout: 10_000 });
  await p4.locator(".important-bar").getByRole("button", { name: "확인" }).click();
  await expect(noticePanel).toContainText("읽음 1/1", { timeout: 10_000 });
  check(true, "중요 공지 → 참여자 상단 고정 → 확인 → 호스트에 읽음 1/1");

  // 셀 강조: 모니터링 그리드에서 H3 선택 → 강조 → 참여자 말풍선·셀로 이동 → 확인함
  await mcell(2, 7).click();
  await mon.getByRole("button", { name: "선택 셀 강조" }).click();
  const hl = mon.getByLabel("셀 강조");
  await expect(hl).toContainText("H3");
  await hl.getByLabel("강조 메모").fill("교육 과정명과 인원을 적어 주세요.");
  await hl.getByRole("button", { name: "강조 표시" }).click();
  await expect(pcell(2, 7).locator(".callout")).toHaveText("교육 과정명과 인원을 적어 주세요.", { timeout: 10_000 });
  await p4.locator(".toast", { hasText: "H3 셀을 강조했습니다" }).getByRole("button", { name: "셀로 이동" }).click();
  await expect(hl).toContainText("이서연 · 확인함", { timeout: 10_000 });
  check(true, "셀 강조 → 참여자 노란 표시·말풍선 → 셀로 이동 → 호스트에 확인함");

  // 채팅: 참여자 → 전체, 호스트 → 1:1
  await p4.locator(".side-panel").getByRole("tab", { name: /^채팅/ }).click();
  await p4.getByLabel("채팅 입력").fill("#F3 금액 확인 부탁드립니다");
  await p4.getByLabel("채팅 입력").press("Enter");
  await mon.getByRole("tab", { name: /^채팅/ }).click();
  const chat = mon.getByLabel("채팅", { exact: true });
  await expect(chat).toContainText("금액 확인 부탁드립니다", { timeout: 10_000 });
  await chat.getByLabel("받는 사람").selectOption("이서연");
  await chat.getByLabel("채팅 입력").fill("확인했습니다 #F3");
  await chat.getByLabel("채팅 입력").press("Enter");
  await expect(p4.getByLabel("채팅 내용")).toContainText("확인했습니다", { timeout: 10_000 });
  check(true, "채팅: 참여자 → 호스트 화면, 호스트 1:1 답장 → 참여자");
  await page.screenshot({ path: join(shots, "host-monitor.png") });
  await p4.screenshot({ path: join(shots, "participant-comms.png") });

  // ------------------------------------------------------------ 6단계: 5 히스토리(1h)
  console.log("[5 히스토리]");
  await step("5 히스토리").click();
  const hist = page.locator(".history-screen");
  const log = hist.getByLabel("변경 기록");
  await expect(log).toContainText("범위 삭제 · 2셀", { timeout: 10_000 });
  await expect(log).toContainText("되돌림 · 2셀");
  await log.getByRole("row").filter({ hasText: "4,500" }).first().click();
  await expect(hist.getByLabel("셀 타임라인")).toContainText("F5");
  await expect(hist.getByLabel("셀 타임라인")).toContainText("원본");
  check(true, "변경 기록이 묶음(범위 삭제·되돌림)으로 보이고, 칸을 누르면 셀 타임라인");
  const csvPath = join(shots, "history.csv");
  const lines = await invoke("history_export_csv", { job: await jobId(), filter: {}, path: csvPath });
  check(lines >= 5 && existsSync(csvPath), `CSV 내보내기 ${lines}건`);

  const snaps = hist.getByLabel("스냅샷");
  await snaps.getByRole("button", { name: /수동/ }).first().click();
  await expect(snaps).toContainText("현재와 1셀 다름");
  await snaps.getByRole("button", { name: "비교 보기" }).click();
  await expect(page.getByRole("dialog", { name: "스냅샷 비교" })).toContainText("F5");
  await page.getByRole("dialog", { name: "스냅샷 비교" }).getByRole("button", { name: "닫기" }).click();
  await snaps.getByRole("button", { name: "이 시점으로 복원" }).click();
  await hist.getByRole("button", { name: "그래도 되돌리기" }).click();
  await expect(hist.getByRole("status")).toContainText("스냅샷으로 복원했습니다(1칸)", { timeout: 10_000 });
  await expect(pcell(4, 5).locator(".mmg-text")).toHaveText("", { timeout: 10_000 });
  await expect(snaps.getByRole("button", { name: /복원 직전/ })).toBeVisible({ timeout: 10_000 });
  check(true, "스냅샷 비교(F5) → 이 시점으로 복원 → 참여자 화면 반영 · '복원 직전' 스냅샷 생김");
  await page.screenshot({ path: join(shots, "host-history.png") });

  // ------------------------------------------------------------ 5단계: 6 저장·마감
  console.log("[6 저장·마감]");
  // 저장한 파일은 별도의 Excel(읽기 전용)로 열어 확인한다. 출력은 ASCII만(콘솔 코드 페이지).
  const readBook = async (file) => {
    const before = excelPids();
    const out = ps(
      `$x=New-Object -ComObject Excel.Application; $x.DisplayAlerts=$false; $b=$x.Workbooks.Open('${file}', 0, $true); ` +
        `$s=$b.Worksheets.Item(1); $h=$b.Worksheets.Item($b.Worksheets.Count); ` +
        `Write-Output ('{0}|{1}|{2}|{3}|{4}' -f $s.Range('F3').Value2, [math]::Round($s.Range('G3').Value2, 4), ` +
        `$b.Worksheets.Count, $h.Range('C2').Value2, $s.Range('F3').Interior.Color); $b.Close($false); $x.Quit()`,
    )
      .trim()
      .split("|");
    // 확인용으로 띄운 Excel이 끝날 때까지 기다린다(뒤의 복구 시험이 Excel 수를 센다).
    for (let i = 0; i < 60 && excelPids().some((p) => !before.includes(p)); i++) await sleep(250);
    return out;
  };
  await step("6 저장").click();
  const save = page.locator(".save-screen");
  await expect(save.getByLabel("필수 완료율")).toContainText("%");
  await expect(save.locator(".stat-row")).toContainText("바뀐 칸 1");
  await save.locator("summary", { hasText: "담당자별 미입력" }).click(); // 접힌 표 펼치기
  await expect(save.getByRole("row", { name: "이서연" })).toContainText("56 / 170");
  check(true, "요약: 필수 완료율 · 바뀐 칸 1 · 담당자별 미입력");

  const copyPath = join(shots, "smoke-budget-copy.xlsx");
  rmSync(copyPath, { force: true });
  await save.getByLabel("저장할 파일").fill(copyPath);
  await save.getByLabel("바뀐 칸을 노란색으로 표시").check();
  await save.getByRole("button", { name: "저장하기" }).click();
  const saved = save.locator(".save-result");
  await expect(saved).toContainText("저장했어요", { timeout: 60_000 });
  await expect(saved).toContainText("값 1칸");
  await expect(saved).toContainText("‘변경 이력’ 시트");
  await expect(saved).toContainText("다시 읽어 확인함");
  let [f3, g3, sheets, histCell, color] = await readBook(copyPath);
  check(
    f3 === "9800" && g3 === "0.0652" && sheets === "4" && histCell === "F3" && color === String(0xb0f3ff),
    `다른 이름으로 저장: F3=9,800 · G3 재계산 · ‘변경 이력’ 시트(F3) · 노란 표시 (${f3}|${g3}|${sheets}|${histCell}|${color})`,
  );

  let expectF3 = "9800";
  if (startedExcel) {
    // 원본(사용자 Excel)에서 F3를 직접 고친다 → 저장할 때 충돌로 묻는다.
    ps(
      "$x=[Runtime.InteropServices.Marshal]::GetActiveObject('Excel.Application'); " +
        "$x.Workbooks.Item('smoke-budget.xlsx').Worksheets.Item(1).Range('F3').Value2=1234",
    );
    expectF3 = "1234";
  }
  await saved.getByRole("button", { name: "저장 화면으로 돌아가기" }).click();
  await save.getByLabel("원본 파일에 반영").check();
  await save.getByLabel(/변경 이력.* 시트 추가/).uncheck();
  await save.getByLabel("바뀐 칸을 노란색으로 표시").uncheck();
  await save.getByLabel("읽기 전용으로 전환 (취합 마감)").check();
  await save.getByRole("button", { name: "저장하기" }).click();
  if (startedExcel) {
    const review = save.getByRole("region", { name: "저장 전 확인" });
    await expect(review).toContainText("같은 칸 1개가 겹칩니다", { timeout: 30_000 });
    await review.getByRole("row", { name: "F3" }).getByLabel("1,234").check();
    await review.getByRole("button", { name: "이대로 저장" }).click();
    await expect(saved).toContainText("Excel 값 유지 1칸", { timeout: 60_000 });
    await expect(pcell(2, 5).locator(".mmg-text")).toHaveText("1,234", { timeout: 10_000 });
    check(true, "원본 충돌: Excel에서 고친 F3 → ‘Excel 값 유지’ → 참여자 화면도 1,234");
  }
  await expect(saved).toContainText("저장했어요", { timeout: 60_000 });
  await expect(saved).toContainText("취합을 마감했습니다");
  await expect(p4.locator(".readonly-bar")).toContainText("호스트가 취합을 마감했습니다", { timeout: 10_000 });
  await expect(page.locator(".service-badge")).toContainText("취합 마감됨");
  [f3, , sheets] = await readBook(work);
  check(
    f3 === expectF3 && sheets === "3",
    `원본에 반영(파일 다시 열어 F3=${f3}, 시트 ${sheets}개) → 참여자 읽기 전용(마감)`,
  );
  await saved.getByRole("button", { name: "저장 화면으로 돌아가기" }).click();
  await save.getByRole("button", { name: "취합 다시 열기" }).click();
  await expect(p4.locator(".readonly-bar")).toHaveCount(0, { timeout: 10_000 });
  await expect(save.locator(".stat-row")).toContainText("바뀐 칸 0", { timeout: 10_000 });
  check(true, "원본에 반영한 값이 새 기준 → 바뀐 칸 0 · 취합 다시 열기");
  await page.screenshot({ path: join(shots, "host-save.png") });

  // ------------------------------------------------------------ 7단계: 앱 강제 종료 → 다시 켜면 저절로 이어서
  console.log("[7 세션 복구]");
  await step("4 모니터링").click();
  await pcell(4, 5).click();
  await p4.keyboard.type("7,700");
  await p4.keyboard.press("Enter");
  await expect(mcell(4, 5).locator(".mmg-text")).toHaveText("7,700", { timeout: 10_000 });
  await sleep(1000); // 기록 파일에 쓰일 때까지
  execSync(`taskkill /PID ${app.pid} /F`, { stdio: "ignore" });
  await expect(p4.getByText("○ 오프라인")).toBeVisible({ timeout: 15_000 });
  const orphans = excelPids().filter((p) => !excelBefore.includes(p));
  await pcell(3, 5).click();
  await p4.keyboard.type("3,300");
  await p4.keyboard.press("Enter");
  await expect(p4.getByText("입력 1건이 대기 중입니다")).toBeVisible({ timeout: 10_000 });
  check(true, `앱 강제 종료 → 참여자 오프라인, 그동안의 입력은 대기열(1건) · 남은 Excel ${orphans.length}개`);

  // 죽은 앱의 웹뷰가 끝나 디버깅 포트가 풀릴 때까지 기다린다.
  for (let i = 0; i < 40; i++) {
    try {
      await fetch(`http://127.0.0.1:${CDP_PORT}/json/version`);
      await sleep(500);
    } catch {
      break;
    }
  }
  const second = await launch();
  app = second.proc;
  const page2 = second.page;
  const restoredJob = page2.locator(".job-item").filter({ hasText: "smoke-budget.xlsx" });
  await expect(restoredJob).toBeVisible({ timeout: 60_000 });
  await expect(restoredJob).toContainText("공유 중", { timeout: 60_000 });
  check(true, "다시 켜면 끝내지 않은 업무를 저절로 이어서 열고 같은 포트·주소로 다시 공유");
  await page2.screenshot({ path: join(shots, "host-recovery.png") });
  const mon2 = page2.locator(".monitor-screen");
  const mcell2 = (r, c) => mon2.locator(`.mmg-cell[data-r="${r}"][data-c="${c}"]`);
  await expect(mon2).toBeVisible();
  await expect(mcell2(4, 5).locator(".mmg-text")).toHaveText("7,700");
  await expect(page2.locator(".service-badge")).toContainText("공유 중");
  check(true, "이어서 연 업무 → 진행 화면 · 죽기 직전 값(F5 7,700) · 같은 포트로 서비스 다시 시작");

  await expect(p4.getByText("● 실시간 동기화")).toBeVisible({ timeout: 30_000 });
  await expect(mcell2(3, 5).locator(".mmg-text")).toHaveText("3,300", { timeout: 10_000 });
  await expect(pcell(4, 5).locator(".mmg-text")).toHaveText("7,700");
  check(true, "참여자 자동 재접속 → 끊긴 동안의 입력(F4 3,300)이 호스트에 반영 · 값 유지");

  const step2 = (label) => page2.locator(".steps").getByRole("button", { name: new RegExp(`^${label}`) });
  const svc2 = page2.locator(".service-screen");
  await step2("3 서비스").click();
  await expect(svc2.getByLabel("수식 계산")).toContainText("Excel이 바로 계산 (수식 1,000칸)", { timeout: 60_000 });
  await expect(svc2.getByRole("row", { name: "이서연" })).toContainText("● 접속 중");
  const afterResume = excelPids().filter((p) => !excelBefore.includes(p));
  console.log(`  Excel PID: 강제 종료 뒤 ${orphans.join(",")} → 이어서 한 뒤 ${afterResume.join(",")}`);
  check(
    afterResume.length === orphans.length && orphans.some((p) => !afterResume.includes(p)),
    `앞 앱의 계산 사본 Excel은 끝내고 새로 띄움(Excel ${orphans.length}개 → ${afterResume.length}개, 교체 ${orphans.filter((p) => !afterResume.includes(p)).length}개)`,
  );
  await pcell(2, 5).click();
  await p4.keyboard.type("9,900");
  await p4.keyboard.press("Enter");
  await expect(pcell(2, 6).locator(".mmg-text")).toHaveText("+7.6%", { timeout: 15_000 });
  check(true, "이어서 입력 F3 = 9,900 → 새 계산 사본이 G3 +7.6%로 다시 계산");

  await step2("4 모니터링").click();
  await mon2.getByRole("tab", { name: /^채팅/ }).click();
  await expect(mon2.getByLabel("채팅", { exact: true })).toContainText("금액 확인 부탁드립니다");
  await step2("5 히스토리").click();
  const hist2 = page2.locator(".history-screen");
  await expect(hist2.getByLabel("변경 기록")).toContainText("범위 삭제 · 2셀", { timeout: 10_000 });
  await expect(hist2.getByLabel("스냅샷").getByRole("button", { name: /복원 직전/ })).toBeVisible();
  check(true, "채팅 · 변경 기록 · 스냅샷도 되살아남");
  await page2.screenshot({ path: join(shots, "host-recovered.png") });

  // ------------------------------------------------------------ 9단계: 여러 업무 동시 진행
  console.log("[9 여러 업무]");
  const invoke2 = (cmd, args) => page2.evaluate(([c, a]) => window.__TAURI_INTERNALS__.invoke(c, a), [cmd, args]);
  const endJob = async (hasText) => {
    await page2.locator(".job-item").filter({ hasText }).locator(".job-end").click();
    await page2.getByRole("alertdialog", { name: "업무 끝내기" }).getByRole("button", { name: "업무 끝내기" }).click();
    await expect(page2.getByRole("alertdialog")).toHaveCount(0);
  };
  await page2.getByRole("button", { name: "+ 새 취합" }).click();
  await page2.getByRole("heading", { name: "불러오기" }).waitFor();
  await invoke2("excel_open_file", { path: variants });
  await page2.getByRole("button", { name: "↻ 다시 찾기" }).click();
  await page2.getByRole("button", { name: /variants\.xlsx/ }).click();
  await page2.getByRole("button", { name: "제목있음", exact: true }).click();
  await page2.getByRole("checkbox", { name: "다른시트 불러오기" }).uncheck();
  await expect(page2.getByLabel("데이터 범위")).toHaveValue("A4:G25", { timeout: 30_000 });
  const commit2 = page2.getByRole("button", { name: "다음: 설정 →" });
  await expect(commit2).toBeEnabled({ timeout: 60_000 });
  await commit2.click();
  await page2.getByRole("button", { name: "다음: 서비스 →" }).click();
  const svc3 = page2.locator(".service-screen");
  await svc3.getByLabel("내 이름").fill("박지훈 (기획팀)");
  await svc3.getByLabel("취합 제목").fill("교육 수요 2차 조사");
  await svc3.getByRole("button", { name: /네트워크 설정/ }).click();
  await expect(svc3.getByLabel("포트")).toHaveValue(String(SERVICE_PORT));
  await expect(svc3.getByLabel("포트")).toBeDisabled();
  check(true, "두 번째 업무: 포트는 공유 중인 첫 업무와 같이 씀(바꿀 수 없음)");
  await svc3.getByLabel("주소 경로").fill("hr");
  await svc3.getByRole("button", { name: "공유 시작", exact: true }).click();
  const url3 = (await svc3.locator("code.url").first().textContent()) ?? "";
  check(url3.endsWith(`:${SERVICE_PORT}/hr`), `두 번째 업무 주소 ${url3}`);
  const both = await (await fetch(`http://127.0.0.1:${SERVICE_PORT}/api/sessions`)).json();
  check(
    both.length === 2 && both.some((s) => s.title === "교육 수요 2차 조사"),
    `참여자 업무 목록: ${both.map((s) => s.title).join(", ")}`,
  );
  const p5 = await participant.newPage();
  await p5.goto(`http://127.0.0.1:${SERVICE_PORT}/`);
  await expect(p5.getByRole("list", { name: "취합 업무 목록" }).getByRole("link")).toHaveCount(2);
  await p5.getByRole("link", { name: /교육 수요 2차 조사/ }).click();
  await expect(p5.getByRole("heading", { name: "교육 수요 2차 조사" })).toBeVisible();
  check(true, "참여자: 같은 주소(/)의 업무 목록에서 두 번째 업무를 골라 들어감");
  await page2.screenshot({ path: join(shots, "host-jobs.png") });

  await restoredJob.locator(".job-pick").click();
  await expect(page2.locator(".service-badge")).toContainText("다른 업무 1개 공유 중");
  await step2("4 모니터링").click();
  await expect(mcell2(4, 5).locator(".mmg-text")).toHaveText("7,700");
  check(true, "첫 업무로 돌아가도 진행 화면·값 그대로");
  await endJob("교육 수요 2차 조사");
  await expect(p5.getByText("호스트가 이 업무를 끝냈습니다")).toBeVisible({ timeout: 10_000 });
  await expect(p4.getByText("● 실시간 동기화")).toBeVisible();
  check(true, "두 번째 업무 끝내기 → 그 업무 참여자만 '끝났습니다', 첫 업무 참여자는 계속");

  await step2("3 서비스").click();
  await svc2.getByRole("button", { name: "공유 중지" }).click();
  await expect(p4.getByText("서비스가 중지되었습니다")).toBeVisible({ timeout: 10_000 });
  check(true, "서비스 중지 → 참여자에게 안내");
  // 시험 업무도 끝내 둔다(다시 켜도 되살리지 않는다).
  await endJob("smoke-budget.xlsx");
  await expect(page2.locator(".job-item")).toHaveCount(1);
  check(true, "업무 끝내기 → 목록에서 빠지고 빈 업무만 남음");
  await second.browser.close();
} finally {
  await participant?.close();
  // 창 닫기(WM_CLOSE)로 정상 종료시켜 RunEvent::Exit에서 숨은 Excel이 정리되게 한다.
  if (app?.pid) {
    try {
      execSync(`taskkill /PID ${app.pid}`, { stdio: "ignore" });
    } catch {}
  }
  if (vite?.pid) {
    try {
      execSync(`taskkill /PID ${vite.pid} /T /F`, { stdio: "ignore" });
    } catch {}
  }
  if (startedExcel) {
    await sleep(2000);
    // 시험용으로 띄운 Excel만 저장하지 않고 닫는다.
    ps(
      "$x=[Runtime.InteropServices.Marshal]::GetActiveObject('Excel.Application'); $x.DisplayAlerts=$false; foreach($b in @($x.Workbooks)){ $b.Close($false) }; $x.Quit()",
    );
  }
}

await sleep(4000);
const leftover = excelPids().filter((p) => !excelBefore.includes(p));
check(leftover.length === 0, "앱 종료 후 남은 Excel 인스턴스 없음");
console.log("호스트 스모크 테스트 통과");
