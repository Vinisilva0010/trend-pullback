import Link from "next/link";
import { Freshness, SectionTitle } from "@/components/Common";
import { getMarkets } from "@/lib/data";
import { pct, price, usdShort } from "@/lib/format";

const dayFormat = new Intl.DateTimeFormat("pt-BR", { timeZone: "UTC", weekday: "short", day: "2-digit", month: "2-digit", year: "numeric" });

function daysUntil(ms: number, now: number): string {
  const days = Math.round((ms - now) / 86_400_000);
  if (days <= 0) return "hoje";
  if (days === 1) return "amanhã";
  return `em ${days} dias`;
}

export default async function IposPage() {
  const markets = await getMarkets();
  const ipos = markets?.ipos;
  if (!markets || !ipos) {
    return (
      <div className="rounded border-2 border-ink bg-accent-soft p-4">
        <p className="text-lg font-black">A lista de IPOs ainda não chegou.</p>
        <p className="mt-1">Ela vem junto com a aba Mercados, na próxima vez que o robô rodar no computador.</p>
      </div>
    );
  }
  // Midnight UTC of today, to count whole days.
  const today = Math.floor(markets.generated_ms / 86_400_000) * 86_400_000;
  const big = ipos.upcoming.filter((u) => u.big);
  const small = ipos.upcoming.filter((u) => !u.big);
  return (
    <div>
      <Freshness ms={markets.generated_ms} label="IPOs" maxHours={50} />
      <SectionTitle>IPOs grandes a caminho</SectionTitle>
      {big.length === 0 ? <p className="rounded bg-panel p-3">Nenhum IPO de US$ 1 bilhão ou mais com data marcada.</p> : null}
      <ul className="space-y-3">
        {big.map((u) => (
          <li key={u.symbol} className="rounded border-2 border-ink p-4">
            <p className="text-xl font-black">
              {u.company} <span className="num">({u.symbol})</span>
            </p>
            <p className="mt-1 text-lg font-black">
              Estreia prevista: {dayFormat.format(new Date(u.date_ms))} ({daysUntil(u.date_ms, today)})
            </p>
            <p className="mt-1">
              Faixa de preço: <b className="num">${price(u.price_low)}</b> a <b className="num">${price(u.price_high)}</b> por ação. Oferta de até <b>{usdShort(u.amount)}</b>, na {u.exchange}.
            </p>
            <p className="mt-2 text-sm">A data e a faixa podem mudar; o painel avisa quando mudarem, 3 dias antes, na véspera, quando o preço final sair e com o resultado do primeiro dia.</p>
          </li>
        ))}
      </ul>

      <SectionTitle>Estrearam há pouco (últimos 90 dias, oferta de US$ 500 milhões ou mais)</SectionTitle>
      {ipos.recent.length === 0 ? <p className="rounded bg-panel p-3">Nenhum.</p> : null}
      <ul className="space-y-3">
        {ipos.recent.map((r) => (
          <li key={r.symbol} className="rounded border-2 border-line p-4">
            <p className="text-lg font-black">
              {r.company} <span className="num">({r.symbol})</span>
            </p>
            <p className="mt-1">
              Preço do IPO: <b className="num">${price(r.price)}</b> em {dayFormat.format(new Date(r.date_ms))}. Oferta de {usdShort(r.amount)}.
            </p>
            {r.first ? (
              <p className="mt-1">
                Primeiro dia: abriu a <b className="num">{price(r.first[0])}</b> (<span className="num">{pct((r.first[0] / r.price - 1) * 100)}</span> sobre o IPO), máxima <b className="num">{price(r.first[1])}</b>, mínima <b className="num">{price(r.first[2])}</b>, fechou a <b className="num">{price(r.first[3])}</b>.
              </p>
            ) : (
              <p className="mt-1 font-bold">Ainda sem preços de negociação na fonte.</p>
            )}
            {r.last != null ? (
              <p className="mt-1 text-lg font-black">
                Agora: <span className="num">{price(r.last)}</span>, <span className="num">{pct(r.from_ipo)}</span> sobre o preço do IPO{r.days ? `, ${r.days} ${r.days === 1 ? "pregão" : "pregões"} depois` : ""}.
              </p>
            ) : null}
            {r.first ? (
              <Link href={`/mercados/${encodeURIComponent(r.symbol)}`} className="mt-2 inline-block rounded border-2 border-ink px-3 py-1.5 font-black">
                Ver gráfico e preço ao vivo →
              </Link>
            ) : null}
          </li>
        ))}
      </ul>

      <SectionTitle>Como entrar em um IPO</SectionTitle>
      <div className="space-y-2 rounded border-2 border-ink p-4 leading-relaxed">
        <p>{ipos.history}</p>
        <p className="font-black">
          O teste das formas de entrar (rompimento da máxima do primeiro dia, da primeira semana, volta ao preço do IPO e outras) roda no computador com o comando ipo-study, sobre os IPOs grandes desde 2019. Enquanto nenhuma forma passar nesse teste, os avisos trazem os fatos e os níveis, sem mandar comprar.
        </p>
      </div>

      {small.length ? (
        <>
          <SectionTitle>Outros IPOs com data marcada (US$ 100 milhões ou mais)</SectionTitle>
          <ul className="divide-y-2 divide-line rounded border-2 border-line">
            {small.map((u) => (
              <li key={u.symbol} className="flex flex-wrap items-baseline gap-x-3 px-3 py-2">
                <span className="font-black">{u.company}</span>
                <span className="num">({u.symbol})</span>
                <span>{dayFormat.format(new Date(u.date_ms))}</span>
                <span className="num">
                  ${price(u.price_low)} a ${price(u.price_high)}
                </span>
                <span>{usdShort(u.amount)}</span>
              </li>
            ))}
          </ul>
        </>
      ) : null}
    </div>
  );
}
