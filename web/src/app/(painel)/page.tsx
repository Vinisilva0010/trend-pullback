import Link from "next/link";
import { Freshness, NoData, SectionTitle } from "@/components/Common";
import { LivePrice } from "@/components/LivePrices";
import { LongShortBar } from "@/components/LongShortBar";
import { PendingCard, SetupCard } from "@/components/SetupCard";
import { StormerCard } from "@/components/StormerCard";
import { LogoutButton } from "@/components/LogoutButton";
import { getDaily, getLive } from "@/lib/data";
import { change, money, pct, price, usdShort } from "@/lib/format";
import { dayReading, isHeading } from "@/lib/view";

const READING_STYLE: Record<string, string> = {
  CUIDADO: "bg-down text-white",
  OPORTUNIDADE: "bg-up text-white",
  NEUTRO: "bg-panel text-ink",
};

export default async function Home() {
  const [live, daily] = await Promise.all([getLive(), getDaily()]);
  if (!live && !daily) return <NoData />;
  const reading = daily ? dayReading(daily.sections) : null;
  const stormer = (live?.assets ?? []).flatMap((a) =>
    a.stormer.filter((s) => s.status === "sinal" || s.status === "aberto").map((s) => ({ a, s })),
  );
  const candidates = (live?.assets ?? []).flatMap((a) =>
    a.frames.flatMap((f) => [
      ...f.setups.filter((s) => s.candidate && (s.status === "armado" || s.status === "aberto")).map((s) => ({ kind: "setup" as const, a, f, s })),
      ...f.pending.filter((p) => p.candidate).map((p) => ({ kind: "pending" as const, a, f, p })),
    ]),
  );

  return (
    <div>
      <Freshness ms={live?.generated_ms} label="Painel" />
      {reading ? (
        <section className="rounded border-2 border-ink p-4">
          <p className="text-sm font-extrabold">Leitura do dia (relatório das 21h15)</p>
          <p className={`mt-1 inline-block rounded px-3 py-1 text-2xl font-black ${READING_STYLE[reading.label] ?? "bg-panel"}`}>{reading.label}</p>
          <p className="mt-2 text-lg leading-snug">{reading.why}</p>
        </section>
      ) : null}

      <div className="mt-4 grid gap-4 md:grid-cols-3">
        {(live?.assets ?? []).map((a) => {
          const d = daily?.assets.find((x) => x.name === a.name);
          const pos = d?.positioning;
          const p = a.price ?? d?.close ?? null;
          const f4 = a.frames.find((f) => f.tf === "4h");
          return (
            <section key={a.symbol} className="rounded border-2 border-ink p-4">
              <div className="flex items-center justify-between">
                <h2 className="text-2xl font-black">{a.name}</h2>
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

      <SectionTitle>Stormer agora (sinais e trades abertos)</SectionTitle>
      {stormer.length === 0 ? (
        <p className="rounded bg-panel p-3">
          Nenhum sinal do Stormer agora. O que gera aviso é o trap (o preço passa de um fundo ou topo e volta), no diário e no semanal.
        </p>
      ) : (
        <div className="grid gap-3 md:grid-cols-2">
          {stormer.map(({ a, s }) => (
            <StormerCard key={s.key} asset={a.name} item={s} />
          ))}
        </div>
      )}

      <SectionTitle>Smart Money agora (spring/UTAD, 1h e 4h)</SectionTitle>
      {candidates.length === 0 ? (
        <p className="rounded bg-panel p-3">Nenhum candidato armado, aberto ou formando agora.</p>
      ) : (
        <div className="grid gap-3 md:grid-cols-2">
          {candidates.map((c) =>
            c.kind === "setup" ? (
              <SetupCard key={c.s.key} asset={c.a.name} frameLabel={c.f.label} setup={c.s} />
            ) : (
              <PendingCard key={c.p.key} asset={c.a.name} frameLabel={c.f.label} pending={c.p} />
            ),
          )}
        </div>
      )}

      {live?.books.length ? (
        <>
          <SectionTitle>Contas fictícias</SectionTitle>
          <div className="grid gap-3 md:grid-cols-2">
            {live.books.map((b) => (
              <Link key={b.tf} href="/contas" className="rounded border-2 border-ink p-3">
                <p className="font-extrabold">{b.label}</p>
                <p className="num text-2xl font-black">
                  {money(b.equity)}{" "}
                  <span className={b.equity > b.capital ? "text-up" : b.equity < b.capital ? "text-down" : ""}>{pct(change(b.capital, b.equity))}</span>
                </p>
                <p className="text-sm">{b.trades.length} {b.trades.length === 1 ? "trade fechado" : "trades fechados"}</p>
              </Link>
            ))}
          </div>
        </>
      ) : null}

      {daily ? (
        <>
          <SectionTitle>Relatório completo de {daily.date.split("-").reverse().join("/")}</SectionTitle>
          <div className="space-y-4">
            {daily.sections.map((section, i) => (
              <section key={i} className="rounded border-2 border-line p-4 leading-relaxed">
                {section.split("\n").map((line, j) =>
                  line.trim() === "" ? (
                    <div key={j} className="h-3" />
                  ) : isHeading(line) || j === 0 ? (
                    <p key={j} className="mt-1 text-lg font-black">
                      {line}
                    </p>
                  ) : (
                    <p key={j}>{line}</p>
                  ),
                )}
              </section>
            ))}
          </div>
        </>
      ) : null}

      <div className="mt-8">
        <LogoutButton />
      </div>
    </div>
  );
}
