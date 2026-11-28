// @ts-check
import { createHash } from 'node:crypto';
import { existsSync, readFileSync } from 'node:fs';
import mdx from '@astrojs/mdx';
import react from '@astrojs/react';
import { defineConfig, fontProviders } from 'astro/config';

/** The hashes of the two wasm scripts/engine.sh put in public/engine/; the model is deployed apart (deploy-model.yml). */
function engineFiles() {
  /** @param {string} name */
  const hash = (name) => {
    const path = new URL('./public/engine/' + name, import.meta.url);
    const wasm = existsSync(path) ? readFileSync(path) : Buffer.alloc(0);
    return JSON.stringify(createHash('sha256').update(wasm).digest('hex').slice(0, 12));
  };
  return { __WASM_HASH__: hash('kuroobi_ai.wasm'), __WASM_MT_HASH__: hash('kuroobi_ai_mt.wasm') };
}

/** The threaded engine shares memory among workers, which a browser allows only on a cross-origin isolated page. */
const ISOLATED = { 'Cross-Origin-Opener-Policy': 'same-origin', 'Cross-Origin-Embedder-Policy': 'require-corp' };

export default defineConfig({
  integrations: [react(), mdx()],
  trailingSlash: 'always',
  markdown: {
    shikiConfig: { themes: { light: 'github-light', dark: 'github-dark' }, defaultColor: false },
  },
  i18n: {
    defaultLocale: 'ja',
    locales: ['ja', 'en'],
    routing: { prefixDefaultLocale: false },
  },
  fonts: [
    {
      provider: fontProviders.google(),
      name: 'Noto Sans JP',
      cssVariable: '--font-jp',
      weights: [400, 500, 700],
      fallbacks: ['sans-serif'],
    },
    {
      provider: fontProviders.google(),
      name: 'JetBrains Mono',
      cssVariable: '--font-mono',
      weights: [400, 500],
      fallbacks: ['monospace'],
    },
  ],
  // The colours, board and stones are the app's own tokens.css, read from crates/gui.
  vite: { server: { fs: { allow: ['..'] }, headers: ISOLATED }, preview: { headers: ISOLATED }, define: engineFiles() },
});
