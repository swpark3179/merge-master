// 앱 버전은 Cargo 작업 공간의 `[workspace.package] version` 하나로 관리한다.
// Tauri(설치 파일 이름·제품 버전)와 호스트 화면의 버전 표시가 모두 여기서 읽는다.
//
//   pnpm bump               지금 버전을 보여 준다
//   pnpm bump 1.2.0         그 버전으로 바꾼다
//   pnpm bump patch|minor|major
import { execSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(import.meta.dirname, "..");
const cargoToml = join(root, "Cargo.toml");
const section = /(\[workspace\.package\][^[]*?\nversion\s*=\s*")([^"]+)(")/;

export function readVersion() {
  const m = readFileSync(cargoToml, "utf8").match(section);
  if (!m) throw new Error("Cargo.toml의 [workspace.package] version을 찾지 못했습니다");
  return m[2];
}

function next(current, arg) {
  const [major, minor, patch] = current.split(".").map(Number);
  if (arg === "major") return `${major + 1}.0.0`;
  if (arg === "minor") return `${major}.${minor + 1}.0`;
  if (arg === "patch") return `${major}.${minor}.${patch + 1}`;
  if (!/^\d+\.\d+\.\d+$/.test(arg)) throw new Error(`버전은 x.y.z 형식이어야 합니다: ${arg}`);
  return arg;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const current = readVersion();
  const arg = process.argv[2];
  if (!arg) {
    console.log(current);
  } else {
    const version = next(current, arg);
    writeFileSync(cargoToml, readFileSync(cargoToml, "utf8").replace(section, `$1${version}$3`));
    // Cargo.lock의 작업 공간 크레이트 버전만 맞춘다(의존성은 그대로, 내려받기 없음).
    execSync("cargo update --workspace --offline", { cwd: root, stdio: "ignore" });
    console.log(`${current} → ${version}`);
  }
}
