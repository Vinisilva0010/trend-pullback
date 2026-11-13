"use client";

import { useMemo, useState } from "react";
import type { AppEvent } from "@/lib/schema";
import { LEAN_BADGE } from "@/components/TopNews";

const TYPES = [
  { key: "importantes", label: "Importantes", test: (e: AppEvent) => e.level >= 3 },
  { key: "trades", label: "Trades", test: (e: AppEvent) => e.kind === "smc" || e.kind === "stormer" },
  { key: "ipo", label: "IPOs", test: (e: AppEvent) => e.kind === "ipo" },
  { key: "noticia", label: "Notícias", test: (e: AppEvent) => e.kind === "noticia" },
  { key: "mercado", label: "Níveis e mercado", test: (e: AppEvent) => !["smc", "stormer", "noticia", "ipo"].includes(e.kind) },
  { key: "tudo", label: "Tudo", test: () => true },
] as const;

const PLACES = [
  { key: "todos", label: "Todos", test: () => true },
  { key: "BTC", label: "BTC", test: (e: AppEvent) => e.asset === "BTC" },
  { key: "ETH", label: "ETH", test: (e: AppEvent) => e.asset === "ETH" },
  { key: "SOL", label: "SOL", test: (e: AppEvent) => e.asset === "SOL" },
  { key: "cripto", label: "Cripto geral", test: (e: AppEvent) => e.market === "cripto" && e.asset === "mercado" },
  { key: "acoes", label: "Ações", test: (e: AppEvent) => e.market === "acoes" },
] as const;

const KIND_LABEL: Record<string, string> = {
  smc: "Smart Money",
  stormer: "Stormer",
  ipo: "IPO",
  noticia: "Notícia",
  mercado: "Mercado",
  nivel: "Nível",
  anomalia: "Movimento anormal",
  rompimento: "Rompimento",
};

const KIND_STYLE: Record<string, string> = {
  stormer: "bg-accent text-ink",
  ipo: "bg-accent text-ink",
  smc: "bg-panel text-ink border-2 border-ink",
  noticia: "bg-ink text-paper",
};

const dayFormat = new Intl.DateTimeFormat("pt-BR", { timeZone: "America/Sao_Paulo", weekday: "long", day: "2-digit", month: "2-digit" });
const hourFormat = new Intl.DateTimeFormat("pt-BR", { timeZone: "America/Sao_Paulo", hour: "2-digit", minute: "2-digit" });
const dayKey = (ms: number) => new Intl.DateTimeFormat("en-CA", { timeZone: "America/Sao_Paulo" }).format(new Date(ms));

function dayTitle(ms: number, now: number): string {
  const k = dayKey(ms);
  if (k === dayKey(now)) return "Hoje";
  if (k === dayKey(now - 86_400_000)) return "Ontem";
  return dayFormat.format(new Date(ms));
}

function Linkified({ text }: { text: string }) {
  const parts = text.split(/(https?:\/\/[^\s)]+)/g);
  return (
    <>
      {parts.map((part, i) => {
        if (!/^https?:\/\//.test(part)) return <span key={i}>{part}</span>;
        let host = part;
        try {
          host = new URL(part).hostname.replace(/^www\./, "");
        } catch {
          return <span key={i}>{part}</span>;
        }
        return (
          <a key={i} href={part} target="_blank" rel="noopener noreferrer nofollow" className="break-all underline">
            {host}
          </a>
        );
      })}
    </>
  );
}

/** Title without the "NOTÍCIA:" prefix (the badge already says it). */
function shortTitle(e: AppEvent): string {
  const title = e.text.split("\n")[0] ?? "";
  return e.kind === "noticia" ? title.replace(/^NOTÍCIA:\s*/, "") : title;
}

function Row({ e }: { e: AppEvent }) {
  const rest = e.text.split("\n").slice(1);
  const lean = e.lean ? LEAN_BADGE[e.lean] : null;
  const asset = e.asset === "mercado" ? (e.market === "acoes" ? "Ações EUA" : "Cripto geral") : e.asset;
  return (
    <li id={e.id} className={`rounded border-2 ${e.level >= 3 ? "border-ink" : "border-line"}`}>
      <details>
        <summary className="cursor-pointer list-none p-3">
          <div className="flex flex-wrap items-center gap-2 text-sm">
            <span className="num font-black">{hourFormat.format(new Date(e.ms))}</span>
            <span className={`rounded px-2 py-0.5 font-extrabold ${KIND_STYLE[e.kind] ?? "bg-panel text-ink"}`}>{KIND_LABEL[e.kind] ?? e.kind}</span>
            <span className="font-black">{asset}</span>
            {lean ? <span className={`rounded border-2 px-2 py-0.5 font-black ${lean.cls}`}>{lean.text}</span> : null}
            {e.level >= 3 ? <span className="font-black">· importante</span> : null}
            {!e.notify ? <span className="font-bold">· só no painel</span> : null}
          </div>
          <p className="mt-1 line-clamp-2 text-[17px] font-black leading-snug">{shortTitle(e)}</p>
          <p className="mt-0.5 text-sm font-bold underline">Ver detalhes</p>
        </summary>
        <div className="space-y-1.5 border-t-2 border-line p-3 leading-relaxed">
          {rest.map((line, i) => (
            <p key={i} className={line.startsWith("CANDIDATO") ? "font-black" : line.startsWith("SÓ PARA") ? "font-extrabold" : ""}>
              <Linkified text={line} />
            </p>
          ))}
        </div>
      </details>
    </li>
  );
}

function Chips<T extends string>({ options, value, onChange }: { options: readonly { key: T; label: string }[]; value: T; onChange: (k: T) => void }) {
  return (
    <div className="flex flex-wrap gap-2">
      {options.map((o) => (
        <button
          key={o.key}
          type="button"
          onClick={() => onChange(o.key)}
          aria-pressed={value === o.key}
          className={`rounded border-2 border-ink px-3 py-1.5 text-sm font-extrabold ${value === o.key ? "bg-ink text-paper" : "bg-paper"}`}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}

const PAGE = 60;

export function EventList({ events }: { events: AppEvent[] }) {
  const [type, setType] = useState<(typeof TYPES)[number]["key"]>("importantes");
  const [place, setPlace] = useState<(typeof PLACES)[number]["key"]>("todos");
  const [limit, setLimit] = useState(PAGE);
  const shown = useMemo(() => {
    const t = TYPES.find((x) => x.key === type) ?? TYPES[0];
    const p = PLACES.find((x) => x.key === place) ?? PLACES[0];
    return [...events].sort((a, b) => b.ms - a.ms).filter((e) => t.test(e) && p.test(e));
  }, [events, type, place]);
  const days = useMemo(() => {
    const now = Date.now();
    const out: { title: string; items: AppEvent[] }[] = [];
    for (const e of shown.slice(0, limit)) {
      const title = dayTitle(e.ms, now);
      const last = out.at(-1);
      if (last && last.title === title) last.items.push(e);
      else out.push({ title, items: [e] });
    }
    return out;
  }, [shown, limit]);
  return (
    <div>
      <p className="mb-1 text-sm font-extrabold">Tipo</p>
      <Chips options={TYPES} value={type} onChange={(k) => { setType(k); setLimit(PAGE); }} />
      <p className="mb-1 mt-3 text-sm font-extrabold">Ativo</p>
      <Chips options={PLACES} value={place} onChange={(k) => { setPlace(k); setLimit(PAGE); }} />
      {type === "importantes" ? (
        <p className="mt-3 text-sm">Importantes: avisos de trade (sinal, entrada, saída, gatilho, stop e alvo), IPOs grandes e notícias que saíram em duas fontes ou mais.</p>
      ) : null}
      {shown.length === 0 ? <p className="mt-4 rounded bg-panel p-3">Nada aqui ainda.</p> : null}
      {days.map((d) => (
        <section key={d.title} className="mt-5">
          <h3 className="mb-2 border-b-2 border-ink pb-1 text-lg font-black capitalize">
            {d.title} <span className="text-sm font-bold normal-case">({d.items.length} {d.items.length === 1 ? "aviso" : "avisos"})</span>
          </h3>
          <ul className="space-y-2">
            {d.items.map((e) => (
              <Row key={e.id} e={e} />
            ))}
          </ul>
        </section>
      ))}
      {shown.length > limit ? (
        <button type="button" onClick={() => setLimit((l) => l + PAGE)} className="mt-4 rounded border-2 border-ink px-4 py-2 font-black">
          Mostrar mais
        </button>
      ) : null}
    </div>
  );
}
