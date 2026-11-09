import { Freshness, NoData, SectionTitle } from "@/components/Common";
import { EquityChart } from "@/components/EquityChart";
import { getLive } from "@/lib/data";
import { change, money, pct, price, rMultiple, when } from "@/lib/format";

const STATUS: Record<string, string> = { target: "alvo", stop: "stop", time: "prazo" };

export default async function BooksPage() {
  const live = await getLive();
  if (!live) return <NoData />;
  return (
    <div>
      <Freshness ms={live.generated_ms} label="Contas" />
      <p className="leading-relaxed">
        Cada conta começa com {money(2200)} e entra só nos candidatos (spring/UTAD) do seu gráfico. Cada trade arrisca 2% da conta, e a posição nunca passa do saldo: com stop curto, o risco fica menor que 2%.
      </p>
      {live.books.map((b) => {
        const wins = b.trades.filter((t) => t.r > 0).length;
        const points = [
          ...(b.start_ms ? [{ ms: b.start_ms, value: b.capital }] : []),
          ...b.trades.map((t) => ({ ms: t.exit_ms, value: t.equity_after })),
        ];
        return (
          <section key={b.tf} className="mt-6">
            <SectionTitle>Conta {b.label}</SectionTitle>
            <p className="num text-3xl font-black">
              {money(b.equity)} <span className={b.equity > b.capital ? "text-up" : b.equity < b.capital ? "text-down" : ""}>{pct(change(b.capital, b.equity))}</span>
            </p>
            <p className="mb-2">
              {b.trades.length} {b.trades.length === 1 ? "trade fechado" : "trades fechados"}, {wins} no lucro{b.start_ms ? `. Começou em ${when(b.start_ms)}` : ""}.
            </p>
            <EquityChart points={points} />
            {b.trades.length ? (
              <ul className="mt-3 space-y-2">
                {[...b.trades].reverse().map((t) => (
                  <li key={t.key} className="rounded border-2 border-line p-3">
                    <p className="font-black">
                      {t.asset.replace("USDT", "")} · {t.side === "long" ? "Compra" : "Venda (short)"} · saiu no {STATUS[t.status] ?? t.status}{" "}
                      <span className={t.r > 0 ? "text-up" : "text-down"}>{rMultiple(t.r)}</span>
                    </p>
                    <p className="num">
                      Entrada {price(t.entry)} em {when(t.fill_ms)} · saída {price(t.exit)} em {when(t.exit_ms)} · saldo depois {money(t.equity_after)}
                    </p>
                  </li>
                ))}
              </ul>
            ) : null}
          </section>
        );
      })}
    </div>
  );
}
