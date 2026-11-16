import Link from "next/link";
import { Freshness, NoData } from "@/components/Common";
import { ScoreCard } from "@/components/ScoreCard";
import { getLive } from "@/lib/data";
import { bookRs, score, SETUPS } from "@/lib/score";

export default async function ScoreboardPage() {
  const live = await getLive();
  if (!live) return <NoData />;
  const rs = bookRs(live.books);
  const shown = SETUPS.filter((s) => rs.has(s.id) || s.id === "acoes-1d" || s.id === "acoes-1w");
  return (
    <div>
      <Freshness ms={live.generated_ms} label="Placar" />
      <h1 className="text-2xl font-black">Placar dos setups ao vivo</h1>
      <p className="mt-2 leading-relaxed">
        Cada trade que as contas fictícias fecharam desde que começaram, separado por setup, em R: 1R é o que se perde quando o preço bate no stop. Um trade que sai no alvo de 2 vezes o risco vale +2R. Ao lado fica o que o teste de 10 anos deu para o mesmo setup.
      </p>
      <p className="mt-2 leading-relaxed">
        A conta só diz algo depois de uns 20 trades. Antes disso, uma sequência de stops ou de alvos é normal até num setup bom. Para comparar com os seus trades de verdade, veja <Link href="/meus-trades" className="underline">Meus trades</Link>.
      </p>
      {shown.map((s) => (
        <ScoreCard key={s.id} title={s.label} score={score(rs.get(s.id) ?? [])} reference={s.reference} note={s.note} />
      ))}
    </div>
  );
}
