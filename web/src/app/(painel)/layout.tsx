import { Nav } from "@/components/Nav";
import { AutoRefresh } from "@/components/AutoRefresh";
import { requireLogin } from "@/lib/auth";
import { getLive } from "@/lib/data";

export const dynamic = "force-dynamic";

export default async function PanelLayout({ children }: { children: React.ReactNode }) {
  await requireLogin();
  const live = await getLive();
  const weekAgo = Date.now() - 7 * 86_400_000;
  const important = (live?.events ?? []).filter((e) => e.level >= 3 && e.ms > weekAgo).map((e) => e.ms);
  return (
    <>
      <Nav important={important} />
      <AutoRefresh />
      <main className="mx-auto max-w-5xl px-4 pb-28 pt-4 md:pb-10">{children}</main>
    </>
  );
}
