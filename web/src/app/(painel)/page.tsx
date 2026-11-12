import Link from "next/link";
import { Freshness, NoData, SectionTitle } from "@/components/Common";
import { ActionBox, type ActionItem } from "@/components/ActionBox";
import { LivePrice } from "@/components/LivePrices";
import { LiveQuotes } from "@/components/LiveQuotes";
import { LongShortBar } from "@/components/LongShortBar";
import { LogoutButton } from "@/components/LogoutButton";
import { Thermometer } from "@/components/Thermometer";
import { TopNews } from "@/components/TopNews";
import { cryptoBrief, stocksBrief, type Brief } from "@/lib/brief";
import { getDaily, getLive, getMarkets } from "@/lib/data";
import { change, pct, price, usdShort } from "@/lib/format";
import { dayReading } from "@/lib/view";

const TONE: Record<Brief["tone"], string> = {
  up: "bg-up text-white",
  down: "bg-down text-white",
  mixed: "bg-accent text-ink",
  flat: "bg-panel text-ink",
};

function BriefCard({ title, brief, link }: { title: string; brief: Brief; link: { href: string; label: string } }) {
  return (
    <section className="rounded border-2 border-ink p-4">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h3 className="text-xl font-black">{title}</h3>
        <span className={`rounded px-3 py-1 text-xl font-black ${TONE[brief.tone]}`}>{brief.label}</span>
      </div>
      <ul className="mt-2 list-disc space-y-1.5 pl-5 leading-snug">
        {brief.lines.map((l, i) => (
          <li key={i}>{l}</li>
        ))}
      </ul>
      <Link href={link.href} className="mt-3 inline-block font-black underline">
        {link.label}
      </Link>
    </section>
  );
}

export default async function Home() {
  const [live, daily, markets] = await Promise.all([getLive(), getDaily(), getMarkets()]);
  if (!live && !daily) return <NoData />;
  const now = Date.now();
  const reading = daily ? dayReading(daily.sections) : null;
  const cryptoTrends = daily?.assets ?? [];
  const cryptoChanges = (live?.assets ?? []).map((a) => a.change_24h).filter((v): v is number => v != null);
  const us = (markets?.assets ?? []).filter((a) => a.group === "EUA");
  const spy = us.find((a) => a.symbol === "SPY");
  const openStock = us.reduce((n, a) => n + a.stormer.filter((s) => s.role === "candidato" && (s.status === "sinal" || s.status === "aberto")).length, 0);
  // Everything that can ask for an action: stock and crypto signals and open trades.
  const actions: ActionItem[] = [
    ...us.flatMap((a) =>
      a.stormer
        .filter((s) => s.role === "candidato" && (s.status === "sinal" || s.status === "aberto"))
        .map((s): ActionItem => ({
          id: s.key, market: "acoes", symbol: a.symbol, name: a.name, side: s.side, status: s.status as "sinal" | "aberto",
          trigger: s.trigger, stop: s.stop, target: s.target, deadline_ms: s.status === "sinal" ? s.deadline_ms : null,
          href: `/mercados/${encodeURIComponent(a.symbol)}?tf=${s.tf}`, lastPrice: null,
        })),
    ),
    ...(live?.assets ?? []).flatMap((a) =>
      a.stormer
        .filter((s) => s.role !== "acompanhar" && (s.status === "sinal" || s.status === "aberto"))
        .map((s): ActionItem => ({
          id: s.key, market: "cripto", symbol: a.name, name: a.name, side: s.side, status: s.status as "sinal" | "aberto",
          trigger: s.trigger, stop: s.stop, target: s.target, deadline_ms: s.status === "sinal" ? s.deadline_ms : null,
          href: `/grafico?ativo=${a.name}&tf=${s.tf}`, lastPrice: a.price,
        })),
    ),
  ];
  const waiting = actions.filter((a) => a.status === "sinal").length;
  const topNews = (["cripto", "acoes"] as const)
    .flatMap((m) => (live?.mood?.[m].items ?? []).map((i) => ({ ...i, market: m })))
    .filter((i) => i.level >= 2 && now - i.ms <= 24 * 3_600_000)
    .sort((a, b) => b.level - a.level || b.ms - a.ms)
    .slice(0, 6);

  return (
    <div>
      <Freshness ms={live?.generated_ms} label="Painel" />
      <LiveQuotes symbols={[...new Set(actions.filter((a) => a.market === "acoes").map((a) => a.symbol))]}>
        <ActionBox items={actions} waiting={waiting} />
      </LiveQuotes>

      <SectionTitle>Leitura do dia</SectionTitle>
      <div className="grid gap-4 md:grid-cols-2">
        <BriefCard title="Cripto" brief={cryptoBrief(daily, reading, live?.mood?.cripto)} link={{ href: "/relatorio", label: "Relatório completo das 21h15" }} />
        <BriefCard title="Ações dos EUA" brief={stocksBrief(us, live?.mood?.acoes, openStock)} link={{ href: "/acoes", label: "Ver sinais de ações" }} />
      </div>

      <LiveQuotes symbols={["SPY"]}>
        <SectionTitle>Termômetro do mercado</SectionTitle>
        <div className="grid gap-4 md:grid-cols-2">
          <Thermometer
            title="Cripto"
            mood={live?.mood?.cripto ?? null}
            price={{
              up: cryptoTrends.filter((d) => d.trend === "alta").length,
              down: cryptoTrends.filter((d) => d.trend === "queda").length,
              total: cryptoTrends.length,
              change: cryptoChanges.length ? cryptoChanges.reduce((x, y) => x + y, 0) / cryptoChanges.length : null,
              scale: 2,
              changeLabel: "média de BTC, ETH e SOL em 24 h",
            }}
          />
          <Thermometer
            title="Ações dos EUA"
            mood={live?.mood?.acoes ?? null}
            price={{
              up: us.filter((a) => a.trend === "alta").length,
              down: us.filter((a) => a.trend === "baixa").length,
              total: us.length,
              change: spy?.change_day ?? null,
              scale: 1,
              changeLabel: "S&P 500 (SPY) no dia",
              quoteSymbol: "SPY",
            }}
          />
        </div>
      </LiveQuotes>

      <SectionTitle>Cripto agora</SectionTitle>
      <div className="grid gap-4 md:grid-cols-3">
        {(live?.assets ?? []).map((a) => {
          const d = daily?.assets.find((x) => x.name === a.name);
          const pos = d?.positioning;
          const p = a.price ?? d?.close ?? null;
          const f4 = a.frames.find((f) => f.tf === "4h");
          return (
            <section key={a.symbol} className="rounded border-2 border-ink p-4">
              <div className="flex items-center justify-between">
                <h3 className="text-2xl font-black">{a.name}</h3>
                <Link href={`/grafico?ativo=${a.name}&tf=4h`} className="rounded bg-accent px-3 py-1 font-black">
                  Gráfico
                </Link>
              </div>
              <LivePrice symbol={a.symbol} fallback={a.price} fallbackChange={a.change_24h} />
              {d ? (
                <p className="mt-2">
                  Tendência: <b className="font-black">{d.trend}</b>
                  {d.previous_trend && d.previous_trend !== d.trend ? ` (antes: ${d.previous_trend})` : ""}
                </p>
              ) : null}
              {pos ? (
                <div className="mt-3">
                  <p className="mb-1 text-sm font-extrabold">Futuros (Binance, Bybit e OKX)</p>
                  <LongShortBar long={pos.long_share} before={pos.long_share_before} />
                  {pos.top_long_share != null ? (
                    <p className="mt-1 text-sm">Maiores traders: {Math.round(pos.top_long_share * 100)}% do dinheiro comprado.</p>
                  ) : null}
                  {pos.oi_change_24h != null ? <p className="text-sm">Posições abertas: {pct(pos.oi_change_24h)} em 24h.</p> : null}
                </div>
              ) : null}
              {p != null && (pos?.longs_zone || pos?.shorts_zone) ? (
                <div className="mt-3 space-y-1 text-[15px]">
                  <p className="text-sm font-extrabold">Liquidações estimadas</p>
                  {pos?.longs_zone ? (
                    <p>
                      <span className="font-black text-down">Comprados</span> entre {price(pos.longs_zone.low)} e {price(pos.longs_zone.high)} ({usdShort(pos.longs_zone.usd)}), {pct(change(p, pos.longs_zone.high), false)} abaixo.
                    </p>
                  ) : null}
                  {pos?.shorts_zone ? (
                    <p>
                      <span className="font-black text-up">Vendidos</span> entre {price(pos.shorts_zone.low)} e {price(pos.shorts_zone.high)} ({usdShort(pos.shorts_zone.usd)}), {pct(change(p, pos.shorts_zone.low), false)} acima.
                    </p>
                  ) : null}
                </div>
              ) : null}
              {f4 && (f4.liquidity.above != null || f4.liquidity.below != null) ? (
                <p className="mt-3 text-[15px]">
                  <span className="text-sm font-extrabold">Liquidez no 4h: </span>
                  acima {price(f4.liquidity.above)}, abaixo {price(f4.liquidity.below)}.
                </p>
              ) : null}
            </section>
          );
        })}
      </div>

      <SectionTitle>Notícias mais importantes (24 h)</SectionTitle>
      <TopNews items={topNews} />
      <p className="mt-2">
        <Link href="/avisos" className="font-black underline">
          Ver todos os avisos e notícias
        </Link>
      </p>

      <div className="mt-8">
        <LogoutButton />
      </div>
    </div>
  );
}
