// @ts-check
import { createHash } from 'node:crypto';
import { existsSync, readFileSync } from 'node:fs';
import mdx from '@astrojs/mdx';
import react from '@astrojs/react';
import { defineConfig, fontProviders } from 'astro/config';

/** The hash of the wasm scripts/engine.sh put in public/engine/; the model is deployed apart (deploy-model.yml). */
function engineFiles() {
  const path = new URL('./public/engine/kuroobi_ai.wasm', import.meta.url);
  const wasm = existsSync(path) ? readFileSync(path) : Buffer.alloc(0);
  return { __WASM_HASH__: JSON.stringify(createHash('sha256').update(wasm).digest('hex').slice(0, 12)) };
}

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
  vite: { server: { fs: { allow: ['..'] } }, define: engineFiles() },
});
