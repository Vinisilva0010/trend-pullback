import Link from "next/link";
import { ago, when } from "@/lib/format";

/** Shows how old the data is, and warns when the tool stopped sending. */
export function Freshness({ ms, label, maxHours = 2 }: { ms: number | null | undefined; label: string; maxHours?: number }) {
  if (!ms) return null;
  const hours = (Date.now() - ms) / 3_600_000;
  if (hours > maxHours) {
    return (
      <div role="alert" className="mb-4 rounded border-2 border-down bg-down-soft p-3 font-extrabold text-ink">
        {label} sem atualização desde {when(ms)} ({ago(ms)}). O computador pode estar desligado, dormindo ou sem internet.
      </div>
    );
  }
  return (
    <p className="mb-3 text-sm">
      {label} atualizado {ago(ms)} ({when(ms)}).
    </p>
  );
}

export function NoData() {
  return (
    <div className="rounded border-2 border-ink bg-accent-soft p-4">
      <p className="text-lg font-black">Ainda não chegaram dados da ferramenta.</p>
      <p className="mt-1">
        Depois que o arquivo .env da ferramenta tiver DASHBOARD_URL e DASHBOARD_TOKEN, a próxima rodada de hora em hora manda tudo para cá.
      </p>
    </div>
  );
}

export function SectionTitle({ children }: { children: React.ReactNode }) {
  return <h2 className="mb-2 mt-6 text-xl font-black">{children}</h2>;
}

export function Pill({ href, active, children }: { href: string; active: boolean; children: React.ReactNode }) {
  return (
    <Link
      href={href}
      aria-current={active ? "page" : undefined}
      className={`rounded border-2 border-ink px-2.5 py-1 text-[15px] font-extrabold md:px-3 md:py-1.5 md:text-base ${active ? "bg-ink text-paper" : "bg-paper"}`}
    >
      {children}
    </Link>
  );
}
