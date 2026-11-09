import Link from "next/link";
import { Freshness, SectionTitle } from "@/components/Common";
import { getMarkets } from "@/lib/data";
import { pct, price } from "@/lib/format";
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
  return (
    <div>
      <Freshness ms={markets.generated_ms} label="Mercados" maxHours={50} />
      <p className="leading-relaxed">
        Preço do último fechamento (as ações têm cerca de 15 minutos de atraso na fonte). Tendência pelas médias de 8 e 80 dias, como no 123 do Stormer. Candidatos são os setups de compra do Stormer que ganharam nos dois períodos do teste com 10 anos de histórico: no semanal, o 123 e o trap (Brasil e EUA); no diário, o 123 (EUA e Injective). Vendas perderam em todos os grupos.
      </p>
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
                      <span className="num text-lg font-black">{price(a.price)}</span>
                      <Change v={a.change_day} label="dia" />
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
    </div>
  );
}
