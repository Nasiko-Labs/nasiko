import type { Metadata } from "next";
import "./globals.css";

export const metadata: Metadata = {
  title: "Agent Metrics | Nasiko",
  description: "Per-agent performance metrics for the Nasiko Titan Builder Challenge",
};

export default function RootLayout({
  children,
}: Readonly<{
  children: React.ReactNode;
}>) {
  return (
    <html lang="en">
      <body>{children}</body>
    </html>
  );
}
