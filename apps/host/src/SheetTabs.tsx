/** 여러 시트를 함께 취합할 때 그리드 아래의 시트 탭(엑셀처럼). 시트가 하나면 그리지 않는다. */
export function SheetTabs({
  names,
  active,
  onSelect,
  badges,
}: {
  names: string[];
  active: number;
  onSelect: (sheet: number) => void;
  /** 탭 옆에 보일 숫자(예: 필수 미입력 칸 수). 0이나 없으면 보이지 않는다. */
  badges?: (number | undefined)[];
}) {
  if (names.length < 2) return null;
  return (
    <div className="sheet-tabs" role="tablist" aria-label="시트">
      {names.map((name, i) => {
        const n = badges?.[i] ?? 0;
        return (
          <button key={`${i}:${name}`} role="tab" aria-selected={i === active} onClick={() => onSelect(i)}>
            <span>{name}</span>
            {n > 0 && <span className="count">{n.toLocaleString()}</span>}
          </button>
        );
      })}
    </div>
  );
}
