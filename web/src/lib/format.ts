// Brazilian formatting for prices, percentages, money and times (Brasília).

export function price(v: number | null | undefined): string {
  if (v == null || !Number.isFinite(v)) return "–";
  const digits = Math.abs(v) >= 1000 ? 0 : Math.abs(v) >= 1 ? 2 : 6;
  return v.toLocaleString("pt-BR", { minimumFractionDigits: digits, maximumFractionDigits: digits });
}

export function pct(v: number | null | undefined, signed = true): string {
  if (v == null || !Number.isFinite(v)) return "–";
  const text = Math.abs(v).toLocaleString("pt-BR", { minimumFractionDigits: 1, maximumFractionDigits: 1 });
  if (!signed) return `${text}%`;
  return `${v > 0 ? "+" : v < 0 ? "-" : ""}${text}%`;
}

export function money(v: number): string {
  const digits = Math.abs(v) >= 1000 ? 0 : 2;
  const text = Math.abs(v).toLocaleString("pt-BR", { minimumFractionDigits: digits, maximumFractionDigits: digits });
  return `${v < 0 ? "-" : ""}$${text}`;
}

export function usdShort(v: number): string {
  if (v >= 1e9) return `US$ ${(v / 1e9).toLocaleString("pt-BR", { maximumFractionDigits: 1 })} bi`;
  if (v >= 1e6) return `US$ ${(v / 1e6).toLocaleString("pt-BR", { maximumFractionDigits: 0 })} mi`;
  return `US$ ${v.toLocaleString("pt-BR", { maximumFractionDigits: 0 })}`;
}

export function rMultiple(r: number | null | undefined): string {
  if (r == null) return "–";
  return `${r > 0 ? "+" : ""}${r.toLocaleString("pt-BR", { minimumFractionDigits: 2, maximumFractionDigits: 2 })}R`;
}

const timeFormat = new Intl.DateTimeFormat("pt-BR", {
  timeZone: "America/Sao_Paulo",
  day: "2-digit",
  month: "2-digit",
  hour: "2-digit",
  minute: "2-digit",
});

export function when(ms: number | null | undefined): string {
  if (ms == null) return "–";
  return timeFormat.format(new Date(ms)).replace(",", " às");
}

export function ago(ms: number, now = Date.now()): string {
  const minutes = Math.max(0, Math.round((now - ms) / 60000));
  if (minutes < 1) return "agora";
  if (minutes < 60) return `há ${minutes} min`;
  const hours = Math.round(minutes / 60);
  if (hours < 48) return `há ${hours} h`;
  return `há ${Math.round(hours / 24)} dias`;
}

/** Change from `from` to `to` in percent. */
export function change(from: number, to: number): number {
  return from === 0 ? 0 : (to / from - 1) * 100;
}
