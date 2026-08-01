import { defineConfig } from 'vite';

// Tauri から読む静的サイトを ui/ に出力する。
// (tauri.conf.json の frontendDist が ui/ を指しているため)
export default defineConfig({
  root: 'src-ui',
  build: {
    outDir: '../ui',
    emptyOutDir: true,
    target: 'safari16',       // Tauri の WebView (WebKit) に合わせる
    sourcemap: true,
  },
  server: { port: 5184, strictPort: true },
});
