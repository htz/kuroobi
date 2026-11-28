import { rolldown } from '../../../../crates/gui/node_modules/rolldown/dist/index.mjs';
const b = await rolldown({ input: new URL('./react-entry.js', import.meta.url).pathname,
  resolve: { modules: [new URL('../../../../crates/gui/node_modules', import.meta.url).pathname] },
  transform: { define: { 'process.env.NODE_ENV': '"production"' } } });
await b.write({ file: new URL('./react.js', import.meta.url).pathname, format: 'iife', minify: true });
console.log('ok');
