import react from '@vitejs/plugin-react'
import { defineConfig } from 'vite'

// `npm run dev` serves the page with live reload, its API from a `prw web` already running:
//   prw web --demo --cache companion/demo --port 7420 --no-open
// then open http://localhost:5173/#k=<the key it printed>. prw web takes only its own host
// and origin, so the proxy says it is the page.
const PRW = 'http://127.0.0.1:7420'

export default defineConfig({
  plugins: [react()],
  server: {
    proxy: {
      '/api': { target: PRW, changeOrigin: true, headers: { origin: PRW } },
    },
  },
  build: {
    // The binary embeds this folder (rust-embed); it is committed, so building prw needs no npm.
    outDir: 'dist',
    emptyOutDir: true,
    // One script and one stylesheet; fonts as files beside them.
    assetsInlineLimit: 0,
  },
})
