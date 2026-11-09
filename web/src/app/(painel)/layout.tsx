import { Nav } from "@/components/Nav";
import { AutoRefresh } from "@/components/AutoRefresh";
import { requireLogin } from "@/lib/auth";

export const dynamic = "force-dynamic";

export default async function PanelLayout({ children }: { children: React.ReactNode }) {
  await requireLogin();
  return (
    <>
      <Nav />
      <AutoRefresh />
      <main className="mx-auto max-w-5xl px-4 pb-28 pt-4 md:pb-10">{children}</main>
    </>
  );
}
