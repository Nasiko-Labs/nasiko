import type { Metadata } from "next";
import type { ReactNode } from "react";
import { Fraunces, Newsreader, IBM_Plex_Mono } from "next/font/google";
import "./globals.css";

const display = Fraunces({
  subsets: ["latin"],
  variable: "--font-display",
  style: ["normal", "italic"],
  display: "swap",
});

const serif = Newsreader({
  subsets: ["latin"],
  variable: "--font-serif",
  style: ["normal", "italic"],
});

const mono = IBM_Plex_Mono({
  subsets: ["latin"],
  weight: ["400", "500"],
  variable: "--font-mono",
});

export const metadata: Metadata = {
  title: "AgentKV — recorded reuse",
  description:
    "Workflow-aware inference optimization through Nasiko. Public evidence for prefix reuse, cost, and latency. Live control plane stays on Modal.",
};

export default function RootLayout({ children }: { children: ReactNode }) {
  return (
    <html lang="en" className={`${display.variable} ${serif.variable} ${mono.variable}`}>
      <body className="font-serif antialiased">{children}</body>
    </html>
  );
}
