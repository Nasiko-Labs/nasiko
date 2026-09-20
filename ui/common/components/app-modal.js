/**
 * Accessible dialog modal with heading, body, footer slot, backdrop dismiss, and ESC/X close.
 *
 * @element app-modal
 * @attr {string} heading - Modal title shown in the header
 * @attr {boolean} hide-footer - Hides the footer container even when a footer slot exists
 * @method open() - Opens the modal (calls showModal on the internal dialog)
 * @method close() - Closes the modal
 * @slot default - Body content
 * @slot [data-slot="footer"] - Footer action row (flex-end, e.g. Cancel / Save buttons)
 * @note The internal <dialog> is a regular DOM child (no Shadow DOM). The `close` event fires
 *       on the internal <dialog> and does NOT bubble — listen on `el.querySelector('dialog')`.
 * @note Backdrop click and the X button are handled internally.
 */
import { icons } from "../utils/icons.js";
const styles = new CSSStyleSheet();
styles.replaceSync(`@scope (app-modal) {
    .app-modal {
      width: min(600px, calc(100% - 2 * var(--space-lg)));
      overflow: hidden;
    }
    /* Only flex-layout when open — preserves UA display:none when closed */
    dialog.app-modal[open] {
      display: flex;
      flex-direction: column;
    }
    header {
      flex-shrink: 0;
      display: flex;
      align-items: center;
      justify-content: space-between;
      gap: var(--space-sm);
      padding: var(--space-sm) var(--space-md);
      border-bottom: 1px solid var(--color-border);
    }
    /* h4 default has margin — reset it */
    .title { margin: 0; font-size: var(--font-size-sm); font-weight: 600; }
    header > button {
      border: 1px solid var(--color-border);
      border-radius: var(--radius-md);
      width: var(--control-h-sm);
      height: var(--control-h-sm);
      flex-shrink: 0;
      &:hover { border-color: var(--color-primary); background: var(--color-bg-base); }
    }
    .body { padding: var(--space-md); overflow-y: auto; flex: 1; max-height: 65vh; min-height: 6rem; }
    footer {
      flex-shrink: 0;
      display: flex;
      justify-content: flex-end;
      gap: var(--space-sm);
      flex-wrap: wrap;
      padding: var(--space-sm) var(--space-md);
      border-top: 1px solid var(--color-border);
    }
  }
`);
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

// global.css already handles: border:none, border-radius, padding:0, box-shadow,
// background, color, max-width:90vw, max-height:90vh, margin:auto, ::backdrop


const CLOSE = icons.x("", 14);

export class AppModal extends HTMLElement {
  #dialog;
  #footer = null;
  #initialized = false;

  static get observedAttributes() {
    return ["heading", "hide-footer"];
  }

  attributeChangedCallback(name, oldValue, newValue) {
    if (name === "heading" && this.#dialog) {
      const titleEl = this.#dialog.querySelector("header > .title");
      if (titleEl) {
        titleEl.textContent = newValue || "";
      }
    }
    if (name === "hide-footer") this.#syncFooterVisibility();
  }

  connectedCallback() {
    if (this.#initialized) return;
    this.#initialized = true;

    const footer = [...this.children].find(
      (el) => el.dataset.slot === "footer",
    );
    const bodyNodes = [...this.childNodes].filter((n) => n !== footer);

    const dialog = document.createElement("dialog");
    dialog.className = "app-modal";
    dialog.innerHTML = `
      <header>
        <h4 class="title"></h4>
        <button type="button" aria-label="Close">${CLOSE}</button>
      </header>
      <div class="body"></div>
      ${footer ? "<footer></footer>" : ""}`;

    const titleEl = dialog.querySelector("header > .title");
    if (titleEl) {
      titleEl.textContent = this.getAttribute("heading") || "";
    }

    bodyNodes.forEach((n) => dialog.querySelector(".body").appendChild(n));
    if (footer) {
      this.#footer = dialog.querySelector("footer");
      this.#footer.appendChild(footer);
    }

    dialog
      .querySelector("header > button")
      .addEventListener("click", () => dialog.close());
    dialog.addEventListener("click", (e) => {
      if (e.target === dialog) dialog.close();
    });

    this.appendChild(dialog);
    this.#dialog = dialog;
    this.#syncFooterVisibility();
  }

  #syncFooterVisibility() {
    if (!this.#footer) return;
    this.#footer.hidden = this.hasAttribute("hide-footer");
  }

  open() {
    this.#dialog?.showModal();
  }
  close() {
    this.#dialog?.close();
  }
}
if (!customElements.get("app-modal")) customElements.define("app-modal", AppModal);
