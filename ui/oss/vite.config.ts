import { dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
import { defineConfig } from 'vite'
import { nasikoApp } from '../vite.shared.ts'

export default defineConfig(({ mode }) =>
  nasikoApp({ id: 'oss', appDir: dirname(fileURLToPath(import.meta.url)), mode }),
)
