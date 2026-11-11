import Link from "next/link";
import { notFound } from "next/navigation";
import { Freshness, Pill, SectionTitle } from "@/components/Common";
import { StormerCard } from "@/components/StormerCard";
import { getMarkets } from "@/lib/data";
import { pct, price } from "@/lib/format";
import { BigQuote, LiveMarketChart, LiveQuotes, TriggerStatus } from "@/components/LiveQuotes";

const TREND_TEXT = {
  alta: "em alta: preço acima da média de 8, e a de 8 acima da de 80",
  baixa: "em baixa: preço abaixo da média de 8, e a de 8 abaixo da de 80",
  lateral: "sem tendência clara: as médias não estão alinhadas",
} as const;

export default async function MarketAssetPage({
  params,
  searchParams,
}: {
  params: Promise<{ symbol: string }>;
  searchParams: Promise<{ tf?: string }>;
}) {
  const [{ symbol }, { tf: tfParam }] = await Promise.all([params, searchParams]);
  const markets = await getMarkets();
  const asset = markets?.assets.find((a) => a.symbol === decodeURIComponent(symbol));
  if (!markets || !asset) notFound();
  const tf = tfParam === "1w" ? "1w" : tfParam === "1M" ? "1M" : "1d";
  const live = asset.stormer.filter((s) => (tf === "1M" || s.tf === tf) && (s.status === "sinal" || s.status === "aberto"));
  const lv = asset.long;
  const where52 = lv && lv.high_52w > lv.low_52w && asset.price != null ? Math.round(((asset.price - lv.low_52w) / (lv.high_52w - lv.low_52w)) * 100) : null;
  const done = asset.stormer.filter((s) => s.exit_ms != null).sort((a, b) => (b.exit_ms ?? 0) - (a.exit_ms ?? 0)).slice(0, 8);
  const extra = live.flatMap((s) => [
    { price: s.trigger, color: "entry" as const, title: s.status === "sinal" ? "Gatilho" : "Entrada" },
    { price: s.stop, color: "down" as const, title: "Stop" },
    { price: s.target, color: "up" as const, title: "Alvo" },
  ]);
  const href = (t: string) => `/mercados/${encodeURIComponent(asset.symbol)}?tf=${t}`;
  return (
    <LiveQuotes symbols={[asset.symbol]}>
      <Link href="/mercados" className="font-extrabold underline">
        ← Mercados
      </Link>
      <h1 className="mt-2 text-3xl font-black">{asset.name}</h1>
      <BigQuote symbol={asset.symbol} fallback={asset.price} fallbackChange={asset.change_day} />
      <p className="mt-1">Diário {TREND_TEXT[asset.trend]}.</p>
      <p>Semanal {TREND_TEXT[asset.weekly_trend]}.</p>
      <Freshness ms={markets.generated_ms} label="Dados" maxHours={50} />
      <div className="mb-3 flex gap-2">
        <Pill href={href("1d")} active={tf === "1d"}>
          Diário
        </Pill>
        <Pill href={href("1w")} active={tf === "1w"}>
          Semanal
        </Pill>
        <Pill href={href("1M")} active={tf === "1M"}>
          Mensal
        </Pill>
      </div>
      <LiveMarketChart
        symbol={asset.symbol}
        tf={tf}
        frame={null}
        longsZone={null}
        shortsZone={null}
        extra={extra}
        candles={tf === "1w" ? asset.weekly : tf === "1M" ? asset.monthly : asset.daily}
      />
      {lv ? (
        <section className="mt-4 rounded border-2 border-ink p-4 leading-relaxed">
          <h2 className="text-xl font-black">Visão longa ({lv.years} anos de histórico)</h2>
          <ul className="mt-2 list-disc space-y-1 pl-5">
            <li>
              Tendência longa: <b className="font-black">{lv.trend_10m ?? "sem dados"}</b>
              {lv.trend_10m ? ` (preço ${lv.trend_10m === "alta" ? "acima" : "abaixo"} da média dos últimos 10 fechamentos mensais)` : ""}.
            </li>
            <li>
              Máxima dos {lv.years} anos: {price(lv.ath)}; o preço está {lv.from_ath != null && lv.from_ath > -0.5 ? "na máxima" : `${pct(lv.from_ath, false)} abaixo dela`}.
            </li>
            <li>
              Últimas 52 semanas: entre {price(lv.low_52w)} e {price(lv.high_52w)}
              {where52 != null ? `; o preço está a ${where52}% do caminho entre a mínima e a máxima` : ""}.
            </li>
            <li>Em 12 meses: {pct(lv.change_12m)}.</li>
          </ul>
        </section>
      ) : null}
      <SectionTitle>Sinais do Stormer agora</SectionTitle>
      {live.length ? (
        <div className="grid gap-3 md:grid-cols-2">
          {live.map((s) => (
            <StormerCard key={s.key} asset={asset.name} item={s}>
              <TriggerStatus symbol={asset.symbol} status={s.status} trigger={s.trigger} stop={s.stop} target={s.target} />
            </StormerCard>
          ))}
        </div>
      ) : (
        <p className="rounded bg-panel p-3">Nenhum sinal neste gráfico agora.</p>
      )}
      {done.length ? (
        <>
          <SectionTitle>Últimos trades desses setups</SectionTitle>
          <div className="grid gap-3 md:grid-cols-2">
            {done.map((s) => (
              <StormerCard key={s.key} asset={asset.name} item={s} />
            ))}
          </div>
        </>
      ) : null}
    </LiveQuotes>
  );
}
