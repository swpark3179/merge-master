// 설치 파일(NSIS) 만들기: pnpm app:build
//
// 1) 참여자 웹을 빌드한다(collect-server 바이너리에 들어간다).
// 2) tauri build — 호스트 화면을 빌드하고 release 실행 파일과 NSIS 설치 파일을 만든다.
//    사내망(인터넷 없음)에서도 설치되도록 WebView2 오프라인 설치 파일을 넣는다(tauri.conf.json `webviewInstallMode`).
//    빌드하는 PC는 처음 한 번 NSIS 도구와 WebView2 오프라인 설치 파일을 내려받아야 한다(이후에는 캐시를 쓴다).
//    MM_WEBVIEW=bootstrapper 이면 오프라인 설치 파일 대신 작은 부트스트래퍼를 넣는다(약 190MB 작아지지만, WebView2가 없는
//    PC는 설치할 때 인터넷이 필요하다. Windows 11과 업데이트된 Windows 10에는 WebView2가 이미 있다) → 이름에 `-bootstrap`.
// 3) release/ 에 `merge-master_<버전>_x64-setup.exe`와 SHA256SUMS.txt를 둔다.
//
// 코드 서명(선택, 환경 변수):
//   MM_SIGN_THUMBPRINT   Windows 인증서 저장소에 있는 코드 서명 인증서의 지문 → signtool로 서명
//   MM_SIGN_TIMESTAMP    타임스탬프 서버(기본 http://timestamp.digicert.com)
//   MM_SIGN_COMMAND      다른 서명 도구를 쓸 때의 명령(파일 자리는 %1), 예: "signtool sign /f cert.pfx /p ... /fd sha256 %1"
// 서명하지 않으면 설치할 때 SmartScreen이 '알 수 없는 게시자'로 경고한다.
import { execSync } from "node:child_process";
import { createHash } from "node:crypto";
import { copyFileSync, existsSync, mkdirSync, readdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";

import { readVersion } from "./version.mjs";

const root = resolve(import.meta.dirname, "..");
const version = readVersion();
const run = (cmd, env = {}) => execSync(cmd, { cwd: root, stdio: "inherit", env: { ...process.env, ...env } });

const sign = {};
if (process.env.MM_SIGN_COMMAND) {
  sign.signCommand = process.env.MM_SIGN_COMMAND;
} else if (process.env.MM_SIGN_THUMBPRINT) {
  sign.certificateThumbprint = process.env.MM_SIGN_THUMBPRINT.replace(/\s/g, "");
  sign.digestAlgorithm = "sha256";
  sign.timestampUrl = process.env.MM_SIGN_TIMESTAMP ?? "http://timestamp.digicert.com";
}
const signed = Object.keys(sign).length > 0;
const bootstrap = process.env.MM_WEBVIEW === "bootstrapper";
const windows = { ...sign, ...(bootstrap ? { webviewInstallMode: { type: "embedBootstrapper", silent: true } } : {}) };

console.log(`엑셀 취합 도구 ${version} 설치 파일을 만듭니다${signed ? " (코드 서명)" : " (서명 안 함)"}`);
run("pnpm build:participant");
let cmd = "pnpm tauri build";
const overrides = join(root, "target", "tauri-build-overrides.json");
if (Object.keys(windows).length > 0) {
  mkdirSync(join(root, "target"), { recursive: true });
  writeFileSync(overrides, JSON.stringify({ bundle: { windows } }));
  cmd += ` --config "${overrides}"`;
}
run(cmd);

const nsisDir = join(root, "target", "release", "bundle", "nsis");
const built = readdirSync(nsisDir)
  .filter((f) => f.endsWith("-setup.exe") && f.includes(`_${version}_`))
  .map((f) => join(nsisDir, f))
  .sort((a, b) => statSync(b).mtimeMs - statSync(a).mtimeMs)[0];
if (!built || !existsSync(built)) throw new Error(`설치 파일을 찾지 못했습니다: ${nsisDir}`);

const out = join(root, "release");
mkdirSync(out, { recursive: true });
const name = `merge-master_${version}_x64${bootstrap ? "-bootstrap" : ""}-setup.exe`;
copyFileSync(built, join(out, name));
const hash = createHash("sha256")
  .update(readFileSync(join(out, name)))
  .digest("hex");
// 같은 버전의 다른 설치 파일(오프라인·부트스트래퍼) 줄은 남긴다.
const sums = join(out, "SHA256SUMS.txt");
const kept = existsSync(sums)
  ? readFileSync(sums, "utf8")
      .split("\n")
      .filter((l) => l.trim() && !l.endsWith(`  ${name}`))
  : [];
writeFileSync(sums, [...kept, `${hash}  ${name}`].join("\n") + "\n");

const mb = (statSync(join(out, name)).size / 1024 / 1024).toFixed(1);
console.log(`\n설치 파일: release/${name} (${mb} MB)`);
console.log(`SHA-256:   ${hash}`);
if (signed) {
  const status = execSync(`powershell -NoProfile -Command "(Get-AuthenticodeSignature '${join(out, name)}').Status"`, {
    encoding: "utf8",
  }).trim();
  console.log(`서명 상태: ${status}`);
  if (status !== "Valid") process.exitCode = 1;
}
