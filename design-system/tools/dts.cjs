// Concatenate component declarations into one index.d.ts, add <Comp>Props aliases, and pull in
// the types they reference from the app's other modules.
const path = require('path');
const ts = require(path.join(__dirname, '../../gui/node_modules/typescript'));
const fs = require('fs');
process.chdir(__dirname);
const parse = (f) => ts.createSourceFile(f, fs.readFileSync(f, 'utf8'), ts.ScriptTarget.Latest, true);
const declName = (n) => (ts.isTypeAliasDeclaration(n) || ts.isInterfaceDeclaration(n) || ts.isEnumDeclaration(n)) && n.name.text;
const dir = 'dts/components/';
let out = '// Kuroobi UI components (window.KuroobiUI), generated from gui/src-ui/js/components.\n';
const declared = new Set(), props = [], bodies = [];
for (const f of fs.readdirSync(dir).sort()) {
  const sf = parse(dir + f);
  sf.forEachChild((n) => { const d = declName(n); if (d) declared.add(d); });
  sf.forEachChild((n) => {
    if (ts.isFunctionDeclaration(n) && n.name && /^[A-Z]/.test(n.name.text) && n.parameters[0]?.type) {
      const name = n.name.text + 'Props';
      if (declared.has(name)) return;
      const tp = n.typeParameters ? '<' + n.typeParameters.map((t) => t.getText(sf) + (t.default ? '' : ' = string')).join(', ') + '>' : '';
      props.push(`export type ${name}${tp} = ${n.parameters[0].type.getText(sf)};`);
    }
  });
  const text = sf.getFullText().split('\n').filter((l) => !/^import /.test(l) && !/^export \{\};?$/.test(l)).join('\n');
  bodies.push(`\n// ---- ${f.replace('.d.ts', '.tsx')} ----\n` + text);
}
out += bodies.join('') + '\n// ---- props ----\n' + props.join('\n') + '\n';
// Types referenced but not declared: take them from the app's other declarations, transitively.
const pool = new Map();
for (const f of ['dts/ggs.d.ts', 'dts/state.d.ts', 'dts/types.d.ts', 'dts/api.d.ts', 'dts/i18n.d.ts']) {
  if (!fs.existsSync(f)) continue;
  const sf = parse(f);
  sf.forEachChild((n) => {
    const d = declName(n);
    if (d && !pool.has(d)) pool.set(d, n.getText(sf));
    if (ts.isVariableStatement(n)) for (const v of n.declarationList.declarations) {
      if (ts.isIdentifier(v.name) && !pool.has(v.name.text)) pool.set(v.name.text, n.getText(sf));
    }
  });
}
const extra = [];
const refs = (s) => [...s.matchAll(/\b([A-Z][A-Za-z0-9_]*)\b/g)].map((m) => m[1]);
let work = refs(out);
while (work.length) {
  const name = work.pop();
  if (declared.has(name) || !pool.has(name)) continue;
  declared.add(name);
  const text = pool.get(name).replace(/^export declare /, 'export ').replace(/^(type|interface) /, 'export $1 ');
  extra.push(text.startsWith('export') ? text : 'export ' + text);
  work.push(...refs(text));
}
out += '\n// ---- types the components use (gui/src-ui/js/ggs.ts, state.ts, types.ts) ----\n' + extra.join('\n') + '\n';
fs.writeFileSync('../project/components/index.d.ts', out);
console.log(props.length, 'props,', extra.length, 'pulled types:', extra.map((e) => e.match(/(?:type|interface|enum) (\w+)/)?.[1]).join(' '));
