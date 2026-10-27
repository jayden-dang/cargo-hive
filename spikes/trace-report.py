"""Compare files a traced rustc opened against what its dep-info declared."""
import glob, os, collections, sys
crate, root = sys.argv[1], os.path.realpath(sys.argv[2])
tr = [l.rstrip('\n').split('\t', 1) for l in open(f'/tmp/hive/trace-{crate}.txt')]
reads = {os.path.realpath(p) for k, p in tr if k == 'read' and os.path.isfile(p)}
d = max(glob.glob(f'{root}/backend/target/debug/deps/{crate}-*.d'), key=os.path.getmtime)
declared = set()
for line in open(d):
    if ':' in line and not line.startswith('#'):
        for p in line.split(':', 1)[1].replace('\\ ', '\0').split():
            declared.add(os.path.realpath(p.replace('\0', ' ')))
inwt = lambda p: p.startswith(root + '/') and '/target/' not in p
ws_reads = {p for p in reads if inwt(p)}
undeclared = sorted(ws_reads - declared)
print(f'opened for reading: {len(reads)} files; in worktree sources: {len(ws_reads)}; declared in dep-info: {len(declared)}')
print(f'UNDECLARED worktree inputs: {len(undeclared)}')
for k, v in collections.Counter(os.path.relpath(os.path.dirname(p), root) for p in undeclared).most_common(6):
    print(f'   {v:5}  {k}/')
print('   e.g.', [os.path.relpath(p, root) for p in undeclared[:3]])
other = sorted(p for p in reads if not p.startswith(root + '/'))
kinds = collections.Counter('rustup toolchain' if '/.rustup/' in p else 'cargo registry' if '/.cargo/registry' in p else 'system' if p.startswith(('/System', '/usr', '/Library', '/private/var/db')) else 'OTHER' for p in other)
print('reads outside the worktree:', dict(kinds))
print('   OTHER:', [p for p in other if not any(s in p for s in ('/.rustup/', '/.cargo/registry')) and not p.startswith(('/System', '/usr', '/Library', '/private/var/db'))][:6])
