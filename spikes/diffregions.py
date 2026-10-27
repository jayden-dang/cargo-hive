"""Show where two same-size-ish binaries differ, with printable context."""
import sys, re
a = open(sys.argv[1], 'rb').read(); b = open(sys.argv[2], 'rb').read()
print('sizes', len(a), len(b))
n = min(len(a), len(b)); d = [i for i in range(n) if a[i] != b[i]]
print('differing bytes', len(d) + abs(len(a) - len(b)))
runs = []
for i in d:
    if runs and i <= runs[-1][1] + 32: runs[-1][1] = i
    else: runs.append([i, i])
print('regions', len(runs))
pr = lambda s: re.sub(rb'[^\x20-\x7e]', b'.', s).decode()
for s, e in runs[:int(sys.argv[3]) if len(sys.argv) > 3 else 6]:
    print(f'@{s}+{e-s+1}: A[{pr(a[max(0,s-48):e+24])}]')
    print(f'{"":>{len(str(s))+3}}  B[{pr(b[max(0,s-48):e+24])}]')
