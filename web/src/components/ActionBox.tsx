"use client";

import Link from "next/link";
import { useQuote } from "@/components/LiveQuotes";
import { change, pct, price, when } from "@/lib/format";

export type ActionItem = {
  id: string;
  /** Stocks use the live quote; crypto comes with the last price of the panel. */
  market: "acoes" | "cripto";
  symbol: string;
  name: string;
  side: "long" | "short";
  status: "sinal" | "aberto";
  trigger: number;
  stop: number;
  target: number;
  deadline_ms: number | null;
  href: string;
  /** Crypto only: last price and today's range are not live here. */
  lastPrice: number | null;
};

type Line = { tone: "act" | "watch"; title: string; detail: string };

/** What this item asks for right now, or null when there is nothing to do. */
function read(item: ActionItem, last: number | null, high: number | null, low: number | null, now: number): Line | null {
  const buy = item.side === "long";
  const passed = (v: number | null, level: number, up: boolean) => v != null && (up ? v >= level : v <= level);
  if (item.status === "sinal") {
    if (passed(buy ? high : low, item.trigger, buy)) {
      return {
        tone: "act",
        // Crypto traps are still under observation: the title says so.
        title: `${buy ? "COMPRAR" : "VENDER (short)"}${item.market === "cripto" ? " (setup em observação)" : ""}: ${item.name} passou do gatilho ${price(item.trigger)}`,
        detail: `Preço agora ${price(last)}${last != null ? `, ${pct(change(item.trigger, last))} do gatilho` : ""}. Stop ${price(item.stop)}, alvo ${price(item.target)}.`,
      };
    }
    if (item.deadline_ms != null && item.deadline_ms - now < 24 * 3_600_000 && item.deadline_ms > now) {
      return {
        tone: "watch",
        title: `Vence hoje: ${item.name}`,
        detail: `Só ${buy ? "comprar" : "vender"} se passar de ${price(item.trigger)} até ${when(item.deadline_ms)}${last != null ? ` (falta ${pct(change(last, item.trigger), false)})` : ""}.`,
      };
    }
    return null;
  }
  if (passed(buy ? low : high, item.stop, !buy)) {
    return { tone: "act", title: `SAIR: ${item.name} bateu no stop ${price(item.stop)}`, detail: `Preço agora ${price(last)}. Se você está nesse trade, feche a posição.` };
  }
  if (passed(buy ? high : low, item.target, buy)) {
    return { tone: "act", title: `REALIZAR: ${item.name} chegou no alvo ${price(item.target)}`, detail: `Preço agora ${price(last)}. Se você está nesse trade, feche para garantir o lucro.` };
  }
  return null;
}

function Row({ item, now }: { item: ActionItem; now: number }) {
  const q = useQuote(item.market === "acoes" ? item.symbol : "");
  const last = item.market === "acoes" ? (q?.price ?? null) : item.lastPrice;
  const high = item.market === "acoes" ? (q?.day?.[2] ?? null) : item.lastPrice;
  const low = item.market === "acoes" ? (q?.day?.[3] ?? null) : item.lastPrice;
  const line = read(item, last, high, low, now);
  if (!line) return null;
  return (
    <li className={`rounded border-2 p-3 ${line.tone === "act" ? "border-ink bg-accent" : "border-ink bg-paper"}`} data-action>
      <p className="text-lg font-black leading-snug">{line.title}</p>
      <p className="mt-0.5">{line.detail}</p>
      <Link href={item.href} className="mt-1 inline-block font-black underline">
        Ver gráfico
      </Link>
    </li>
  );
}

/**
 * The top of the home page: only what asks for an action now. The
 * "nothing to do" line shows through CSS when no row rendered.
 */
export function ActionBox({ items, waiting }: { items: ActionItem[]; waiting: number }) {
  const now = Date.now();
  return (
    <section className="rounded border-2 border-ink p-4">
      <h2 className="text-xl font-black">O que fazer agora</h2>
      <ul className="action-list mt-2 space-y-2">
        {items.map((item) => (
          <Row key={item.id} item={item} now={now} />
        ))}
        <li className="action-empty rounded bg-panel p-3 font-extrabold">
          Nada a fazer agora.{waiting > 0 ? ` ${waiting} ${waiting === 1 ? "sinal está" : "sinais estão"} esperando o gatilho; o aviso chega quando algum passar.` : " Nenhum sinal aberto."}
        </li>
      </ul>
    </section>
  );
}
