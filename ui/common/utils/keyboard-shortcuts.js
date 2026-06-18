/**
 * keyboard-shortcuts.js
 *
 * Global keyboard shortcuts integration utility.
 * Auto-initializes the kb-shortcut-manager and registers default shortcuts.
 *
 * Usage:
 *   <script type="module" src="/assets/utils/keyboard-shortcuts.js"></script>
 *
 * Provides singleton access to the keyboard manager and convenience functions.
 */

import '../components/kb-shortcut-manager.js';
import '../components/kb-command-palette.js';

// Create and append the manager to the document
let keyboardManager = document.querySelector('kb-shortcut-manager');
if (!keyboardManager) {
  keyboardManager = document.createElement('kb-shortcut-manager');
  document.body.appendChild(keyboardManager);
}

// Create and append the command palette
let commandPalette = document.querySelector('kb-command-palette');
if (!commandPalette) {
  commandPalette = document.createElement('kb-command-palette');
  document.body.appendChild(commandPalette);
}

// Wait for components to be defined
await customElements.whenDefined('kb-shortcut-manager');
await customElements.whenDefined('kb-command-palette');

// Register default shortcuts
const defaultShortcuts = [
  {
    id: 'open-command-palette',
    key: '?',
    handler: () => keyboardManager.openCommandPalette(),
    description: 'Open command palette',
    category: 'Navigation',
    contexts: ['global'],
    priority: 100,
    allowInInput: false
  },
  {
    id: 'close-command-palette',
    key: 'Escape',
    handler: () => {
      const palette = document.querySelector('kb-command-palette');
      if (palette && palette.isOpen()) {
        palette.close();
      }
    },
    description: 'Close command palette',
    category: 'Navigation',
    contexts: ['global'],
    priority: 100,
    allowInInput: false
  }
];

// Register each default shortcut
for (const shortcut of defaultShortcuts) {
  keyboardManager.registerShortcut(shortcut);
}

/**
 * Register a keyboard shortcut (convenience wrapper)
 * @param {Object} definition - Shortcut definition
 * @returns {string} Shortcut ID
 */
export function registerShortcut(definition) {
  return keyboardManager.registerShortcut(definition);
}

/**
 * Unregister a keyboard shortcut (convenience wrapper)
 * @param {string} id - Shortcut ID
 * @returns {boolean} True if shortcut was found and removed
 */
export function unregisterShortcut(id) {
  return keyboardManager.unregisterShortcut(id);
}

/**
 * Enable a shortcut (convenience wrapper)
 * @param {string} id - Shortcut ID
 */
export function enableShortcut(id) {
  keyboardManager.enable(id);
}

/**
 * Disable a shortcut (convenience wrapper)
 * @param {string} id - Shortcut ID
 */
export function disableShortcut(id) {
  keyboardManager.disable(id);
}

/**
 * Enter a context (convenience wrapper)
 * @param {string} context - Context name
 */
export function enterContext(context) {
  keyboardManager.enterContext(context);
}

/**
 * Exit a context (convenience wrapper)
 * @param {string} context - Context name
 */
export function exitContext(context) {
  keyboardManager.exitContext(context);
}

/**
 * Get all shortcuts (convenience wrapper)
 * @param {Object} [filters] - Optional filters
 * @returns {Array} Array of shortcut definitions
 */
export function getAllShortcuts(filters) {
  return keyboardManager.getAllShortcuts(filters);
}

/**
 * Open command palette (convenience wrapper)
 */
export function openCommandPalette() {
  keyboardManager.openCommandPalette();
}

// Export singleton instances
export { keyboardManager, commandPalette };

// Log initialization
console.log('Keyboard shortcuts system initialized');
