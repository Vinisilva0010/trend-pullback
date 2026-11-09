import type { Metadata, Viewport } from "next";
import { headers } from "next/headers";
import "./globals.css";

export const metadata: Metadata = {
  title: "Trend Pullback",
  description: "Painel da ferramenta de trade: setups, contas fictícias e avisos.",
  manifest: "/manifest.webmanifest",
  appleWebApp: { capable: true, title: "Trend Pullback", statusBarStyle: "default" },
  icons: { icon: "/icon-192.png", apple: "/apple-touch-icon.png" },
  robots: { index: false, follow: false },
};

export const viewport: Viewport = {
  width: "device-width",
  initialScale: 1,
  themeColor: "#ffffff",
};

export default async function RootLayout({ children }: { children: React.ReactNode }) {
  // Reading the request headers keeps every page dynamic, which the CSP
  // nonce set by the proxy needs.
  await headers();
  return (
    <html lang="pt-BR">
      <body>{children}</body>
    </html>
  );
}
