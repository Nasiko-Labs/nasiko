import { clsx, type ClassValue } from 'clsx'
import { extendTailwindMerge } from 'tailwind-merge'

/** tailwind-merge only knows Tailwind's default scales: without our `@theme` names (index.css), `max-w-sheet` lost to
 *  the sheet primitive's `sm:max-w-sm`, and `text-code` / `text-lead` read as colours and dropped beside one. */
const twMerge = extendTailwindMerge({
  extend: {
    theme: {
      container: ['page', 'sheet', 'sheet-sm', 'sheet-lg'],
      text: ['code', 'lead'],
    },
  },
})

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs))
}

/**
 * A v4 UUID. `crypto.randomUUID` exists only in secure contexts (HTTPS or localhost), and OSS nasiko-server
 * also serves plain HTTP (TODOS "chat ids over plain HTTP"); `getRandomValues` works everywhere.
 */
export function uuid(): string {
  if (typeof crypto.randomUUID === 'function') return crypto.randomUUID()
  const b = crypto.getRandomValues(new Uint8Array(16))
  b[6] = (b[6] & 0x0f) | 0x40
  b[8] = (b[8] & 0x3f) | 0x80
  const h = [...b].map((x) => x.toString(16).padStart(2, '0')).join('')
  return `${h.slice(0, 8)}-${h.slice(8, 12)}-${h.slice(12, 16)}-${h.slice(16, 20)}-${h.slice(20)}`
}
