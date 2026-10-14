/**
 * The pre-paint script: mode and colour theme on <html> before first paint, inlined by the app's
 * root layout under the page CSP nonce. Ported from the lab's `oss/src/index.html` (nasiko-ui-lab
 * 1b99bdda). The key names, theme ids and attribute duplicate `theme-store.ts`, which the script
 * can't import; `theme-store.test.tsx` checks they match. The store takes over after boot (menu
 * changes, following the OS on System).
 */
export const THEME_PREPAINT_SCRIPT = `
      // Mode and colour theme before first paint. The key names, theme ids and attribute duplicate
      // theme-store.ts, which can't be imported here; theme-store.test.tsx checks they match.
      (function () {
        var theme = null, accent = null
        try { theme = localStorage.getItem('openruntime.theme'); accent = localStorage.getItem('openruntime.accent') } catch (e) {}
        if (theme !== 'light' && theme !== 'dark') theme = 'system'
        if (['teal', 'indigo', 'plum', 'carbon'].indexOf(accent) < 0) accent = 'carbon'
        if (/\\/login\\/?$/.test(location.pathname)) accent = 'carbon'
        var dark = theme === 'dark' || (theme === 'system' && !!window.matchMedia && window.matchMedia('(prefers-color-scheme: dark)').matches)
        document.documentElement.classList.toggle('dark', dark)
        document.documentElement.setAttribute('data-theme', accent)
      })()
`;
