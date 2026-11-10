import Link from "next/link";
import { Freshness, SectionTitle } from "@/components/Common";
import { getMarkets } from "@/lib/data";
import { LiveQuotes, RowQuote, TriggerStatus } from "@/components/LiveQuotes";
import { StormerCard } from "@/components/StormerCard";
import { pct } from "@/lib/format";
import type { MarketAsset } from "@/lib/schema";

const TREND_STYLE: Record<MarketAsset["trend"], string> = {
  alta: "bg-up-soft text-up",
  baixa: "bg-down-soft text-down",
  lateral: "bg-panel text-ink",
};

function Change({ v, label }: { v: number | null; label: string }) {
  const color = v == null ? "" : v > 0 ? "text-up" : v < 0 ? "text-down" : "";
  return (
    <span className={`num font-extrabold ${color}`}>
      {pct(v)} <span className="font-bold text-ink">{label}</span>
    </span>
  );
}

export default async function MarketsPage() {
  const markets = await getMarkets();
  if (!markets) {
    return (
      <div className="rounded border-2 border-ink bg-accent-soft p-4">
        <p className="text-lg font-black">A aba Mercados ainda não recebeu dados.</p>
        <p className="mt-1">Ela é atualizada todo dia depois das 21h15, ou quando você roda o comando de mercados da ferramenta.</p>
      </div>
    );
  }
  const groups = [...new Set(markets.assets.map((a) => a.group))];
  const usNow = markets.assets
    .filter((a) => a.group === "EUA")
    .flatMap((a) => a.stormer.filter((s) => s.role === "candidato" && (s.status === "sinal" || s.status === "aberto")).map((s) => ({ a, s })));
  return (
    <LiveQuotes symbols={markets.assets.map((a) => a.symbol)}>
      <Freshness ms={markets.generated_ms} label="Mercados" maxHours={50} />
      <p className="leading-relaxed">
        Preço atualizado a cada minuto enquanto a página está aberta, do Yahoo Finance; ao lado de cada preço aparece se está ao vivo, com atraso ou com o mercado fechado. Tendência pelas médias de 8 e 80 dias, como no 123 do Stormer. Candidatos são os setups de compra do Stormer que ganharam nos dois períodos do teste com 10 anos de histórico e também ganharam de comprar em qualquer dia com o mesmo stop. Nas 84 ações e ETFs dos EUA testados: o trap diário a favor da tendência e o trap semanal. Esses geram aviso no Telegram e no celular e entram na conta Ações EUA (trap); as ações dos EUA fora desta lista aparecem aqui enquanto tiverem sinal ou trade aberto. Nas 62 ações do Brasil nada ganhou da base. Em cripto, o 123 diário (Injective). Vendas perderam em todos os grupos.
      </p>
      <SectionTitle>Ações dos EUA com sinal ou trade aberto</SectionTitle>
      {usNow.length === 0 ? (
        <p className="rounded bg-panel p-3">Nenhum agora. Os sinais saem depois do fechamento de Nova York, na rodada das 21h15.</p>
      ) : (
        <div className="grid gap-3 md:grid-cols-2">
          {usNow.map(({ a, s }) => (
            <StormerCard key={s.key} asset={a.name} item={s}>
              <TriggerStatus symbol={a.symbol} status={s.status} trigger={s.trigger} stop={s.stop} target={s.target} />
              <Link href={`/mercados/${encodeURIComponent(a.symbol)}?tf=${s.tf}`} className="mt-2 inline-block font-black underline">
                Ver gráfico
              </Link>
            </StormerCard>
          ))}
        </div>
      )}
      {groups.map((g) => (
        <section key={g}>
          <SectionTitle>{g}</SectionTitle>
          <ul className="divide-y-2 divide-line rounded border-2 border-ink">
            {markets.assets
              .filter((a) => a.group === g)
              .map((a) => {
                const signals = a.stormer.filter((s) => s.status === "sinal" || s.status === "aberto").length;
                return (
                  <li key={a.symbol}>
                    <Link href={`/mercados/${encodeURIComponent(a.symbol)}`} className="flex flex-wrap items-center gap-x-4 gap-y-1 px-3 py-3 hover:bg-panel">
                      <span className="min-w-[9rem] text-lg font-black">{a.name}</span>
                      <RowQuote symbol={a.symbol} fallback={a.price} fallbackChange={a.change_day} />
                      <Change v={a.change_week} label="semana" />
                      <span className={`rounded px-2 py-0.5 text-sm font-black ${TREND_STYLE[a.trend]}`}>{a.trend}</span>
                      {signals ? <span className="rounded bg-accent px-2 py-0.5 text-sm font-black">{signals} sinal{signals > 1 ? "is" : ""}</span> : null}
                    </Link>
                  </li>
                );
              })}
          </ul>
        </section>
      ))}
    </LiveQuotes>
  );
}
