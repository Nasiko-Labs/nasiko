# Smart LLM Router — Remotion demo (≤90s)

Product demo (not a build log). Emphasizes **Nasiko** (control plane + `/v1/route`)
and **DronaHQ** (shared classify → tier → REST → savings contract).

## Setup

```bash
cd agents/smart-llm-router/demo-video
npm install
```

## Preview

```bash
npm start
```

## Render MP4 (~90s, 1920×1080)

```bash
npm run render
# → out/smart-llm-router-demo.mp4
```

Requires Chrome/Chromium (Remotion bundles it on first render).

## Shots used

| File | Role |
| --- | --- |
| `public/shots/console-before-slow.png` | Nasiko console · 20s lag |
| `public/shots/console-after-fast.png` | Nasiko console · ~10ms |
| `public/shots/dashboard-live.png` | Analytics `/ui` with savings |
