import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

import { PERSON_COLORS } from "./flags";

// tokens.css의 색이 글자 4.5:1, 테두리·표시 3:1(WCAG 2.x 대비)을 넘는지 확인한다.

const css = readFileSync(new URL("./tokens.css", import.meta.url), "utf8");
const raw = new Map([...css.matchAll(/(--[\w-]+):\s*([^;]+);/g)].map((m) => [m[1]!, m[2]!.trim()]));

function token(name: string): string {
  let v = raw.get(name);
  for (let i = 0; v?.startsWith("var("); i++) {
    if (i > 5) throw new Error(`${name}: var() 고리`);
    v = raw.get(v.slice(4, -1).trim());
  }
  if (!v) throw new Error(`${name} 없음`);
  return v;
}

const color = (v: string) => (v.startsWith("--") ? token(v) : v);

function luminance(hex: string): number {
  const h = hex.replace("#", "");
  const full = h.length === 3 ? [...h].map((c) => c + c).join("") : h;
  const [r, g, b] = [0, 2, 4].map((i) => {
    const c = parseInt(full.slice(i, i + 2), 16) / 255;
    return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r! + 0.7152 * g! + 0.0722 * b!;
}

function contrast(a: string, b: string): number {
  const [x, y] = [luminance(color(a)), luminance(color(b))];
  return (Math.max(x, y) + 0.05) / (Math.min(x, y) + 0.05);
}

describe("색 토큰 대비", () => {
  const text: [string, string][] = [
    ["--ink", "#fff"],
    ["--ink-2", "#fff"],
    ["--muted", "#fff"],
    ["--muted", "--paper"],
    ["--muted", "--head"],
    ["--muted", "--hover"],
    ["--accent", "#fff"],
    ["#fff", "--accent"],
    ["#fff", "--accent-hover"],
    ["--accent-ink", "--accent-tint"],
    ["--accent-ink", "--accent-soft"],
    ["--ok", "#fff"],
    ["--ok-ink", "--ok-soft"],
    ["--warn", "#fff"],
    ["--warn-ink", "--warn-soft"],
    ["--danger", "#fff"],
    ["--danger", "--danger-tint"],
    ["--danger", "--danger-soft"],
    ["--grid-head-ink", "--grid-head-bg"],
    ["--others-ink", "--others-bg"],
    ["--missing-ink", "--missing-bg"],
    ["--hl-ink", "--hl-bg"],
    ["--hl-ink", "--hl-soft"],
  ];
  it.each(text)("글자 %s on %s ≥ 4.5", (fg, bg) => {
    expect(contrast(fg, bg)).toBeGreaterThanOrEqual(4.5);
  });

  const marks: [string, string][] = [
    ["--field", "#fff"],
    ["--grid-edge", "#fff"],
    ["--focus", "#fff"],
    ["--missing-mark", "--missing-bg"],
    ["--hl-line", "--hl-bg"],
  ];
  it.each(marks)("표시 %s on %s ≥ 3", (fg, bg) => {
    expect(contrast(fg, bg)).toBeGreaterThanOrEqual(3);
  });

  it.each([...PERSON_COLORS])("이름표 흰 글자 on %s ≥ 4.5", (c) => {
    expect(contrast("#fff", c)).toBeGreaterThanOrEqual(4.5);
  });
});
