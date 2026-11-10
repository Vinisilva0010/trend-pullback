import { Freshness, NoData, Pill, SectionTitle } from "@/components/Common";
import { PendingCard, SetupCard } from "@/components/SetupCard";
import { StormerCard } from "@/components/StormerCard";
import { getLive } from "@/lib/data";

export default async function SetupsPage({ searchParams }: { searchParams: Promise<{ ver?: string }> }) {
  const { ver } = await searchParams;
  const all = ver === "todos";
  const live = await getLive();
  if (!live) return <NoData />;
  const rows = live.assets.flatMap((a) => a.frames.flatMap((f) => f.setups.map((s) => ({ a, f, s }))));
  const keep = (c: boolean) => all || c;
  const open = rows.filter((r) => r.s.status === "aberto" && keep(r.s.candidate));
  const armed = rows.filter((r) => r.s.status === "armado" && keep(r.s.candidate));
  const closed = rows
    .filter((r) => ["alvo", "stop", "prazo"].includes(r.s.status) && keep(r.s.candidate))
    .sort((x, y) => (y.s.exit_ms ?? 0) - (x.s.exit_ms ?? 0))
    .slice(0, 20);
  const stormerNow = live.assets.flatMap((a) =>
    a.stormer.filter((s) => (s.status === "sinal" || s.status === "aberto") && (all || s.role !== "acompanhar")).map((s) => ({ a, s })),
  );
  const stormerDone = live.assets
    .flatMap((a) => a.stormer.filter((s) => s.exit_ms != null && (all || s.role !== "acompanhar")).map((s) => ({ a, s })))
    .sort((x, y) => (y.s.exit_ms ?? 0) - (x.s.exit_ms ?? 0))
    .slice(0, 12);
  const pending = live.assets.flatMap((a) => a.frames.flatMap((f) => f.pending.filter((p) => keep(p.candidate)).map((p) => ({ a, f, p }))));

  const block = (title: string, items: React.ReactNode[], empty: string) => (
    <>
      <SectionTitle>
        {title} ({items.length})
      </SectionTitle>
      {items.length ? <div className="grid gap-3 md:grid-cols-2">{items}</div> : <p className="rounded bg-panel p-3">{empty}</p>}
    </>
  );

  return (
    <div>
      <h1 className="text-2xl font-black">Sinais cripto (BTC, ETH e SOL)</h1>
      <Freshness ms={live.generated_ms} label="Sinais" />
      <div className="flex flex-wrap gap-2">
        <Pill href="/setups" active={!all}>
          Só candidatos
        </Pill>
        <Pill href="/setups?ver=todos" active={all}>
          Todos
        </Pill>
      </div>
      <p className="mt-3 leading-relaxed">
        Cada setup foi comparado com entrar em qualquer dia com o mesmo stop e o mesmo alvo. Em BTC, ETH e SOL, o trap do Stormer (o preço passa de um fundo ou topo e volta, a varrida de liquidez com a entrada no rompimento) ficou acima dessa base nos dois períodos, mas ainda sem trades suficientes para provar: está em observação e gera aviso. O 123 de compra não ganhou da base e fica só para acompanhar. No Smart Money (1h e 4h), a varrida na borda de uma lateral (spring ou UTAD) segue em teste nas contas fictícias.
      </p>
      {block("Stormer: sinais e trades abertos", stormerNow.map((r) => <StormerCard key={r.s.key} asset={r.a.name} item={r.s} />), "Nenhum sinal do Stormer agora.")}
      {block("Stormer: fechados recentes", stormerDone.map((r) => <StormerCard key={r.s.key} asset={r.a.name} item={r.s} />), "Nenhum trade do Stormer fechado ainda.")}
      {block("Smart Money: trades abertos", open.map((r) => <SetupCard key={r.s.key} asset={r.a.name} frameLabel={r.f.label} setup={r.s} />), "Nenhum trade aberto.")}
      {block("Smart Money: ordens armadas", armed.map((r) => <SetupCard key={r.s.key} asset={r.a.name} frameLabel={r.f.label} setup={r.s} />), "Nenhuma ordem armada.")}
      {block("Varridas esperando confirmação", pending.map((r) => <PendingCard key={r.p.key} asset={r.a.name} frameLabel={r.f.label} pending={r.p} />), "Nenhuma varrida esperando.")}
      {block("Smart Money: fechados recentes", closed.map((r) => <SetupCard key={r.s.key} asset={r.a.name} frameLabel={r.f.label} setup={r.s} />), "Nenhum trade fechado ainda.")}
    </div>
  );
}
