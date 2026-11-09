"use client";

import { useState } from "react";

export function LoginForm() {
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function submit(form: FormData) {
    setBusy(true);
    setError(null);
    const r = await fetch("/api/login", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ password: String(form.get("password") ?? "") }),
    }).catch(() => null);
    setBusy(false);
    if (r?.ok) {
      window.location.href = "/";
      return;
    }
    const body = (await r?.json().catch(() => null)) as { error?: string } | null;
    setError(body?.error ?? "Não deu para entrar agora.");
  }

  return (
    <main className="mx-auto flex min-h-dvh max-w-sm flex-col justify-center px-4">
      <h1 className="text-3xl font-black">Trend Pullback</h1>
      <p className="mt-1">Painel da sua ferramenta de trade.</p>
      <form action={submit} className="mt-6 space-y-3">
        <label className="block">
          <span className="font-extrabold">Senha</span>
          <input
            name="password"
            type="password"
            autoComplete="current-password"
            required
            className="mt-1 block w-full rounded border-2 border-ink px-3 py-3 text-lg font-bold"
          />
        </label>
        <button type="submit" disabled={busy} className="w-full rounded bg-accent py-3 text-lg font-black text-ink disabled:opacity-60">
          {busy ? "Entrando…" : "Entrar"}
        </button>
        {error ? <p className="font-extrabold text-down">{error}</p> : null}
      </form>
    </main>
  );
}
