import { Freshness, NoData, Pill, SectionTitle } from "@/components/Common";
import { PendingCard, SetupCard } from "@/components/SetupCard";
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
      <Freshness ms={live.generated_ms} label="Setups" />
      <div className="flex flex-wrap gap-2">
        <Pill href="/setups" active={!all}>
          Só candidatos
        </Pill>
        <Pill href="/setups?ver=todos" active={all}>
          Todos
        </Pill>
      </div>
      <p className="mt-3 leading-relaxed">
        Candidato é a varrida na borda de uma lateral (spring ou UTAD) no gráfico de 1h ou 4h: o único tipo que ficou positivo no histórico, ainda sem prova. Os outros perderam dinheiro no histórico e aparecem só para você acompanhar.
      </p>
      {block("Trades abertos", open.map((r) => <SetupCard key={r.s.key} asset={r.a.name} frameLabel={r.f.label} setup={r.s} />), "Nenhum trade aberto.")}
      {block("Ordens armadas", armed.map((r) => <SetupCard key={r.s.key} asset={r.a.name} frameLabel={r.f.label} setup={r.s} />), "Nenhuma ordem armada.")}
      {block("Varridas esperando confirmação", pending.map((r) => <PendingCard key={r.p.key} asset={r.a.name} frameLabel={r.f.label} pending={r.p} />), "Nenhuma varrida esperando.")}
      {block("Fechados recentes", closed.map((r) => <SetupCard key={r.s.key} asset={r.a.name} frameLabel={r.f.label} setup={r.s} />), "Nenhum trade fechado ainda.")}
    </div>
  );
}
