/**
 * @note Not a custom element — abstract base class for layout custom elements.
 * @note Extend `BaseLayout` and pass a CSS prefix string to `super(prefix)`.
 *       Observed attributes are automatically applied via `updateProperty` on connect.
 */
export class BaseLayout extends HTMLElement {
  #prefix;

  constructor(prefix) {
    super();
    this.#prefix = prefix;
  }

  connectedCallback() {
    const observed = this.constructor.observedAttributes || [];
    for (const attr of observed) {
      const val = this.getAttribute(attr);
      if (val !== null) this.updateProperty(attr, val);
    }
  }

  attributeChangedCallback(name, oldVal, newVal) {
    if (this.isConnected) {
      if (newVal !== null) {
        this.updateProperty(name, newVal);
      } else {
        this.style.removeProperty(`--${this.#prefix}-${name}`);
      }
    }
  }

  updateProperty(name, value) {
    this.style.setProperty(`--${this.#prefix}-${name}`, value);
  }
}
