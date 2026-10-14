// The part of ./stale-build.ts that must not import anything: the build reads it from the Vite config to emit
// `/nasiko-boot-guard.js` (shared/config/vite.config.ts), as it does the theme pre-paint script.

/** sessionStorage key: the times (ms since epoch, comma-separated) of recent automatic reloads. Shared with the boot guard. */
export const RELOAD_KEY = "nasiko:stale-build-reload";
/** The window in which automatic reloads are counted. */
export const RELOAD_WINDOW_MS = 60_000;
/**
 * At most this many automatic reloads per window. Two, not one: an upgrade followed by a quick rollback is two real
 * transitions; a rollout that keeps flipping between versions (or a server that keeps failing) stops after two.
 */
export const MAX_RELOADS = 2;

/** The automatic reloads recorded within the window (a malformed or absent marker counts as none). */
export function recentReloads(marker: string | null, now: number): number[] {
  return (marker ?? "")
    .split(",")
    .map(Number)
    .filter((at) => Number.isFinite(at) && at > 0 && now - at < RELOAD_WINDOW_MS);
}
/**
 * The boot guard: a classic script the build emits as `/nasiko-boot-guard.js` (not hashed, revalidated like the
 * shell) and the page loads before the entry module. If a script or stylesheet under `/assets/` fails to load and
 * the app has not mounted, it reloads once (same marker and window as above) and otherwise shows a plain message with
 * a Reload button. Offline, it shows that instead and loads again when the connection is back. Nothing has mounted,
 * so there is no unsaved work on this page to protect. It removes itself once
 * the app has rendered.
 */
export const BOOT_GUARD_SCRIPT = `
(function () {
  var KEY = ${JSON.stringify(RELOAD_KEY)}, WINDOW = ${String(RELOAD_WINDOW_MS)}, MAX = ${String(MAX_RELOADS)}, failed = false
  function mounted() { var root = document.getElementById('root'); return !!(root && root.firstElementChild) }
  function recent(now) {
    try {
      return String(sessionStorage.getItem(KEY) || '').split(',').map(Number)
        .filter(function (at) { return at > 0 && now - at < WINDOW })
    } catch (e) { return null }
  }
  function fallback(message) {
    var root = document.getElementById('root')
    if (!root || mounted()) return
    var box = document.createElement('div'), p = document.createElement('p'), b = document.createElement('button')
    box.setAttribute('role', 'alert'); box.setAttribute('data-boot-error', '')
    box.style.cssText = 'font: 15px/1.5 system-ui, sans-serif; max-width: 32rem; margin: 20vh auto; padding: 0 1rem'
    p.textContent = message || 'Nasiko could not load. It may have just been updated.'
    b.type = 'button'; b.textContent = 'Reload'
    b.addEventListener('click', function () { location.reload() })
    box.appendChild(p); box.appendChild(b); root.appendChild(box)
  }
  function onError(event) {
    var el = event.target, url = el && (el.src || el.href)
    if (failed || mounted() || !url || String(url).indexOf('/assets/') < 0) return
    if (el.tagName !== 'SCRIPT' && el.tagName !== 'LINK') return
    failed = true
    // Offline is not a new version: wait for the connection, then load again (nothing has mounted, nothing to lose).
    if (navigator.onLine === false) {
      window.addEventListener('online', function () { location.reload() }, { once: true })
      var offline = function () { fallback("You're offline. Nasiko will load when the connection is back.") }
      if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', offline)
      else offline()
      return
    }
    var now = Date.now(), reloads = recent(now)
    if (reloads && reloads.length < MAX) {
      try { sessionStorage.setItem(KEY, reloads.concat([now]).join(',')); location.reload(); return } catch (e) {}
    }
    if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', function () { fallback() })
    else fallback()
  }
  window.addEventListener('error', onError, true)
  window.addEventListener('load', function () { setTimeout(function () { if (mounted()) window.removeEventListener('error', onError, true) }, 0) })
})()
`;
