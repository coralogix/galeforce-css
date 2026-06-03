import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import galeforce from '@cx/vite-plugin-galeforcecss'

export default defineConfig({
  // Drop-in: GaleforceCSS reads tailwind.config.js automatically and
  // processes @tailwind / @apply / theme() in every CSS file Vite
  // touches. No `import 'virtual:galeforcecss.css'` needed.
  plugins: [react(), galeforce()],
})
