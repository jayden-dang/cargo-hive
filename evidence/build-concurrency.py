"""Reconstruct when agent sessions ran Cargo builds, per checkout, from Claude Code
transcripts, and measure how often builds in different checkouts overlapped.
Reads only tool-call metadata (command text, cwd, timestamps); prints aggregates."""
import glob, json, os, re, sys, collections, datetime as dt

BUILD = re.compile(r'\b(cargo\s+(\+\S+\s+)?(build|test|nextest|clippy|check|run|llvm-cov|watch)|just\s+(test|check|lint|build|dev)|moon\s+run)\b')
dirs = [d for d in glob.glob(os.path.expanduser('~/.claude/projects/*')) if 'klynt' in d]
calls = {}      # tool_use_id -> dict
for d in dirs:
    for f in glob.glob(d + '/**/*.jsonl', recursive=True):
        for line in open(f, errors='ignore'):
            try: e = json.loads(line)
            except Exception: continue
            ts = e.get('timestamp'); msg = e.get('message') or {}
            content = msg.get('content') if isinstance(msg, dict) else None
            if not ts or not isinstance(content, list): continue
            for c in content:
                if not isinstance(c, dict): continue
                if c.get('type') == 'tool_use' and c.get('name') == 'Bash':
                    cmd = (c.get('input') or {}).get('command', '')
                    if BUILD.search(cmd):
                        cwd = e.get('cwd', '?')
                        m = re.search(r'cd\s+(/\S+)', cmd)
                        where = m.group(1) if m else cwd
                        calls[c['id']] = dict(start=ts, cwd=where, cmd=cmd, bg=bool((c.get('input') or {}).get('run_in_background')))
                elif c.get('type') == 'tool_result' and c.get('tool_use_id') in calls:
                    calls[c['tool_use_id']]['end'] = ts
P = lambda s: dt.datetime.fromisoformat(s.replace('Z', '+00:00'))
def checkout(p):
    p = os.path.realpath(p) if p.startswith('/') else p
    for pat in (r'(.*/\.worktrees/[^/]+)', r'(.*/\.claude/worktrees/[^/]+)', r'(/private/tmp/[^/]+/klynt)', r'(/tmp/[^/]+/klynt)', r'(/Users/jayden/Developer/klynt/klynt)'):
        m = re.match(pat, p)
        if m: return m.group(1).replace('/private/tmp', '/tmp')
    return p
iv = []
for c in calls.values():
    if 'end' not in c or c['bg']: continue
    s, e = P(c['start']), P(c['end'])
    if 2 <= (e - s).total_seconds() <= 3600: iv.append((s, e, checkout(c['cwd'])))
bg = sum(1 for c in calls.values() if c['bg'])
print(f'build-like tool calls: {len(calls)} (foreground with duration: {len(iv)}, background w/o duration: {bg})')
print('checkouts:', len({x[2] for x in iv}))
for k, v in collections.Counter(x[2] for x in iv).most_common(12): print(f'   {v:5}  {k}')
# sweep: time with k distinct checkouts building simultaneously
ev = sorted([(s, 1, w) for s, e, w in iv] + [(e, -1, w) for s, e, w in iv])
active = collections.Counter(); last = None; dur = collections.Counter(); peak = (0, None)
for t, d, w in ev:
    if last is not None:
        k = sum(1 for x in active.values() if x > 0)
        if k: dur[k] += (t - last).total_seconds()
    active[w] += d; last = t
    k = sum(1 for x in active.values() if x > 0)
    if k > peak[0]: peak = (k, t)
tot = sum(dur.values())
print(f'total build wall time: {tot/3600:.1f} h; distribution of simultaneous checkouts building:')
for k in sorted(dur): print(f'   {k} checkout(s): {dur[k]/3600:6.2f} h  ({100*dur[k]/tot:4.1f}%)')
print('peak simultaneous checkouts:', peak[0], 'at', peak[1])
days = collections.defaultdict(float)
for s, e, w in iv: days[s.date()] += (e - s).total_seconds()
print('span:', min(days), '->', max(days), f'({len(days)} active days)')

# First build per checkout (excluding the main checkout): how long did the first
# foreground build call take, and how many new checkouts appeared per active day?
first = {}
for s, e, w in sorted(iv):
    first.setdefault(w, (s, e))
fb = sorted((e - s).total_seconds() for w, (s, e) in first.items() if w != '/Users/jayden/Developer/klynt/klynt')
if fb:
    q = lambda p: fb[min(len(fb) - 1, int(p * len(fb)))]
    print(f'first build call in a new checkout: n={len(fb)} median={q(.5):.0f}s p75={q(.75):.0f}s p90={q(.9):.0f}s max={fb[-1]:.0f}s')
born = collections.Counter(s.date() for w, (s, e) in first.items())
print('new checkouts per active day:', dict(sorted(collections.Counter(born.values()).items())), '(count -> days)')

# Codex sessions: shell commands live inside custom_tool_call 'exec' JS bodies.
cx = {}
for f in glob.glob(os.path.expanduser('~/.codex/sessions/**/*.jsonl'), recursive=True):
    for line in open(f, errors='ignore'):
        try: e = json.loads(line)
        except Exception: continue
        p = e.get('payload') or {}; ts = e.get('timestamp')
        if not ts: continue
        if p.get('type') == 'custom_tool_call' and BUILD.search(p.get('input', '')) and 'klynt' in p.get('input', ''):
            m = re.search(r'workdir\s*:\s*"([^"]+)"', p['input']) or re.search(r'cd\s+(/[^\s&;"]+)', p['input'])
            cx[p.get('call_id')] = dict(start=ts, cwd=m.group(1) if m else '?')
        elif p.get('type') == 'custom_tool_call_output' and p.get('call_id') in cx:
            cx[p['call_id']]['end'] = ts
civ = [(P(c['start']), P(c['end']), checkout(c['cwd'])) for c in cx.values() if 'end' in c]
civ = [x for x in civ if 2 <= (x[1] - x[0]).total_seconds() <= 3600]
print(f'codex build-like calls with duration: {len(civ)} across {len({x[2] for x in civ})} checkouts')
allv = iv + civ
ev = sorted([(s, 1, w) for s, e, w in allv] + [(e, -1, w) for s, e, w in allv])
active = collections.Counter(); last = None; dur = collections.Counter()
for t, d, w in ev:
    if last is not None:
        k = sum(1 for x in active.values() if x > 0)
        if k: dur[k] += (t - last).total_seconds()
    active[w] += d; last = t
tot = sum(dur.values())
print(f'Claude+Codex: total build wall {tot/3600:.1f} h; ' + ', '.join(f'{k} checkout(s) {100*dur[k]/tot:.1f}%' for k in sorted(dur)))

kind = lambda w: ('.worktrees/<name>' if '/.worktrees/' in w else '.claude/worktrees/<name> (native)' if '/.claude/worktrees/' in w
                  else '/tmp/<run>/klynt (full clone)' if w.startswith('/tmp/') else 'main checkout' if w == '/Users/jayden/Developer/klynt/klynt' else 'other')
print('checkout kinds:', dict(collections.Counter(kind(w) for w in {x[2] for x in iv})))
print('build wall by kind (h):', {k: round(sum((e - s).total_seconds() for s, e, w in iv if kind(w) == k) / 3600, 2) for k in {kind(x[2]) for x in iv}})

# Shapes of the first build-like command in each new checkout (normalized), to choose
# what a seed must cover.
firstcmd = {}
for cid, c in sorted(calls.items(), key=lambda kv: kv[1]['start']):
    w = checkout(c['cwd'])
    if w != '/Users/jayden/Developer/klynt/klynt': firstcmd.setdefault(w, c)
def shape(cmd):
    m = BUILD.search(cmd); s = cmd[m.start():].split('&&')[0].split('|')[0].split(';')[0]
    s = re.sub(r'\s+', ' ', s).strip()
    s = re.sub(r'(-p|--package) \S+', r'\1 <crate>', s)
    s = re.sub(r'--test \S+', '--test <t>', s)
    s = re.sub(r' (\S+::\S+|[a-z_]+::|\'[^\']*\'|"[^"]*")', '', s)
    return ' '.join(s.split()[:7])
print('first-command shapes in new checkouts:')
for k, v in collections.Counter(shape(c['cmd']) for c in firstcmd.values()).most_common(15): print(f'   {v:3}  {k}')
