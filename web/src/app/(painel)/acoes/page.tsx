import Link from "next/link";
import { Freshness, SectionTitle } from "@/components/Common";
import { LiveQuotes, TriggerStatus } from "@/components/LiveQuotes";
import { StormerCard } from "@/components/StormerCard";
import { getLive, getMarkets } from "@/lib/data";
import { change, money, pct } from "@/lib/format";

export default async function StocksPage() {
  const [markets, live] = await Promise.all([getMarkets(), getLive()]);
  if (!markets) {
    return (
      <div className="rounded border-2 border-ink bg-accent-soft p-4">
        <p className="text-lg font-black">As ações ainda não chegaram.</p>
        <p className="mt-1">Elas são atualizadas todo dia depois das 21h15.</p>
      </div>
    );
  }
  const us = markets.assets.filter((a) => a.group === "EUA");
  const rows = us.flatMap((a) => a.stormer.filter((s) => s.role === "candidato").map((s) => ({ a, s })));
  // Daily signals first: the paper book takes them first.
  const signals = rows.filter((r) => r.s.status === "sinal").sort((x, y) => Number(x.s.tf !== "1d") - Number(y.s.tf !== "1d"));
  const open = rows.filter((r) => r.s.status === "aberto");
  const closed = rows
    .filter((r) => r.s.exit_ms != null)
    .sort((x, y) => (y.s.exit_ms ?? 0) - (x.s.exit_ms ?? 0))
    .slice(0, 10);
  const book = live?.books.find((b) => b.tf === "acoes");
  const symbols = [...new Set([...signals, ...open].map((r) => r.a.symbol))];

  const card = ({ a, s }: (typeof rows)[number]) => (
    <StormerCard key={s.key} asset={a.name} item={s}>
      <TriggerStatus symbol={a.symbol} status={s.status} trigger={s.trigger} stop={s.stop} target={s.target} />
      <Link href={`/mercados/${encodeURIComponent(a.symbol)}?tf=${s.tf}`} className="mt-2 inline-block font-black underline">
        Ver gráfico
      </Link>
    </StormerCard>
  );

  return (
    <LiveQuotes symbols={symbols}>
      <Freshness ms={markets.generated_ms} label="Ações" maxHours={50} />
      <section className="rounded border-2 border-ink bg-accent-soft p-4 leading-relaxed">
        <p className="text-lg font-black">Como usar</p>
        <p className="mt-1">
          Todos os sinais aqui são o mesmo setup: o preço passou abaixo de um fundo e voltou para cima. No teste de 10 anos em 84 ações e ETFs dos EUA, ele ganhou dinheiro e foi melhor que comprar em qualquer dia com o mesmo stop.
        </p>
        <p className="mt-1">
          Um sinal não é compra agora. Ele só vale se o preço passar do gatilho até o prazo. Durante o pregão, o Telegram e o celular avisam quando isso acontece e dizem quantas ações comprar. Depois avisam quando bater o stop (vender) ou o alvo (realizar).
        </p>
        <p className="mt-1">A conta fictícia entra em um trade por dia, no primeiro que passar do gatilho (o diário tem preferência), com até três abertos ao mesmo tempo.</p>
      </section>

      {book ? (
        <p className="mt-4 rounded border-2 border-ink p-3">
          <span className="font-extrabold">{book.label}: </span>
          <span className="num text-xl font-black">{money(book.equity)}</span>{" "}
          <span className={book.equity >= book.capital ? "text-up" : "text-down"}>{pct(change(book.capital, book.equity))}</span>
          <span> · {book.trades.length} {book.trades.length === 1 ? "trade fechado" : "trades fechados"}. </span>
          <Link href="/contas" className="font-black underline">
            Ver conta
          </Link>
        </p>
      ) : null}

      <SectionTitle>Esperando o gatilho ({signals.length})</SectionTitle>
      {signals.length ? <div className="grid gap-3 md:grid-cols-2">{signals.map(card)}</div> : <p className="rounded bg-panel p-3">Nenhum sinal agora. Os sinais saem depois do fechamento de Nova York, na rodada das 21h15.</p>}

      <SectionTitle>Trades abertos ({open.length})</SectionTitle>
      {open.length ? <div className="grid gap-3 md:grid-cols-2">{open.map(card)}</div> : <p className="rounded bg-panel p-3">Nenhum trade aberto.</p>}

      <SectionTitle>Fechados recentes</SectionTitle>
      {closed.length ? <div className="grid gap-3 md:grid-cols-2">{closed.map(card)}</div> : <p className="rounded bg-panel p-3">Nenhum trade fechado ainda.</p>}
    </LiveQuotes>
  );
}
