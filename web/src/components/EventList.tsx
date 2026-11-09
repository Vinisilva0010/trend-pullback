"use client";

import { useMemo, useState } from "react";
import type { AppEvent } from "@/lib/schema";
import { when } from "@/lib/format";

const FILTERS = [
  { key: "importantes", label: "Importantes", test: (e: AppEvent) => e.notify },
  { key: "setups", label: "Setups", test: (e: AppEvent) => e.kind === "smc" || e.kind === "stormer" },
  { key: "noticia", label: "Notícias", test: (e: AppEvent) => e.kind === "noticia" },
  { key: "mercado", label: "Mercado e níveis", test: (e: AppEvent) => !["smc", "stormer", "noticia"].includes(e.kind) },
  { key: "tudo", label: "Tudo", test: () => true },
] as const;

const KIND_LABEL: Record<string, string> = {
  smc: "Smart Money",
  stormer: "Stormer",
  noticia: "Notícia",
  mercado: "Mercado",
  nivel: "Nível",
  anomalia: "Movimento anormal",
  rompimento: "Rompimento",
};

function Linkified({ text }: { text: string }) {
  const parts = text.split(/(https:\/\/[^\s)]+)/g);
  return (
    <>
      {parts.map((part, i) =>
        part.startsWith("https://") ? (
          <a key={i} href={part} target="_blank" rel="noopener noreferrer nofollow" className="break-all underline">
            {new URL(part).hostname}
          </a>
        ) : (
          <span key={i}>{part}</span>
        ),
      )}
    </>
  );
}

export function EventList({ events }: { events: AppEvent[] }) {
  const [filter, setFilter] = useState<(typeof FILTERS)[number]["key"]>("importantes");
  const shown = useMemo(() => {
    const f = FILTERS.find((x) => x.key === filter) ?? FILTERS[0];
    return events.filter(f.test);
  }, [events, filter]);
  return (
    <div>
      <div className="mb-3 flex flex-wrap gap-2">
        {FILTERS.map((f) => (
          <button
            key={f.key}
            type="button"
            onClick={() => setFilter(f.key)}
            aria-pressed={filter === f.key}
            className={`rounded border-2 border-ink px-3 py-1.5 text-sm font-extrabold ${filter === f.key ? "bg-ink text-paper" : "bg-paper"}`}
          >
            {f.label}
          </button>
        ))}
      </div>
      {shown.length === 0 ? <p className="rounded bg-panel p-3">Nada aqui ainda.</p> : null}
      <ul className="space-y-3">
        {shown.map((e) => {
          const [title, ...rest] = e.text.split("\n");
          const candidate = e.text.includes("CANDIDATO");
          return (
            <li key={e.id} id={e.id} className={`rounded border-2 p-3 ${candidate ? "border-accent bg-accent-soft" : "border-line"}`}>
              <details open={e.notify}>
                <summary className="cursor-pointer list-none">
                  <div className="flex flex-wrap items-center gap-2 text-sm">
                    <span className="rounded bg-ink px-2 py-0.5 font-extrabold text-paper">{KIND_LABEL[e.kind] ?? e.kind}</span>
                    <span className="font-extrabold">{e.asset === "mercado" ? "Mercado" : e.asset}</span>
                    <span className="num">{when(e.ms)}</span>
                  </div>
                  <p className="mt-1 text-lg font-black leading-snug">{title}</p>
                </summary>
                <div className="mt-2 space-y-1.5 leading-relaxed">
                  {rest.map((line, i) => (
                    <p key={i} className={line.startsWith("CANDIDATO") ? "font-black" : line.startsWith("SÓ PARA") ? "font-extrabold" : ""}>
                      <Linkified text={line} />
                    </p>
                  ))}
                </div>
              </details>
            </li>
          );
        })}
      </ul>
    </div>
  );
}
