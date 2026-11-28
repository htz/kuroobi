// Bundle the web's components into one classic script, window.KuroobiWeb, reading window.React, and the
// site's style layer into bundle.css: what the canvas (design/web) mounts.
import { rolldown } from '../../../web/node_modules/rolldown/dist/index.mjs';
import { readFileSync, writeFileSync } from 'node:fs';

const OUT = new URL('../project/components/', import.meta.url);
const HOOKS = ['useState','useEffect','useRef','useMemo','useCallback','useLayoutEffect','useSyncExternalStore','useContext','createContext','Fragment','createElement','forwardRef','memo','useId','useReducer'];
const shims = {
  react: `const R = window.React; export default R; export const { ${HOOKS.join(', ')} } = R;`,
  'react-dom': `export default window.ReactDOM;`,
  // jsxs gets a static children array; spread it so React does not ask for keys.
  'react/jsx-runtime': `const R = window.React;
    const withKey = (p, k) => (k === undefined ? p : { ...p, key: k });
    export const jsx = (t, p, k) => R.createElement(t, withKey(p, k));
    export const jsxs = (t, p, k) => { const { children, ...rest } = p; return R.createElement(t, withKey(rest, k), ...children); };
    export const Fragment = R.Fragment;`,
};
const plugin = {
  name: 'web-ds',
  resolveId(id) { if (id in shims) return '\0shim:' + id; },
  load(id) { if (id.startsWith('\0shim:')) return shims[id.slice(6)]; },
};
const bundle = await rolldown({ input: new URL('./entry.tsx', import.meta.url).pathname, plugins: [plugin],
  transform: { jsx: { runtime: 'automatic' } } });
const { output } = await bundle.generate({ format: 'iife', name: 'KuroobiWeb', minify: true });
const code = output[0].code.replace(/<!--/g, '\\x3C!--');
if (code.toLowerCase().includes('</script')) throw new Error('</script in the bundle');
globalThis.window = globalThis;
window.React = (await import('../../../web/node_modules/react/index.js')).default;
window.KuroobiWeb = new Function(code + ";return KuroobiWeb;")();
const names = Object.keys(window.KuroobiWeb).filter((k) => /^[A-Z][a-z]/.test(k) && typeof window.KuroobiWeb[k] === 'function');
writeFileSync(new URL('bundle.js', OUT), '/* @ds-bundle: ' + JSON.stringify({ format: 4, namespace: 'KuroobiWeb', components: names.map((name) => ({ name })) }) + ' */\n' + code);
// The colour tokens come from the canvas's own copy of the app's tokens.css; the fonts from its Google Fonts link.
const css = readFileSync(new URL('../../../web/src/styles/site.css', import.meta.url), 'utf8')
  .replace(/^@import [^\n]*\n/m, '')
  .replace('--ff-web: var(--font-jp),', '--ff-web: "Noto Sans JP",')
  .replace('--ff-code: var(--font-mono),', '--ff-code: "JetBrains Mono",');
writeFileSync(new URL('bundle.css', OUT), '/* Kuroobi web — web/src/styles/site.css */\n' + css);
console.log(names.length, 'components:', names.join(' '));
