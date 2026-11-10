import { Freshness, NoData, SectionTitle } from "@/components/Common";
import { getDaily } from "@/lib/data";
import { isHeading } from "@/lib/view";

export default async function ReportPage() {
  const daily = await getDaily();
  if (!daily) return <NoData />;
  return (
    <div>
      <Freshness ms={daily.generated_ms} label="Relatório" maxHours={30} />
      <SectionTitle>Relatório de {daily.date.split("-").reverse().join("/")} (cripto, 21h15)</SectionTitle>
      <div className="space-y-4">
        {daily.sections.map((section, i) => (
          <section key={i} className="rounded border-2 border-line p-4 leading-relaxed">
            {section.split("\n").map((line, j) =>
              line.trim() === "" ? (
                <div key={j} className="h-3" />
              ) : isHeading(line) || j === 0 ? (
                <p key={j} className="mt-1 text-lg font-black">
                  {line}
                </p>
              ) : (
                <p key={j}>{line}</p>
              ),
            )}
          </section>
        ))}
      </div>
    </div>
  );
}
