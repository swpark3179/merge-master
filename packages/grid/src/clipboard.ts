/**
 * Excel 클립보드 형식(TSV). Excel은 탭·줄바꿈·큰따옴표가 든 칸을 큰따옴표로 감싸고 안의 큰따옴표는 두 번 쓴다.
 * 마지막 줄 끝에는 줄바꿈이 붙는다.
 */

/** TSV → 행×열 글자. 끝의 빈 줄 하나는 버린다. */
export function parseTsv(text: string): string[][] {
  const rows: string[][] = [];
  let row: string[] = [];
  let cell = "";
  let quoted = false;
  let i = 0;
  const s = text.replace(/\r\n?/g, "\n");
  while (i < s.length) {
    const ch = s[i]!;
    if (quoted) {
      if (ch === '"' && s[i + 1] === '"') {
        cell += '"';
        i += 2;
        continue;
      }
      if (ch === '"') {
        quoted = false;
        i++;
        continue;
      }
      cell += ch;
      i++;
      continue;
    }
    if (ch === '"' && cell === "") {
      quoted = true;
      i++;
    } else if (ch === "\t") {
      row.push(cell);
      cell = "";
      i++;
    } else if (ch === "\n") {
      row.push(cell);
      rows.push(row);
      row = [];
      cell = "";
      i++;
    } else {
      cell += ch;
      i++;
    }
  }
  if (cell !== "" || row.length > 0) {
    row.push(cell);
    rows.push(row);
  }
  return rows;
}

function quote(cell: string): string {
  return /[\t\n\r"]/.test(cell) ? `"${cell.replace(/"/g, '""')}"` : cell;
}

/** 행×열 글자 → TSV(Excel과 같이 줄마다 CRLF). */
export function toTsv(rows: string[][]): string {
  return rows.map((r) => r.map(quote).join("\t")).join("\r\n") + "\r\n";
}
