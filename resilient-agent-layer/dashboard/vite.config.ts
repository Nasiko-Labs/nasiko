import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

export default defineConfig({
  plugins: [react()],
  server: {
    port: 3000,
    proxy: {
      '/admin': 'http://localhost:8000',
      '/invoke': 'http://localhost:8000',
      '/health': 'http://localhost:8000',
    }
  }
})
