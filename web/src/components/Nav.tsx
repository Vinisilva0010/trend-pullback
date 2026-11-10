"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";
import { useEffect, useState } from "react";

type Item = { href: string; label: string };

// Phone: four main tabs and "Mais" at the bottom. Computer: every tab at the top.
const MAIN: Item[] = [
  { href: "/", label: "Início" },
  { href: "/acoes", label: "Ações" },
  { href: "/mercados", label: "Mercados" },
  { href: "/avisos", label: "Avisos" },
];
const MORE: Item[] = [
  { href: "/setups", label: "Sinais cripto" },
  { href: "/grafico", label: "Gráfico cripto" },
  { href: "/contas", label: "Contas fictícias" },
  { href: "/relatorio", label: "Relatório do dia" },
];
const DESKTOP: Item[] = [MAIN[0]!, MAIN[1]!, MAIN[2]!, MORE[0]!, MORE[1]!, MORE[2]!, MORE[3]!, MAIN[3]!];

const SEEN_KEY = "tp-avisos-visto";

function isActive(path: string, href: string): boolean {
  return href === "/" ? path === "/" : path.startsWith(href);
}

function readSeen(): number {
  try {
    return Number(window.localStorage.getItem(SEEN_KEY) ?? 0) || 0;
  } catch {
    return 0;
  }
}

/** Important alerts not seen yet: the red dot on Avisos and on the app icon. */
function useUnread(path: string, important: number[]): number {
  const [unread, setUnread] = useState(0);
  useEffect(() => {
    const nav = navigator as Navigator & { setAppBadge?: (n?: number) => Promise<void>; clearAppBadge?: () => Promise<void> };
    if (path.startsWith("/avisos")) {
      try {
        window.localStorage.setItem(SEEN_KEY, String(Date.now()));
      } catch {
        // Without storage the dot just shows again next time.
      }
      setUnread(0);
      void nav.clearAppBadge?.().catch(() => undefined);
      return;
    }
    const seen = readSeen();
    const n = important.filter((ms) => ms > seen).length;
    setUnread(n);
    if (n > 0) void nav.setAppBadge?.(n).catch(() => undefined);
    else void nav.clearAppBadge?.().catch(() => undefined);
  }, [path, important]);
  return unread;
}

function Dot({ n }: { n: number }) {
  if (n <= 0) return null;
  return (
    <span className="absolute right-1 top-1 min-w-[1.25rem] rounded-full bg-down px-1 text-center text-xs font-black leading-5 text-white" aria-label={`${n} avisos novos`}>
      {n > 9 ? "9+" : n}
    </span>
  );
}

export function Nav({ important }: { important: number[] }) {
  const path = usePathname();
  const unread = useUnread(path, important);
  const [open, setOpen] = useState(false);
  useEffect(() => setOpen(false), [path]);
  const moreActive = MORE.some((m) => isActive(path, m.href));
  return (
    <>
      {/* Computer */}
      <nav aria-label="Seções" className="hidden border-b-2 border-ink bg-paper md:block">
        <ul className="mx-auto flex max-w-5xl">
          {DESKTOP.map((item) => {
            const active = isActive(path, item.href);
            return (
              <li key={item.href} className="relative flex-1">
                <Link
                  href={item.href}
                  aria-current={active ? "page" : undefined}
                  className={`block py-4 text-center text-[15px] font-extrabold ${active ? "bg-accent" : "hover:bg-panel"}`}
                >
                  {item.label}
                </Link>
                {item.href === "/avisos" ? <Dot n={unread} /> : null}
              </li>
            );
          })}
        </ul>
      </nav>

      {/* Phone */}
      <nav aria-label="Seções" className="fixed inset-x-0 bottom-0 z-30 border-t-2 border-ink bg-paper pb-[env(safe-area-inset-bottom)] md:hidden">
        {open ? (
          <ul className="border-b-2 border-ink">
            {MORE.map((item) => (
              <li key={item.href}>
                <Link
                  href={item.href}
                  aria-current={isActive(path, item.href) ? "page" : undefined}
                  className={`block border-t-2 border-line px-5 py-4 text-lg font-black first:border-t-0 ${isActive(path, item.href) ? "bg-accent" : ""}`}
                >
                  {item.label}
                </Link>
              </li>
            ))}
          </ul>
        ) : null}
        <ul className="flex">
          {MAIN.map((item) => {
            const active = isActive(path, item.href);
            return (
              <li key={item.href} className="relative flex-1">
                <Link href={item.href} aria-current={active ? "page" : undefined} className={`block py-4 text-center text-[15px] font-extrabold ${active ? "bg-accent" : ""}`}>
                  {item.label}
                </Link>
                {item.href === "/avisos" ? <Dot n={unread} /> : null}
              </li>
            );
          })}
          <li className="flex-1">
            <button
              type="button"
              onClick={() => setOpen((v) => !v)}
              aria-expanded={open}
              className={`block w-full py-4 text-center text-[15px] font-extrabold ${open || moreActive ? "bg-accent" : ""}`}
            >
              {open ? "Fechar" : "Mais"}
            </button>
          </li>
        </ul>
      </nav>
    </>
  );
}
