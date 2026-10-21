/** @type {import('tailwindcss').Config} */
export default {
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  theme: {
    extend: {
      colors: {
        ink: "#0a0e14",
        panel: "#0f1622",
        panel2: "#141d2b",
        edge: "#1e2a3c",
        low: "#3fb950",
        medium: "#d29922",
        high: "#f0883e",
        critical: "#f85149",
        accent: "#58a6ff",
      },
      fontFamily: {
        mono: ["JetBrains Mono", "ui-monospace", "SFMono-Regular", "monospace"],
      },
    },
  },
  plugins: [],
};
