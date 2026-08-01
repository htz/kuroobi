import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

// Tauri から読む静的サイトを ui/ に出力する。
// (tauri.conf.json の frontendDist が ui/ を指しているため)
export default defineConfig({
  plugins: [react()],
  root: 'src-ui',
  build: {
    outDir: '../ui',
    emptyOutDir: true,
    target: 'safari16',       // Tauri の WebView (WebKit) に合わせる
    sourcemap: true,
  },
  server: { port: 5184, strictPort: true },
});
