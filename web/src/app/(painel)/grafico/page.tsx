import { CandleChart } from "@/components/CandleChart";
import { Freshness, NoData, Pill, SectionTitle } from "@/components/Common";
import { PendingCard, SetupCard } from "@/components/SetupCard";
import { StormerCard } from "@/components/StormerCard";
import { getDaily, getLive } from "@/lib/data";
import { price } from "@/lib/format";
import { ASSETS, TF_LABEL, TIMEFRAMES, type AssetName, type Timeframe } from "@/lib/view";

export default async function ChartPage({ searchParams }: { searchParams: Promise<{ ativo?: string; tf?: string }> }) {
  const params = await searchParams;
  const asset: AssetName = (ASSETS as readonly string[]).includes(params.ativo ?? "") ? (params.ativo as AssetName) : "BTC";
  const tf: Timeframe = (TIMEFRAMES as readonly string[]).includes(params.tf ?? "") ? (params.tf as Timeframe) : "4h";
  const [live, daily] = await Promise.all([getLive(), getDaily()]);
  const a = live?.assets.find((x) => x.name === asset);
  const frame = a?.frames.find((f) => f.tf === tf) ?? null;
  const pos = daily?.assets.find((x) => x.name === asset)?.positioning ?? null;
  const active = frame?.setups.filter((s) => s.status === "armado" || s.status === "aberto") ?? [];
  const recent = (frame?.setups ?? []).filter((s) => !active.includes(s)).slice(-6).reverse();
  // Stormer in crypto: the daily and weekly charts, whatever chart is open.
  const stormerAll = a?.stormer ?? [];
  const stormerNow = stormerAll.filter((s) => s.status === "sinal" || s.status === "aberto");
  const stormerPast = stormerAll
    .filter((s) => s.exit_ms != null)
    .sort((x, y) => (y.exit_ms ?? 0) - (x.exit_ms ?? 0))
    .slice(0, 4);
  const stormer = stormerNow.filter((s) => s.tf === tf);
  const extra = stormer.flatMap((s) => [
    { price: s.trigger, color: "entry" as const, title: s.status === "sinal" ? "Stormer: gatilho" : "Stormer: entrada" },
    { price: s.stop, color: "down" as const, title: "Stormer: stop" },
    { price: s.target, color: "up" as const, title: "Stormer: alvo" },
  ]);

  return (
    <div>
      <Freshness ms={live?.generated_ms} label="Setups" />
      <div className="mb-3 flex flex-wrap gap-2">
        {ASSETS.map((x) => (
          <Pill key={x} href={`/grafico?ativo=${x}&tf=${tf}`} active={x === asset}>
            {x}
          </Pill>
        ))}
        <span className="mx-1" />
        {TIMEFRAMES.map((t) => (
          <Pill key={t} href={`/grafico?ativo=${asset}&tf=${t}`} active={t === tf}>
            {TF_LABEL[t]}
          </Pill>
        ))}
      </div>
      <CandleChart symbol={`${asset}USDT`} tf={tf} frame={frame} longsZone={pos?.longs_zone ?? null} shortsZone={pos?.shorts_zone ?? null} extra={extra} />

      <SectionTitle>O que as linhas querem dizer</SectionTitle>
      <ul className="space-y-1 leading-relaxed">
        <li>
          <b className="font-black text-[#c99700]">Amarela grossa</b>: entrada do setup. <b className="font-black text-down">Vermelha</b>: stop.{" "}
          <b className="font-black text-up">Verde</b>: alvo.
        </li>
        <li>
          <b className="font-black text-[#1f5fd1]">Azul tracejada</b>: o preço que confirma uma varrida que ainda está esperando.
        </li>
        <li>
          <b className="font-black">Preta pontilhada</b>: liquidez mais perto (topo ou fundo ainda não tocado, onde ficam stops)
          {frame ? `: acima ${price(frame.liquidity.above)}, abaixo ${price(frame.liquidity.below)}` : ""}.
        </li>
        <li>
          <b className="font-black">Tracejado longo</b>: zona estimada onde os comprados (vermelho) ou os vendidos (verde) seriam liquidados.
        </li>
        <li>Setas marcam as varridas; círculo é a entrada; quadrado é a saída (verde no alvo, vermelho no stop).</li>
      </ul>

      {!live ? <NoData /> : null}
      <SectionTitle>Stormer em {asset} (diário e semanal)</SectionTitle>
      <p className="mb-2 leading-relaxed">
        Em cripto o Stormer acompanha o trap (o preço passa do fundo ou do topo anterior e volta) no diário e no semanal, compras e vendas. Está em observação: no teste ficou acima da base, mas ainda com poucos trades para ter certeza. Os sinais também vão para o Telegram.
      </p>
      {stormerNow.length ? (
        <div className="mb-3 grid gap-3 md:grid-cols-2">
          {stormerNow.map((s) => (
            <StormerCard key={s.key} asset={asset} item={s} />
          ))}
        </div>
      ) : (
        <p className="rounded bg-panel p-3">
          Nenhum sinal do Stormer agora em {asset}. Os sinais aparecem quando fecha um candle diário (21h) ou semanal (domingo à noite).
        </p>
      )}
      {stormerPast.length ? (
        <>
          <p className="mb-2 mt-4 text-lg font-black">Últimos trades do Stormer em {asset}</p>
          <div className="grid gap-3 md:grid-cols-2">
            {stormerPast.map((s) => (
              <StormerCard key={s.key} asset={asset} item={s} />
            ))}
          </div>
        </>
      ) : null}
      {frame ? (
        <>
          <SectionTitle>Smart Money em {asset} · {TF_LABEL[tf]}</SectionTitle>
          {active.length === 0 && frame.pending.length === 0 ? <p className="rounded bg-panel p-3">Nenhum setup de Smart Money armado, aberto ou formando neste gráfico.</p> : null}
          <div className="grid gap-3 md:grid-cols-2">
            {active.map((s) => (
              <SetupCard key={s.key} asset={asset} frameLabel={frame.label} setup={s} />
            ))}
            {frame.pending.map((p) => (
              <PendingCard key={p.key} asset={asset} frameLabel={frame.label} pending={p} />
            ))}
          </div>
          {recent.length ? (
            <>
              <SectionTitle>Últimos setups deste gráfico</SectionTitle>
              <div className="grid gap-3 md:grid-cols-2">
                {recent.map((s) => (
                  <SetupCard key={s.key} asset={asset} frameLabel={frame.label} setup={s} />
                ))}
              </div>
            </>
          ) : null}
        </>
      ) : null}
    </div>
  );
}
