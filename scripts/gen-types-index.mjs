// ts-rs가 packages/shared/src/generated 에 만든 타입 파일들을 index.ts 하나로 다시 내보낸다.
// 실행: pnpm types  (cargo test로 타입을 생성한 뒤 이 스크립트를 돌린다)
import { readdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const dir = join(import.meta.dirname, "../packages/shared/src/generated");
const names = readdirSync(dir)
  .filter((f) => f.endsWith(".ts") && f !== "index.ts")
  .map((f) => f.slice(0, -3))
  .sort();
const body = names.map((n) => `export type { ${n} } from "./${n}";`).join("\n");
writeFileSync(join(dir, "index.ts"), `// 생성 파일 — scripts/gen-types-index.mjs\n${body}\n`);
console.log(`generated/index.ts: ${names.length}개 타입`);
