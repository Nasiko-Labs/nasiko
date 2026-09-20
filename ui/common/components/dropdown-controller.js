/**
 * Shared dropdown controller for autocomplete-style components.
 *
 * @note Not a custom element — import and instantiate as a helper class.
 * @note Handles: positioning, open/close ARIA state, keyboard navigation,
 *       item highlighting, and click/hover binding.
 * @note Usage:
 *   const dd = new DropdownController(ulEl, anchorEl, '.my-option');
 *   dd.bindItems(count, () => this.#selectOption());
 *   dd.open();
 */
export class DropdownController {
  #el;       // the <ul> dropdown element
  #anchor;   // the element used for positioning (input / contenteditable div)
  #optSel;   // CSS selector for individual option <li> elements, e.g. '.ac-option'
  #selIdx = -1;
  #isOpen = false;
  #count  = 0;

  constructor(el, anchor, optSel) {
    this.#el     = el;
    this.#anchor = anchor;
    this.#optSel = optSel;
  }

  get isOpen() { return this.#isOpen; }
  get selIdx()  { return this.#selIdx; }

  open() {
    if (this.#isOpen) return;
    const r = this.#anchor.getBoundingClientRect();
    // Flip upward when the anchor sits near the fold — otherwise the fixed
    // dropdown extends past the viewport bottom and can't be seen.
    const roomBelow = window.innerHeight - r.bottom - 8;
    if (roomBelow < 160 && r.top > window.innerHeight - r.bottom) {
      this.#el.style.top    = 'auto';
      this.#el.style.bottom = `${window.innerHeight - r.top + 4}px`;
    } else {
      this.#el.style.bottom = 'auto';
      this.#el.style.top    = `${r.bottom + 4}px`;
    }
    this.#el.style.left  = `${r.left}px`;
    this.#el.style.width = `${r.width}px`;
    this.#el.classList.remove('hidden');
    this.#el.setAttribute('aria-hidden', 'false');
    this.#anchor.setAttribute('aria-expanded', 'true');
    this.#isOpen = true;
  }

  close() {
    if (!this.#isOpen) return;
    this.#el.classList.add('hidden');
    this.#el.setAttribute('aria-hidden', 'true');
    this.#anchor.setAttribute('aria-expanded', 'false');
    this.#isOpen = false;
    this.#selIdx = -1;
  }

  /** Wrap-around keyboard navigation. */
  navigate(dir) {
    if (!this.#count) return;
    this.#selIdx = (this.#selIdx + dir + this.#count) % this.#count;
    this.#highlight();
  }

  /** Set selected index and highlight it. */
  setIndex(i) {
    this.#selIdx = i;
    this.#highlight();
  }

  /**
   * Attach click/hover listeners after fresh HTML has been rendered into the
   * dropdown element. Resets the selection index to -1.
   * @param {number}   count    Number of rendered option items.
   * @param {function} onSelect Called when the user clicks an option (after index is set).
   */
  bindItems(count, onSelect) {
    this.#count  = count;
    this.#selIdx = -1;
    this.#el.querySelectorAll(this.#optSel).forEach((el, i) => {
      el.addEventListener('click',     () => { this.setIndex(i); onSelect(); });
      el.addEventListener('mouseover', () => this.setIndex(i));
    });
  }

  #highlight() {
    this.#el.querySelectorAll(this.#optSel).forEach((el, i) => {
      const active = i === this.#selIdx;
      el.classList.toggle('selected', active);
      el.setAttribute('aria-selected', String(active));
      if (active) el.scrollIntoView({ block: 'nearest' });
    });
  }
}
