"""Replay a logged rustc invocation, swapping one group of path args from
worktree B into worktree A's command line, to find what breaks incremental reuse."""
import glob, os, shutil, subprocess, sys, time
argf = sorted(glob.glob('/tmp/hive/args-domain-*.txt'))
def load(f):
    a = open(f).read().rstrip('\n').split('\n')
    env = dict(kv.split('=', 1) for kv in open(f.replace('args-', 'env-').replace('.txt', '.bin')).read().split('\0') if '=' in kv)
    return a, env
(A, envA), (B, envB) = load(argf[0]), load(argf[1])
if 'wt-a' not in A[1] and 'wt-a' not in ' '.join(A): (A, envA), (B, envB) = (B, envB), (A, envA)
inc = [x for x in A if x.startswith('incremental=')][0].split('=', 1)[1]
snap = '/tmp/hive/domain-inc-snap'
groups = {'input': lambda x: x.endswith('lib.rs'), 'out-dir': lambda x: x.endswith('/deps') and '=' not in x,
          '-L': lambda x: x.startswith('dependency='), '--extern': lambda x: '.rmeta' in x or '.rlib' in x or '.dylib' in x,
          'incremental': lambda x: x.startswith('incremental=')}
def run(label, args, env):
    shutil.rmtree(inc, ignore_errors=True); shutil.copytree(snap, inc, symlinks=True)
    t = time.time()
    p = subprocess.run(args[:1] + args[1:] + ['-Zincremental-info'], cwd='/', env={**env, 'RUSTC_BOOTSTRAP': '1'}, capture_output=True, text=True)
    out = p.stderr + p.stdout
    verdict = 'DISCARDED' if 'ignoring cache' in out else 'reused'
    print(f'{label:34} {verdict:9} {time.time()-t:5.1f}s rc={p.returncode}', flush=True)
# Make the snapshot from A's own invocation (with -Zincremental-info, like the tests).
os.makedirs(snap, exist_ok=True); shutil.rmtree(snap)
subprocess.run(A + ['-Zincremental-info'], cwd='/', env={**envA, 'RUSTC_BOOTSTRAP': '1'}, capture_output=True)
shutil.copytree(inc, snap, symlinks=True)
run('T0 A unchanged', A, envA)
run('T1 A args, B env', A, envB)
for g, pred in groups.items():
    if g == 'incremental': continue
    mixed = [b if pred(a) and a != b else a for a, b in zip(A, B)]
    mixed = [m.replace('wt-e/backend/target/debug/incremental', 'wt-a/backend/target/debug/incremental') for m in mixed]
    run(f'T  swap {g}', mixed, envA)
# All of B's arguments and environment, with B's own incremental dir seeded from A's snapshot.
incB = [x for x in B if x.startswith('incremental=')][0].split('=', 1)[1]
inc = incB
run('T_all B args + B env', B, envB)
