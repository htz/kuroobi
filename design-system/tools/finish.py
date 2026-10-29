# bundle.raw.js -> project/components/bundle.js (+header), base.css -> bundle.css
import re, json, os, subprocess
HERE = os.path.dirname(os.path.abspath(__file__))
GUI = os.path.join(HERE, '..', '..', 'gui')
OUT = os.path.join(HERE, '..', 'project', 'components')
os.chdir(HERE)
code = open('bundle.raw.js').read().replace('<!--', '\\x3C!--')
assert '</script' not in code.lower()
names = json.loads(subprocess.check_output(['node','-e',"""
global.window=global; window.React=require(process.argv[1]); global.navigator={languages:['ja'],language:'ja'}; global.localStorage={getItem(){return null},setItem(){}};
require('vm').runInThisContext(require('fs').readFileSync('bundle.raw.js','utf8'));
console.log(JSON.stringify(Object.keys(KuroobiUI).filter(k=>/^[A-Z][a-z]/.test(k)&&typeof KuroobiUI[k]==='function')))""", os.path.join(GUI, 'node_modules', 'react')]))
hdr = {"format":4,"namespace":"KuroobiUI","components":[{"name":n} for n in names]}
open(os.path.join(OUT, 'bundle.js'),'w').write('/* @ds-bundle: ' + json.dumps(hdr) + ' */\n' + code)
css = open(os.path.join(GUI, 'src-ui', 'base.css')).read()
# The app frame rules (fill the window, no page scroll) do not belong in a preview frame.
css = css.replace('html, body, #root { height:100%; }\n', '')
css = re.sub(r'\n  overflow:hidden;[^\n]*\n', '\n', css, count=1)
open(os.path.join(OUT, 'bundle.css'),'w').write('/* Kuroobi base — gui/src-ui/base.css (frame rules removed) */\n' + css)
print(len(names), 'components')
