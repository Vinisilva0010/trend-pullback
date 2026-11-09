import { CandleChart } from "@/components/CandleChart";
import { Freshness, NoData, Pill, SectionTitle } from "@/components/Common";
import { PendingCard, SetupCard } from "@/components/SetupCard";
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
      <CandleChart symbol={`${asset}USDT`} tf={tf} frame={frame} longsZone={pos?.longs_zone ?? null} shortsZone={pos?.shorts_zone ?? null} />

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
      {frame ? (
        <>
          <SectionTitle>Agora em {asset} · {TF_LABEL[tf]}</SectionTitle>
          {active.length === 0 && frame.pending.length === 0 ? <p className="rounded bg-panel p-3">Nada armado, aberto ou formando neste gráfico.</p> : null}
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
