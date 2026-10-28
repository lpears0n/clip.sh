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
  server: {
    port: 5173,
    strictPort: true,
    watch: { ignored: ['**/src-tauri/**'] },
  },
})
