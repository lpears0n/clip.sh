import { resolve } from 'node:path'
import { svelte } from '@sveltejs/vite-plugin-svelte'
import tailwindcss from '@tailwindcss/vite'
import { defineConfig } from 'vite'

// Tauri expects a fixed port and must see Rust-side errors in the terminal.
export default defineConfig({
  // Component styles are injected from JS rather than served as virtual CSS
  // modules: a webview that reloads against a fresh dev server can request
  // those modules before they exist, and the page then fails to load.
  plugins: [tailwindcss(), svelte({ emitCss: false })],
  clearScreen: false,
  // Two pages: the app, and the in-game overlay popup (its own window).
  build: {
    rollupOptions: {
      input: {
        main: resolve(import.meta.dirname, 'index.html'),
        overlay: resolve(import.meta.dirname, 'overlay.html'),
      },
    },
  },
  server: {
    port: 5173,
    strictPort: true,
    watch: { ignored: ['**/src-tauri/**'] },
  },
})
