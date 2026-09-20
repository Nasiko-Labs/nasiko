/**
 * Sliding active-indicator for ad-hoc tab strips and segmented controls
 * (`app-tabs` owns its own equivalent). Appends an absolutely positioned
 * marker to the strip and keeps it glued to the active item, animating
 * position/size changes instead of jumping. Respects reduced motion.
 *
 * The marker replaces the static active underline/fill, so each surface's
 * CSS must suppress its own active style under `.has-tab-indicator` (and,
 * for pills, give `.tab-indicator` the matching background/radius) so the
 * at-rest look is unchanged.
 *
 * Survives full innerHTML re-renders of the strip: the marker is re-appended
 * with its previous geometry and slides to the new active item.
 */
const BAR_HEIGHT = 2;

const styles = new CSSStyleSheet();
styles.replaceSync(`
  .has-tab-indicator { position: relative; }
  .tab-indicator {
    position: absolute;
    left: 0;
    top: 0;
    pointer-events: none;
    transition:
      transform 200ms cubic-bezier(0.2, 0, 0, 1),
      width 200ms cubic-bezier(0.2, 0, 0, 1),
      height 200ms cubic-bezier(0.2, 0, 0, 1);
  }
  .tab-indicator--bar {
    height: ${BAR_HEIGHT}px;
    background: var(--fg-brand-highlight);
  }
  .tab-indicator--pill { z-index: 0; }
  @media (prefers-reduced-motion: reduce) {
    .tab-indicator { transition: none; }
  }
`);
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

/**
 * @param {HTMLElement} container - the strip; gains `has-tab-indicator`
 * @param {string} itemSelector - selects the tab items (e.g. ".type-tab")
 * @param {string} activeSelector - compound suffix marking the active item
 *   (e.g. ".active", ".is-active", ":has(input:checked)")
 * @param {{ pill?: boolean }} [opts] - pill mode tracks the item's full box
 *   (segmented controls); default is a 2px underline bar over the item's
 *   own bottom border
 */
export function attachSlidingIndicator(
  container,
  itemSelector,
  activeSelector,
  { pill = false } = {},
) {
  const indicator = document.createElement("span");
  indicator.className = `tab-indicator ${pill ? "tab-indicator--pill" : "tab-indicator--bar"}`;
  indicator.setAttribute("aria-hidden", "true");
  indicator.hidden = true;
  // Zero-size static twin of the indicator: its rect gives the exact
  // (0,0)-translate origin, so item positions can be measured as rect deltas.
  // offsetLeft/offsetTop can't be trusted here — fieldsets measure children
  // from the border box but anchor abs-positioned ones below the legend.
  const probe = document.createElement("span");
  probe.setAttribute("aria-hidden", "true");
  probe.style.cssText = "position:absolute;left:0;top:0;width:0;height:0;pointer-events:none;";
  container.classList.add("has-tab-indicator");
  container.append(probe, indicator);

  let placed = false;

  const moveTo = (item) => {
    const origin = probe.getBoundingClientRect();
    const rect = item.getBoundingClientRect();
    const x = rect.left - origin.left;
    const yTop = rect.top - origin.top;
    const y = pill ? yTop : yTop + rect.height - BAR_HEIGHT;
    indicator.style.width = `${rect.width}px`;
    if (pill) indicator.style.height = `${rect.height}px`;
    indicator.style.transform = `translate(${x}px, ${y}px)`;
  };

  const sync = (animate = true) => {
    const item = container.querySelector(itemSelector + activeSelector);
    if (!item) {
      // Strip is skeleton/empty — hide and forget so the next placement snaps.
      indicator.hidden = true;
      placed = false;
    } else {
      if (indicator.parentElement !== container) container.append(probe, indicator);
      indicator.hidden = false;
      if (!placed || !animate) {
        indicator.style.transition = "none";
        moveTo(item);
        void indicator.offsetWidth; // commit geometry before re-enabling
        indicator.style.transition = "";
        placed = true;
      } else {
        void indicator.offsetWidth; // after a re-append: settle the old geometry so the move animates
        moveTo(item);
      }
    }
    observer.takeRecords(); // drop the mutations we just made
  };

  // Covers in-place class toggles and full strip re-renders alike.
  const observer = new MutationObserver(() => sync());
  observer.observe(container, {
    subtree: true,
    childList: true,
    attributes: true,
    attributeFilter: ["class", "aria-selected"],
  });
  // Radio-backed segments (:checked flips without any attribute mutation).
  container.addEventListener("change", () => sync());
  new ResizeObserver(() => sync(false)).observe(container);
  document.fonts?.ready.then(() => sync(false));
  sync(false);
}
