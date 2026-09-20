/**
 * Theme preference — "light" | "dark" | "system".
 *
 * global.css declares `color-scheme: light dark`, so with no stored choice the
 * OS preference wins. An explicit choice is persisted in localStorage and
 * pinned as `data-theme` on <html> (`:root[data-theme=…]` in global.css).
 *
 * Importing this module applies the stored choice as a side effect, so any
 * page that loads a component importing it (app-header, login-page) renders
 * with the pinned theme.
 */
const STORAGE_KEY = 'app-theme';

export function getTheme() {
  const stored = localStorage.getItem(STORAGE_KEY);
  return stored === 'light' || stored === 'dark' ? stored : 'system';
}

export function setTheme(theme) {
  if (theme === 'light' || theme === 'dark') {
    localStorage.setItem(STORAGE_KEY, theme);
    document.documentElement.dataset.theme = theme;
  } else {
    localStorage.removeItem(STORAGE_KEY);
    delete document.documentElement.dataset.theme;
  }
}

setTheme(getTheme());
