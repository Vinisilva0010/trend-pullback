import Link from "next/link";
import { notFound } from "next/navigation";
import { CandleChart } from "@/components/CandleChart";
import { Freshness, Pill, SectionTitle } from "@/components/Common";
import { StormerCard } from "@/components/StormerCard";
import { getMarkets } from "@/lib/data";
import { pct, price } from "@/lib/format";

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
  const tf = tfParam === "1w" ? "1w" : "1d";
  const live = asset.stormer.filter((s) => s.tf === tf && (s.status === "sinal" || s.status === "aberto"));
  const done = asset.stormer.filter((s) => s.exit_ms != null).sort((a, b) => (b.exit_ms ?? 0) - (a.exit_ms ?? 0)).slice(0, 8);
  const extra = live.flatMap((s) => [
    { price: s.trigger, color: "entry" as const, title: s.status === "sinal" ? "Gatilho" : "Entrada" },
    { price: s.stop, color: "down" as const, title: "Stop" },
    { price: s.target, color: "up" as const, title: "Alvo" },
  ]);
  const href = (t: string) => `/mercados/${encodeURIComponent(asset.symbol)}?tf=${t}`;
  return (
    <div>
      <Link href="/mercados" className="font-extrabold underline">
        ← Mercados
      </Link>
      <h1 className="mt-2 text-3xl font-black">{asset.name}</h1>
      <p className="num text-2xl font-black">
        {price(asset.price)} <span className={(asset.change_day ?? 0) >= 0 ? "text-up" : "text-down"}>{pct(asset.change_day)}</span>{" "}
        <span className="text-base font-bold">no dia</span>
      </p>
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
      </div>
      <CandleChart
        symbol={asset.symbol}
        tf={tf}
        frame={null}
        longsZone={null}
        shortsZone={null}
        extra={extra}
        candles={tf === "1w" ? asset.weekly : asset.daily}
      />
      <SectionTitle>Sinais do Stormer agora</SectionTitle>
      {live.length ? (
        <div className="grid gap-3 md:grid-cols-2">
          {live.map((s) => (
            <StormerCard key={s.key} asset={asset.name} item={s} />
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
    </div>
  );
}
