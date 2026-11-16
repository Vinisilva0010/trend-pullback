"use client";

import { useRouter } from "next/navigation";
import { useState } from "react";
import { dayMs, parseNum } from "@/lib/journal";
import { SETUPS } from "@/lib/score";

const INPUT = "mt-1 block w-full rounded border-2 border-ink bg-paper px-3 py-2 text-base font-bold";
const BUTTON = "rounded border-2 border-ink bg-accent px-4 py-2 font-black disabled:opacity-50";

function today(): string {
  // The date in Brasília, as yyyy-mm-dd.
  return new Date(Date.now() - 3 * 3_600_000).toISOString().slice(0, 10);
}

async function send(method: "POST" | "PATCH" | "DELETE", body: unknown): Promise<string | null> {
  try {
    const r = await fetch("/api/journal", { method, headers: { "content-type": "application/json" }, body: JSON.stringify(body) });
    if (r.ok) return null;
    const j = (await r.json().catch(() => null)) as { error?: string } | null;
    return j?.error ?? `erro ${r.status}`;
  } catch {
    return "sem conexão com o painel";
  }
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <label className="block font-extrabold">
      {label}
      {children}
    </label>
  );
}

/** Form for a new trade. */
export function NewTradeForm() {
  const router = useRouter();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function submit(e: React.FormEvent<HTMLFormElement>) {
    e.preventDefault();
    const f = new FormData(e.currentTarget);
    const text = (k: string) => String(f.get(k) ?? "");
    const entry = parseNum(text("entry"));
    const stop = parseNum(text("stop"));
    const target = text("target").trim() ? parseNum(text("target")) : null;
    const opened = dayMs(text("opened"));
    if (entry == null || stop == null || opened == null || (text("target").trim() && target == null)) {
      setError("Confira os preços e a data: use números como 123,45.");
      return;
    }
    setBusy(true);
    setError(null);
    const form = e.currentTarget;
    const err = await send("POST", {
      symbol: text("symbol"),
      side: text("side"),
      setup: text("setup"),
      entry,
      stop,
      target,
      opened_ms: opened,
      reason: text("reason"),
    });
    setBusy(false);
    if (err) {
      setError(err);
      return;
    }
    form.reset();
    router.refresh();
  }

  return (
    <form onSubmit={submit} className="mt-3 grid gap-3 rounded border-2 border-ink p-4 md:grid-cols-2">
      <Field label="Ativo (ex.: NVDA, BTC)">
        <input name="symbol" required maxLength={20} autoCapitalize="characters" className={INPUT} />
      </Field>
      <Field label="Lado">
        <select name="side" className={INPUT} defaultValue="long">
          <option value="long">Compra</option>
          <option value="short">Venda (short)</option>
        </select>
      </Field>
      <Field label="Setup">
        <select name="setup" className={INPUT} defaultValue="acoes-1d">
          {SETUPS.map((s) => (
            <option key={s.id} value={s.id}>
              {s.label}
            </option>
          ))}
        </select>
      </Field>
      <Field label="Data da entrada">
        <input name="opened" type="date" required defaultValue={today()} className={INPUT} />
      </Field>
      <Field label="Preço de entrada">
        <input name="entry" required inputMode="decimal" className={INPUT} />
      </Field>
      <Field label="Stop">
        <input name="stop" required inputMode="decimal" className={INPUT} />
      </Field>
      <Field label="Alvo (opcional)">
        <input name="target" inputMode="decimal" className={INPUT} />
      </Field>
      <Field label="Por que entrou (opcional)">
        <input name="reason" maxLength={500} className={INPUT} />
      </Field>
      <div className="md:col-span-2">
        <button type="submit" disabled={busy} className={BUTTON}>
          {busy ? "Salvando..." : "Registrar trade"}
        </button>
        {error ? (
          <p role="alert" className="mt-2 font-extrabold text-down">
            {error}
          </p>
        ) : null}
      </div>
    </form>
  );
}

/** Closing an open trade, or removing one typed by mistake. */
export function CloseTradeForm({ id }: { id: string }) {
  const router = useRouter();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function submit(e: React.FormEvent<HTMLFormElement>) {
    e.preventDefault();
    const f = new FormData(e.currentTarget);
    const exit = parseNum(String(f.get("exit") ?? ""));
    const closed = dayMs(String(f.get("closed") ?? ""));
    if (exit == null || closed == null) {
      setError("Confira o preço de saída e a data.");
      return;
    }
    setBusy(true);
    setError(null);
    const err = await send("PATCH", { id, exit, closed_ms: closed, followed: f.get("followed") === "sim", exit_note: String(f.get("note") ?? "") });
    setBusy(false);
    if (err) setError(err);
    else router.refresh();
  }

  async function remove() {
    if (busy) return;
    setBusy(true);
    const err = await send("DELETE", { id });
    setBusy(false);
    if (err) setError(err);
    else router.refresh();
  }

  return (
    <form onSubmit={submit} className="mt-3 grid gap-3 border-t-2 border-line pt-3 md:grid-cols-2">
      <Field label="Preço de saída">
        <input name="exit" required inputMode="decimal" className={INPUT} />
      </Field>
      <Field label="Data da saída">
        <input name="closed" type="date" required defaultValue={today()} className={INPUT} />
      </Field>
      <Field label="Seguiu o plano (entrada, stop e alvo como planejado)?">
        <select name="followed" className={INPUT} defaultValue="sim">
          <option value="sim">Sim</option>
          <option value="nao">Não (mexeu no stop, saiu antes, entrou fora do gatilho...)</option>
        </select>
      </Field>
      <Field label="Nota da saída (opcional)">
        <input name="note" maxLength={500} className={INPUT} />
      </Field>
      <div className="flex flex-wrap items-center gap-3 md:col-span-2">
        <button type="submit" disabled={busy} className={BUTTON}>
          {busy ? "Salvando..." : "Fechar trade"}
        </button>
        <RemoveButton onConfirm={remove} busy={busy} />
        {error ? (
          <p role="alert" className="font-extrabold text-down">
            {error}
          </p>
        ) : null}
      </div>
    </form>
  );
}

/** Two taps to remove, so a slip of the finger does not erase a trade. */
function RemoveButton({ onConfirm, busy }: { onConfirm: () => void; busy: boolean }) {
  const [armed, setArmed] = useState(false);
  return (
    <button
      type="button"
      disabled={busy}
      onClick={() => (armed ? onConfirm() : setArmed(true))}
      onBlur={() => setArmed(false)}
      className="rounded border-2 border-down px-4 py-2 font-black text-down disabled:opacity-50"
    >
      {armed ? "Toque de novo para apagar" : "Apagar (digitei errado)"}
    </button>
  );
}

/** Removal for a closed trade. */
export function DeleteTrade({ id }: { id: string }) {
  const router = useRouter();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  async function remove() {
    setBusy(true);
    const err = await send("DELETE", { id });
    setBusy(false);
    if (err) setError(err);
    else router.refresh();
  }
  return (
    <div className="mt-2 flex flex-wrap items-center gap-3">
      <RemoveButton onConfirm={remove} busy={busy} />
      {error ? <p role="alert" className="font-extrabold text-down">{error}</p> : null}
    </div>
  );
}
