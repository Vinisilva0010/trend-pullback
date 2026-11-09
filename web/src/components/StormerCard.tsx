import type { StormerItem } from "@/lib/schema";
import { change, pct, price, rMultiple, when } from "@/lib/format";

const STATUS: Record<StormerItem["status"], string> = {
  sinal: "Sinal: esperando o rompimento",
  aberto: "Trade aberto",
  alvo: "Saiu no alvo",
  stop: "Saiu no stop",
  prazo: "Saiu pelo prazo",
};

/** One Stormer signal or trade in plain words. */
export function StormerCard({ asset, item }: { asset: string; item: StormerItem }) {
  const long = item.side === "long";
  const candidate = item.role === "candidato";
  const closed = item.status === "alvo" || item.status === "stop" || item.status === "prazo";
  return (
    <article className={`rounded border-2 p-3 ${item.role === "acompanhar" ? "border-line" : "border-accent"}`}>
      <div className="flex flex-wrap items-center gap-2">
        <span className={`rounded px-2 py-0.5 text-sm font-black text-white ${long ? "bg-up" : "bg-down"}`}>{long ? "Compra" : "Venda (short)"}</span>
        <span className="text-lg font-black">{asset}</span>
        <span className="font-bold">
          Stormer {item.pattern} · {item.tf === "1w" ? "semanal" : "diário"}
        </span>
        {candidate ? (
          <span className="rounded bg-accent px-2 py-0.5 text-sm font-black">Candidato (ganhou da base)</span>
        ) : item.role === "observacao" ? (
          <span className="rounded border-2 border-accent px-2 py-0.5 text-sm font-black">Em observação (acima da base, sem prova)</span>
        ) : (
          <span className="rounded bg-panel px-2 py-0.5 text-sm font-extrabold">Só acompanhar</span>
        )}
      </div>
      <p className="mt-2 text-lg font-black">
        {STATUS[item.status]}
        {closed && item.r != null ? <span className={item.r > 0 ? "text-up" : "text-down"}> {rMultiple(item.r)}</span> : null}
      </p>
      {item.status === "sinal" ? (
        <p className="mt-1 font-extrabold">
          {long ? "Comprar se passar de" : "Vender se cair abaixo de"} {price(item.trigger)} até {when(item.deadline_ms)}. Se não acontecer, o sinal vence.
        </p>
      ) : (
        <p className="mt-1">
          Entrada {price(item.trigger)} em {when(item.entry_ms)}
          {item.exit != null ? ` · saída ${price(item.exit)} em ${when(item.exit_ms)}` : ""}.
        </p>
      )}
      <p className="num mt-1">
        Stop {price(item.stop)} ({pct(change(item.trigger, item.stop))}) · Alvo {price(item.target)} ({pct(change(item.trigger, item.target))}), 2 vezes o risco.
      </p>
    </article>
  );
}
