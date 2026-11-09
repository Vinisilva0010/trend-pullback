"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";

const ITEMS = [
  { href: "/", label: "Início" },
  { href: "/grafico", label: "Gráfico" },
  { href: "/setups", label: "Setups" },
  { href: "/mercados", label: "Mercados" },
  { href: "/contas", label: "Contas" },
  { href: "/avisos", label: "Avisos" },
];

export function Nav() {
  const path = usePathname();
  return (
    <nav
      aria-label="Seções"
      className="fixed inset-x-0 bottom-0 z-20 border-t-2 border-ink bg-paper pb-[env(safe-area-inset-bottom)] md:static md:border-t-0 md:border-b-2"
    >
      <ul className="mx-auto flex max-w-5xl">
        {ITEMS.map((item) => {
          const active = item.href === "/" ? path === "/" : path.startsWith(item.href);
          return (
            <li key={item.href} className="flex-1">
              <Link
                href={item.href}
                aria-current={active ? "page" : undefined}
                className={`block py-3 text-center text-[13px] font-extrabold sm:text-[15px] md:py-4 ${
                  active ? "bg-accent text-ink" : "text-ink hover:bg-panel"
                }`}
              >
                {item.label}
              </Link>
            </li>
          );
        })}
      </ul>
    </nav>
  );
}
