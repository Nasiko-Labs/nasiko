# Nasiko Agent Metrics Page

Standalone React metrics dashboard for Challenge 2.

## Run Locally

```powershell
cd web
python -m http.server 4000
```

Open `http://localhost:4000`.

## Docker

```powershell
docker build -t nasiko-web-metrics .
docker run --rm -p 4000:4000 nasiko-web-metrics
```

The page uses React 18 and Chart.js from pinned CDN URLs and does not require a local Node package install. `src/app.jsx` is the readable source; `src/app.js` is the browser-ready compiled file used by `index.html`.
