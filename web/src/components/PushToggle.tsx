"use client";

import { useEffect, useState } from "react";

function keyBytes(base64: string): Uint8Array<ArrayBuffer> {
  const padded = (base64 + "=".repeat((4 - (base64.length % 4)) % 4)).replaceAll("-", "+").replaceAll("_", "/");
  const raw = atob(padded);
  const out = new Uint8Array(new ArrayBuffer(raw.length));
  for (let i = 0; i < raw.length; i++) out[i] = raw.charCodeAt(i);
  return out;
}

type State = "loading" | "unsupported" | "ios-install" | "off" | "on" | "blocked" | "no-keys";

/** Turns phone/browser notifications on or off for this device. */
export function PushToggle() {
  const [state, setState] = useState<State>("loading");
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<string | null>(null);

  useEffect(() => {
    (async () => {
      const ios = /iPhone|iPad|iPod/.test(navigator.userAgent);
      const standalone = window.matchMedia("(display-mode: standalone)").matches || (navigator as { standalone?: boolean }).standalone === true;
      if (!("serviceWorker" in navigator) || !("PushManager" in window)) {
        setState(ios && !standalone ? "ios-install" : "unsupported");
        return;
      }
      if (Notification.permission === "denied") {
        setState("blocked");
        return;
      }
      const reg = await navigator.serviceWorker.register("/sw.js", { scope: "/" });
      const sub = await reg.pushManager.getSubscription();
      setState(sub ? "on" : "off");
    })().catch(() => setState("unsupported"));
  }, []);

  async function enable() {
    setBusy(true);
    setMessage(null);
    try {
      const keyRes = await fetch("/api/push");
      const { publicKey } = (await keyRes.json()) as { publicKey: string | null };
      if (!publicKey) {
        setState("no-keys");
        return;
      }
      const permission = await Notification.requestPermission();
      if (permission !== "granted") {
        setState(permission === "denied" ? "blocked" : "off");
        return;
      }
      const reg = await navigator.serviceWorker.ready;
      const sub = await reg.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: keyBytes(publicKey) });
      const r = await fetch("/api/push", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(sub.toJSON()) });
      if (!r.ok) throw new Error();
      setState("on");
      setMessage("Pronto: este aparelho vai receber os avisos importantes.");
    } catch {
      setMessage("Não deu para ligar as notificações. Tente de novo.");
    } finally {
      setBusy(false);
    }
  }

  async function disable() {
    setBusy(true);
    try {
      const reg = await navigator.serviceWorker.ready;
      const sub = await reg.pushManager.getSubscription();
      if (sub) {
        await fetch("/api/push", { method: "DELETE", headers: { "content-type": "application/json" }, body: JSON.stringify({ endpoint: sub.endpoint }) });
        await sub.unsubscribe();
      }
      setState("off");
      setMessage("Notificações desligadas neste aparelho.");
    } finally {
      setBusy(false);
    }
  }

  const TEXT: Record<State, string> = {
    loading: "Verificando notificações…",
    unsupported: "Este navegador não recebe notificações.",
    "ios-install": "No iPhone: toque em Compartilhar e depois em \"Adicionar à Tela de Início\". Abra o painel pelo ícone e ligue as notificações aqui.",
    off: "Notificações desligadas neste aparelho.",
    on: "Notificações ligadas neste aparelho.",
    blocked: "As notificações estão bloqueadas nas configurações do navegador para este site.",
    "no-keys": "As chaves de notificação ainda não foram configuradas na Vercel (VAPID_PUBLIC_KEY e VAPID_PRIVATE_KEY).",
  };

  return (
    <div className="rounded border-2 border-ink p-3">
      <p className="font-extrabold">{TEXT[state]}</p>
      {state === "off" ? (
        <button type="button" disabled={busy} onClick={enable} className="mt-2 rounded bg-accent px-4 py-2 font-black text-ink disabled:opacity-60">
          Ligar notificações
        </button>
      ) : null}
      {state === "on" ? (
        <button type="button" disabled={busy} onClick={disable} className="mt-2 rounded border-2 border-ink px-4 py-2 font-black disabled:opacity-60">
          Desligar
        </button>
      ) : null}
      {message ? <p className="mt-2">{message}</p> : null}
    </div>
  );
}
