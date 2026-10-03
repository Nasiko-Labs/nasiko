import type { Metadata, Viewport } from "next";
import { JetBrains_Mono } from "next/font/google";
import ParticleBackground from "@/components/ParticleBackground";
import "./globals.css";

const jetbrainsMono = JetBrains_Mono({
  subsets: ["latin"],
  variable: "--font-mono",
  display: "swap",
});

export const metadata: Metadata = {
  title: "Nasiko Router Lab — P2 Request Classifier",
  description: "Adaptive semantic request classification and AI model tier routing instrument for the Nasiko LLM Router.",
  icons: {
    icon: "/favicon.ico",
  },
};

export const viewport: Viewport = {
  themeColor: "#000000",
  width: "device-width",
  initialScale: 1,
};

export default function RootLayout({
  children,
}: Readonly<{
  children: React.ReactNode;
}>) {
  return (
    <html lang="en" className={jetbrainsMono.variable}>
      <body className="bg-black text-white antialiased font-mono selection:bg-white selection:text-black overflow-hidden h-screen w-screen fixed inset-0">
        <ParticleBackground />
        <div className="relative z-10 w-full h-full bg-transparent">
          {children}
        </div>
      </body>
    </html>
  );
}
