import { SectionTitle } from "@/components/Common";
import { CloseTradeForm, DeleteTrade, NewTradeForm } from "@/components/JournalForms";
import { ScoreCard } from "@/components/ScoreCard";
import { getLive } from "@/lib/data";
import { price, rMultiple } from "@/lib/format";
import { stopPct, tradeR, type JournalTrade } from "@/lib/journal";
import { listTrades } from "@/lib/journalStore";
import { bookRs, score, setupInfo, SETUPS, type SetupId } from "@/lib/score";

export const dynamic = "force-dynamic";

const dayFormat = new Intl.DateTimeFormat("pt-BR", { timeZone: "America/Sao_Paulo", day: "2-digit", month: "2-digit", year: "numeric" });

function day(ms: number): string {
  return dayFormat.format(new Date(ms));
}

function pctPt(v: number): string {
  return `${v.toLocaleString("pt-BR", { minimumFractionDigits: 1, maximumFractionDigits: 1 })}%`;
}

function Header({ t }: { t: JournalTrade }) {
  return (
    <>
      <p className="font-black">
        {t.symbol} · {t.side === "long" ? "Compra" : "Venda (short)"} · {setupInfo(t.setup).label}
      </p>
      <p className="num">
        Entrada {price(t.entry)} em {day(t.opened_ms)} · stop {price(t.stop)} ({pctPt(stopPct(t))} da entrada){t.target != null ? ` · alvo ${price(t.target)}` : ""}
      </p>
      {t.reason ? <p className="mt-1">Motivo: {t.reason}</p> : null}
    </>
  );
}

function count(n: number): string {
  return `${n} ${n === 1 ? "trade" : "trades"}`;
}

/** Mean R of a list, as text. */
function mean(rs: number[]): string {
  return rs.length ? rMultiple(rs.reduce((a, b) => a + b, 0) / rs.length) : "–";
}

export default async function JournalPage() {
  const [trades, live] = await Promise.all([listTrades(), getLive()]);
  const open = trades.filter((t) => t.closed_ms == null);
  const closed = trades.filter((t) => t.closed_ms != null);
  const books = bookRs(live?.books ?? []);
  // Closed trades per setup, oldest exit first.
  const bySetup = new Map<SetupId, JournalTrade[]>();
  for (const t of [...closed].sort((a, b) => (a.closed_ms ?? 0) - (b.closed_ms ?? 0))) {
    bySetup.set(t.setup, [...(bySetup.get(t.setup) ?? []), t]);
  }
  const rsOf = (list: JournalTrade[]) => list.map(tradeR).filter((r): r is number => r != null);
  const followed = rsOf(closed.filter((t) => t.followed === true));
  const broke = rsOf(closed.filter((t) => t.followed === false));

  return (
    <div>
      <h1 className="text-2xl font-black">Meus trades</h1>
      <p className="mt-2 leading-relaxed">
        Os trades que você fez de verdade, digitados aqui. Fica guardado só ativo, preços, datas e as suas notas: nada de corretora, saldo ou senha. O resultado é medido em R (1R = o que se perde no stop), o mesmo jeito do teste e das contas fictícias, então dá para comparar mesmo com posições de tamanhos diferentes.
      </p>

      <SectionTitle>Registrar um trade</SectionTitle>
      <NewTradeForm />

      <SectionTitle>Abertos ({open.length})</SectionTitle>
      {open.length ? (
        <ul className="space-y-3">
          {open.map((t) => (
            <li key={t.id} className="rounded border-2 border-ink p-3">
              <Header t={t} />
              <CloseTradeForm id={t.id} />
            </li>
          ))}
        </ul>
      ) : (
        <p>Nenhum trade aberto.</p>
      )}

      <SectionTitle>Resultado por setup</SectionTitle>
      {closed.length ? (
        <>
          {SETUPS.filter((s) => bySetup.has(s.id)).map((s) => {
            const mine = rsOf(bySetup.get(s.id) ?? []);
            const book = books.get(s.id) ?? [];
            const bookLine = book.length ? ` Conta fictícia no mesmo setup: ${count(book.length)}, média de ${mean(book)} por trade.` : "";
            return <ScoreCard key={s.id} title={s.label} score={score(mine)} reference={s.reference} note={`${s.note}${bookLine}`} />;
          })}
          {followed.length && broke.length ? (
            <p className="num mt-4 rounded border-2 border-ink bg-accent-soft p-3 font-extrabold">
              Quando seguiu o plano: {count(followed.length)}, média de {mean(followed)}. Quando não seguiu: {count(broke.length)}, média de {mean(broke)}.
            </p>
          ) : null}
        </>
      ) : (
        <p>Nenhum trade fechado ainda. O placar aparece quando você fechar o primeiro.</p>
      )}

      <SectionTitle>Fechados ({closed.length})</SectionTitle>
      {closed.length ? (
        <ul className="space-y-3">
          {closed.map((t) => {
            const r = tradeR(t);
            return (
              <li key={t.id} className="rounded border-2 border-ink p-3">
                <Header t={t} />
                <p className="num mt-1 font-extrabold">
                  Saída {price(t.exit)} em {t.closed_ms != null ? day(t.closed_ms) : "–"} ·{" "}
                  <span className={r != null && r > 0 ? "text-up" : r != null && r < 0 ? "text-down" : ""}>{rMultiple(r)}</span> ·{" "}
                  {t.followed ? "seguiu o plano" : "fora do plano"}
                </p>
                {t.exit_note ? <p className="mt-1">Nota: {t.exit_note}</p> : null}
                <DeleteTrade id={t.id} />
              </li>
            );
          })}
        </ul>
      ) : null}
    </div>
  );
}
