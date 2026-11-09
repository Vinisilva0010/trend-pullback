import type { Pending, Setup } from "@/lib/schema";
import { rMultiple } from "@/lib/format";
import { confirmLine, planLines, sideWord, STATUS_TEXT, sweepStory } from "@/lib/view";

function Tag({ candidate }: { candidate: boolean }) {
  return candidate ? (
    <span className="rounded bg-accent px-2 py-0.5 text-sm font-black text-ink">Candidato (spring/UTAD)</span>
  ) : (
    <span className="rounded bg-panel px-2 py-0.5 text-sm font-extrabold">Só acompanhar</span>
  );
}

export function SetupCard({ asset, frameLabel, setup }: { asset: string; frameLabel: string; setup: Setup }) {
  const long = setup.side === "long";
  const closed = ["alvo", "stop", "prazo"].includes(setup.status);
  return (
    <article className={`rounded border-2 p-3 ${setup.candidate ? "border-accent" : "border-line"}`}>
      <div className="flex flex-wrap items-center gap-2">
        <span className={`rounded px-2 py-0.5 text-sm font-black text-white ${long ? "bg-up" : "bg-down"}`}>{sideWord(setup.side)}</span>
        <span className="text-lg font-black">{asset}</span>
        <span className="font-bold">{frameLabel}</span>
        <Tag candidate={setup.candidate} />
      </div>
      <p className="mt-2 text-lg font-black">
        {STATUS_TEXT[setup.status]}
        {closed && setup.r != null ? <span className={setup.r > 0 ? "text-up" : "text-down"}> {rMultiple(setup.r)}</span> : null}
      </p>
      <p className="mt-1">{sweepStory(setup)}</p>
      <ul className="mt-1 space-y-0.5">
        {planLines(setup).map((l) => (
          <li key={l} className="num">
            {l}
          </li>
        ))}
      </ul>
      {setup.wyckoff || setup.climax ? (
        <p className="mt-1 text-sm">
          {setup.wyckoff ? "Varrida na borda de uma lateral (Wyckoff). " : ""}
          {setup.climax ? "Volume de clímax na varrida." : ""}
        </p>
      ) : null}
    </article>
  );
}

export function PendingCard({ asset, frameLabel, pending }: { asset: string; frameLabel: string; pending: Pending }) {
  return (
    <article className={`rounded border-2 border-dashed p-3 ${pending.candidate ? "border-accent" : "border-line"}`}>
      <div className="flex flex-wrap items-center gap-2">
        <span className="rounded bg-ink px-2 py-0.5 text-sm font-black text-paper">Varrida, falta confirmar</span>
        <span className="text-lg font-black">{asset}</span>
        <span className="font-bold">{frameLabel}</span>
        <Tag candidate={pending.candidate} />
      </div>
      <p className="mt-2">
        {sideWord(pending.side)}: {sweepStory(pending)}
      </p>
      <p className="mt-1 font-extrabold">{confirmLine(pending)}</p>
    </article>
  );
}
