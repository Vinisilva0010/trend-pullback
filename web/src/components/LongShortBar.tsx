/** Long/short split of futures accounts as one bar with both percentages. */
export function LongShortBar({ long, before }: { long: number; before: number }) {
  const longPct = Math.round(long * 100);
  const shortPct = 100 - longPct;
  const moved = (long - before) * 100;
  const points = Math.round(moved);
  return (
    <div>
      <div className="flex justify-between text-base font-black">
        <span className="text-up">{longPct}% comprados</span>
        <span className="text-down">{shortPct}% vendidos</span>
      </div>
      <div className="mt-1 flex h-3 overflow-hidden rounded-full border-2 border-ink" aria-hidden="true">
        <div className="bg-up" style={{ width: `${longPct}%` }} />
        <div className="flex-1 bg-down" />
      </div>
      <p className="mt-1 text-sm">
        24h atrás: {Math.round(before * 100)}% comprados
        {points === 0 ? " (quase igual)" : ` (${points > 0 ? "+" : ""}${points} ${Math.abs(points) === 1 ? "ponto" : "pontos"})`}.
      </p>
    </div>
  );
}
