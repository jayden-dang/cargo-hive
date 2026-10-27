"""Compare same-named artifacts between two target dirs; classify by workspace vs registry."""
import os, sys, filecmp, json, re, collections
A, B, meta = sys.argv[1], sys.argv[2], sys.argv[3]
m = json.load(open(meta))
ws = {p['name'].replace('-', '_') for p in m['packages'] if p['id'] in set(m['workspace_members'])}
c = collections.Counter(); sizes = collections.Counter(); bad = []
for f in sorted(os.listdir(A)):
    if not f.endswith(('.rlib', '.rmeta', '.dylib')) or not os.path.exists(os.path.join(B, f)):
        continue
    name = re.sub(r'^lib', '', f).rsplit('-', 1)[0]
    kind = ('ws' if name in ws else 'reg') + '/' + f.rsplit('.', 1)[1]
    same = filecmp.cmp(os.path.join(A, f), os.path.join(B, f), shallow=False)
    c[(kind, same)] += 1; sizes[(kind, same)] += os.path.getsize(os.path.join(A, f))
    if not same: bad.append(f)
for k in sorted(c): print(f"{k[0]:12} {'IDENTICAL' if k[1] else 'DIFFERENT':9} {c[k]:4} files {sizes[k]/1e9:6.2f} GB")
print('different:', bad[:15])
