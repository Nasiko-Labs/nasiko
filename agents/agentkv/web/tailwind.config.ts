import type { Config } from "tailwindcss";

const config: Config = {
  content: ["./app/**/*.{ts,tsx}", "./components/**/*.{ts,tsx}"],
  theme: {
    extend: {
      colors: {
        paper: "#efe4c8",
        cream: "#f7f0dc",
        ink: "#12201c",
        phosphor: "#c6ff2e",
        forest: "#1d3a32",
        rule: "#cbbd9a",
        mute: "#6b6454",
        teal: "#237d77",
        ochre: "#ac7b38",
        moss: "#37652f",
      },
      fontFamily: {
        display: ["var(--font-display)", "Georgia", "serif"],
        serif: ["var(--font-serif)", "Georgia", "serif"],
        mono: ["var(--font-mono)", "ui-monospace", "monospace"],
      },
      keyframes: {
        marquee: {
          from: { transform: "translateX(0)" },
          to: { transform: "translateX(-50%)" },
        },
        spinSlow: {
          to: { transform: "rotate(360deg)" },
        },
        rise: {
          from: { opacity: "0", transform: "translateY(12px)" },
          to: { opacity: "1", transform: "none" },
        },
      },
      animation: {
        marquee: "marquee 38s linear infinite",
        spinSlow: "spinSlow 10s linear infinite",
        rise: "rise 0.7s ease-out both",
      },
    },
  },
  plugins: [],
};

export default config;
