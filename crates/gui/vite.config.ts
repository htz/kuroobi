import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import yaml from '@rollup/plugin-yaml';

// Emit the static site Tauri loads into ui/ (tauri.conf.json's
// frontendDist points there).
export default defineConfig({
  plugins: [react(), yaml()],
  root: 'src-ui',
  build: {
    outDir: '../ui',
    emptyOutDir: true,
    target: 'safari16',       // match Tauri's WebView (WebKit)
    sourcemap: true,
  },
  server: { port: 5184, strictPort: true },
});
