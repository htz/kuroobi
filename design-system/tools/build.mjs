// Bundle the GUI's components into one classic script reading window.React / window.ReactDOM.
import { rolldown } from '../../gui/node_modules/rolldown/dist/index.mjs';
import yaml from '../../gui/node_modules/js-yaml/dist/js-yaml.mjs';
import { readFileSync, writeFileSync } from 'node:fs';
import { createRequire } from 'node:module';

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
  api: `const none = () => Promise.reject(new Error('no backend in the design system'));
    const stub = new Proxy({}, { get: () => none });
    export const api = stub, ggsApi = stub; export const jsLog = () => {}, emitApp = () => {};
    export const onApp = async () => () => {}, onHints = async () => () => {}, onGgsSnapshot = async () => () => {};`,
};
const plugin = {
  name: 'ds',
  resolveId(id, importer) {
    if (id in shims && id !== 'api') return '\0shim:' + id;
    if (id === './api' && importer && importer.includes('/src-ui/js/')) return '\0shim:api';
    if (id.endsWith('?raw')) return { id: new URL(id.slice(0, -4), 'file://' + importer).pathname + '?raw' };
  },
  load(id) {
    if (id.startsWith('\0shim:')) return shims[id.slice(6)];
    if (id.endsWith('?raw')) return 'export default ' + JSON.stringify(readFileSync(id.slice(0, -4), 'utf8'));
    if (id.endsWith('.yaml')) return 'export default ' + JSON.stringify(yaml.load(readFileSync(id, 'utf8')));
  },
};
const bundle = await rolldown({ input: new URL('./entry.tsx', import.meta.url).pathname, plugins: [plugin],
  transform: { jsx: { runtime: 'automatic' } } });
const { output } = await bundle.generate({ format: 'iife', name: 'KuroobiUI', minify: true });
let code = output[0].code;
writeFileSync(new URL('./bundle.raw.js', import.meta.url), code);
console.log('bytes', code.length);
