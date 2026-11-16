import { rMultiple } from "@/lib/format";
import { verdict, type Reference, type Score } from "@/lib/score";

function share(v: number): string {
  return `${Math.round(v * 100)}%`;
}

const TONE = { good: "border-up bg-up-soft", bad: "border-down bg-down-soft", neutral: "border-line bg-panel" } as const;

/** One setup: the live numbers, what the test gave and what that means. */
export function ScoreCard({ title, score, reference, note }: { title: string; score: Score; reference: Reference | null; note?: string }) {
  const v = verdict(score, reference);
  return (
    <section className="mt-4 rounded border-2 border-ink p-4">
      <h3 className="text-lg font-black">{title}</h3>
      {score.trades ? (
        <>
          <p className="num mt-1 text-2xl font-black">
            <span className={score.totalR > 0 ? "text-up" : score.totalR < 0 ? "text-down" : ""}>{rMultiple(score.totalR)}</span> no total
          </p>
          <p className="num mt-1">
            {score.trades} {score.trades === 1 ? "trade fechado" : "trades fechados"}, {score.wins} no lucro ({share(score.winShare)}). Média de {rMultiple(score.meanR)} por trade. Melhor {rMultiple(score.bestR)}, pior {rMultiple(score.worstR)}. Maior sequência de perdas seguidas: {score.worstStreak}.
          </p>
          {score.luck && score.trades >= 2 ? (
            <p className="num mt-1 text-sm">
              Com {score.trades} trades, só a sorte ou o azar podem levar a média de {rMultiple(score.luck.low)} a {rMultiple(score.luck.high)}.
            </p>
          ) : null}
        </>
      ) : null}
      <p className="num mt-2">
        {reference
          ? `No teste: ${rMultiple(reference.r)} por trade, ${share(reference.winShare)} no lucro, em ${reference.trades} trades (${reference.source}).`
          : "Sem número de teste em R para comparar."}
        {note ? ` ${note}` : ""}
      </p>
      <p className={`mt-2 rounded border-2 p-2 font-extrabold ${TONE[v.tone]}`}>{v.text}</p>
    </section>
  );
}
