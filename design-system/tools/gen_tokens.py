# tokens.css (Kuroobi GUI) -> tokens.json for the Design System artifact.
import re, json, os, sys
HERE = os.path.dirname(os.path.abspath(__file__))
os.chdir(HERE)
src = open(os.path.join(HERE, '..', '..', 'gui', 'src-ui', 'tokens.css')).read()
def block(sel):
    i = src.index(sel); j = src.index('{', i); depth=0
    for k in range(j, len(src)):
        if src[k]=='{': depth+=1
        elif src[k]=='}':
            depth-=1
            if depth==0: return src[j+1:k]
root = block(':root {')
light = block(':root[data-theme="light"] {')
def decls(b):
    out=[]; comment=''
    # walk declarations, keeping the nearest preceding/trailing comment as usage
    for m in re.finditer(r'/\*(.*?)\*/|--([A-Za-z0-9-]+)\s*:\s*([^;]+);', b, re.S):
        if m.group(1) is not None: comment=' '.join(m.group(1).split()); 
        else: out.append((m.group(2), m.group(3).strip(), comment))
    return out
R = decls(root); L = {n:v for n,v,_ in decls(light)}
# trailing comment on the same line overrides
def trailing(b):
    t={}
    for line in b.splitlines():
        names=re.findall(r'--([A-Za-z0-9-]+)\s*:', line)
        c=re.search(r'/\*(.*?)\*/', line)
        if names and c:
            for n in names: t[n]=' '.join(c.group(1).split())
    return t
T = trailing(root)
iscolor = lambda v: re.match(r'^(#[0-9a-fA-F]{3,8}|rgba?\([^()]*\))$', v)
color, shadow, spacing, radius, height, width, timing = [],[],[],[],[],[],[]
for n,v,c in R:
    use = T.get(n) or c
    if n.startswith('sh-'):
        shadow.append({"name":n,"value":{"dark":v,"light":L.get(n,v)},"usage":use})
    elif iscolor(v):
        val = {"dark": v.lower() if v.startswith('#') else v}
        if n in L: val["light"] = L[n].lower() if L[n].startswith('#') else L[n]
        color.append({"name":n,"value":val,"usage":use})
    elif n.startswith('sp-'): spacing.append({"name":n,"value":v,"usage":use})
    elif n.startswith('r-'): radius.append({"name":n,"value":v,"usage":use})
    elif n.startswith('h-'): height.append({"name":n,"value":v,"usage":use})
    elif n.startswith('w-'): width.append({"name":n,"value":v,"usage":use})
    elif n in ('dur','ease'): timing.append({"name":n,"value":v,"usage":use})
json.dump({"color":color,"shadow":shadow,"spacing":spacing,"radius":radius,"height":height,"width":width,"timing":timing},
          open('raw_tokens.json','w'), ensure_ascii=False, indent=1)
print(len(color), len(shadow), len(spacing), len(radius), len(height), len(width), len(timing))
missing=[n for n,v,c in R if not (n.startswith(('sh-','sp-','r-','h-','w-','ff','fs-')) or iscolor(v) or n in('dur','ease'))]
print('unplaced:', missing)
