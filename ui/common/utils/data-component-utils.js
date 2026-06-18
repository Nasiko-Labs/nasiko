/**
 * Shared utilities for data-fetching components (smart-table, data-view).
 * Reduces duplication for event tracking, debounce, and loading animation.
 */

/**
 * Creates an event-listener tracker that simplifies add + cleanup.
 *
 *   const events = createEventTracker();
 *   events.add(button, 'click', handler);
 *   events.add(button, 'click', handler2, { _tag: true }); // extra metadata
 *   events.removeTagged('_tag');   // remove only tagged entries
 *   events.cleanup();              // remove all
 */
export function createEventTracker() {
  let entries = [];

  return {
    add(element, event, handler, meta) {
      element.addEventListener(event, handler);
      entries.push({ element, event, handler, ...meta });
    },

    removeTagged(tagKey) {
      entries = entries.filter(entry => {
        if (entry[tagKey]) {
          entry.element.removeEventListener(entry.event, entry.handler);
          return false;
        }
        return true;
      });
    },

    cleanup() {
      entries.forEach(({ element, event, handler }) => {
        element.removeEventListener(event, handler);
      });
      entries = [];
    },
  };
}

/**
 * Returns a debounced wrapper around `fn`.
 * Calling `.cancel()` on the returned function clears the pending timer.
 */
export function debounce(fn, delay = 300) {
  let timer = null;
  const debounced = (...args) => {
    if (timer) clearTimeout(timer);
    timer = setTimeout(() => fn(...args), delay);
  };
  debounced.cancel = () => { if (timer) clearTimeout(timer); timer = null; };
  return debounced;
}

/**
 * CSS string for a subtle opacity-pulse loading animation.
 * Pass the BEM class name that should trigger it (e.g. 'smart-table__scroll--loading').
 *
 * Allowed by AGENTS.md rule 7 exception for data-loading indicators.
 */
export function loadingPulseCSS(className) {
  return `
  @keyframes data-loading-pulse {
    0%, 100% { opacity: 0.45; }
    50%      { opacity: 0.75; }
  }
  .${className} {
    animation: data-loading-pulse 1.5s ease-in-out infinite;
    pointer-events: none;
    cursor: wait;
  }
`;
}
