import type { MoodItem } from "@/lib/schema";
import { when } from "@/lib/format";

export const LEAN_BADGE: Record<string, { text: string; cls: string }> = {
  alta: { text: "↑ Puxa para alta", cls: "bg-up-soft text-up border-up" },
  queda: { text: "↓ Puxa para queda", cls: "bg-down-soft text-down border-down" },
  incerto: { text: "Direção incerta", cls: "bg-panel text-ink border-line" },
};

/** The most important headlines of the last day, both markets. */
export function TopNews({ items }: { items: (MoodItem & { market: string })[] }) {
  if (items.length === 0) return <p className="rounded bg-panel p-3">Nenhuma notícia importante nas últimas 24 horas.</p>;
  return (
    <ul className="divide-y-2 divide-line rounded border-2 border-ink">
      {items.map((n) => {
        const lean = LEAN_BADGE[n.lean] ?? LEAN_BADGE.incerto!;
        return (
          <li key={n.link} className="p-3">
            <div className="flex flex-wrap items-center gap-2 text-sm">
              <span className={`rounded border-2 px-2 py-0.5 font-black ${lean.cls}`}>{lean.text}</span>
              <span className="rounded bg-ink px-2 py-0.5 font-extrabold text-paper">{n.market === "acoes" ? "Ações EUA" : "Cripto"}</span>
              <span className="font-extrabold">{n.kind}</span>
              {n.level === 3 ? <span className="rounded bg-accent px-2 py-0.5 font-black">Várias fontes</span> : null}
            </div>
            <a href={n.link} target="_blank" rel="noopener noreferrer nofollow" className="mt-1 block text-lg font-black leading-snug underline-offset-2 hover:underline">
              {n.title}
            </a>
            <p className="num text-sm font-bold">
              {n.source} · {when(n.ms)}
            </p>
          </li>
        );
      })}
    </ul>
  );
}
