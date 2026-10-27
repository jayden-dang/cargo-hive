"""Spike 5: applesauce vs afsctool on a real Cargo target dir.

For each variant: copy the frozen corpus (preserving hardlinks), compress it,
then measure physical size, read-back time, byte-for-byte equality with the
original, and whether hardlinks / mtimes / the compressed flag survived.
Usage: python3 -I compress-bench.py CORPUS WORKDIR [variant ...]
"""
import os, re, shutil, stat, subprocess, sys, time

UF_COMPRESSED = 0x20
CORPUS, WORK = sys.argv[1], sys.argv[2]
NCPU = os.cpu_count()
VARIANTS = {
    'afsctool-lzfse': ['afsctool', '-c', '-T', 'LZFSE', f'-J{NCPU}'],
    'afsctool-lzvn': ['afsctool', '-c', '-T', 'LZVN', f'-J{NCPU}'],
    'afsctool-zlib5': ['afsctool', '-c', '-T', 'ZLIB', '-5', f'-J{NCPU}'],
    'applesauce-lzfse': ['applesauce', 'compress', '-q', '-c', 'lzfse'],
    'applesauce-lzvn': ['applesauce', 'compress', '-q', '-c', 'lzvn'],
    'applesauce-zlib5': ['applesauce', 'compress', '-q', '-c', 'zlib', '-l', '5'],
    'applesauce-lzfse-verify': ['applesauce', 'compress', '-q', '-c', 'lzfse', '--verify'],
}


def physical_kb(path):
    return int(subprocess.run(['du', '-sk', path], capture_output=True, text=True).stdout.split()[0])


def walk(root):
    for r, _, fs in os.walk(root):
        for f in fs:
            yield os.path.join(r, f)


def survey(root):
    """Map relative path -> (inode, nlink, mtime_ns, compressed)."""
    out = {}
    for p in walk(root):
        st = os.lstat(p)
        if stat.S_ISREG(st.st_mode):
            out[os.path.relpath(p, root)] = (st.st_ino, st.st_nlink, st.st_mtime_ns, bool(st.st_flags & UF_COMPRESSED))
    return out


def link_groups(s):
    """Partition of files into hardlink groups, as frozensets of relative paths."""
    by_ino = {}
    for rel, (ino, nlink, _, _) in s.items():
        if nlink > 1:
            by_ino.setdefault(ino, set()).add(rel)
    return {frozenset(g) for g in by_ino.values()}


def read_all(root):
    t = time.time(); n = 0
    for p in walk(root):
        with open(p, 'rb') as f:
            while chunk := f.read(1 << 22):
                n += len(chunk)
    return time.time() - t, n


def identical(a, b):
    with open(a, 'rb') as fa, open(b, 'rb') as fb:
        while True:
            x, y = fa.read(1 << 22), fb.read(1 << 22)
            if x != y:
                return False
            if not x:
                return True


def run(name, cmd):
    dst = os.path.join(WORK, name)
    shutil.rmtree(dst, ignore_errors=True)
    subprocess.run(['ditto', CORPUS, dst], check=True)  # ditto preserves hardlinks and mtimes
    before = survey(dst); before_kb = physical_kb(dst)
    t = time.time()
    p = subprocess.run(['/usr/bin/time', '-l'] + cmd + [dst], capture_output=True, text=True)
    wall = time.time() - t
    m = re.search(r'([\d.]+) real\s+([\d.]+) user\s+([\d.]+) sys', p.stderr)
    rss = re.search(r'(\d+)\s+maximum resident set size', p.stderr)
    after = survey(dst); after_kb = physical_kb(dst)
    rtime, rbytes = read_all(dst)
    bad = [rel for rel in after if not identical(os.path.join(CORPUS, rel), os.path.join(dst, rel))]
    groups_ok = link_groups(before) == link_groups(after)
    mtime_kept = sum(after[r][2] == before[r][2] for r in after)
    compressed = sum(v[3] for v in after.values())
    print(f'{name:24} rc={p.returncode} wall={wall:6.1f}s cpu={float(m.group(2)) + float(m.group(3)) if m else -1:7.1f}s '
          f'rss={int(rss.group(1)) / 1e6 if rss else -1:6.0f}MB | {before_kb / 1e6:5.2f} -> {after_kb / 1e6:5.2f} GB '
          f'({before_kb / max(after_kb, 1):4.2f}x) | compressed files {compressed}/{len(after)} | '
          f'read-back {rbytes / 1e9 / rtime:5.2f} GB/s | mismatches {len(bad)} | hardlink groups kept {groups_ok} | '
          f'mtime kept {mtime_kept}/{len(after)}', flush=True)
    if bad:
        print('   mismatched e.g.', bad[:3])
    if p.returncode:
        print('   stderr:', p.stderr[-400:])
    shutil.rmtree(dst, ignore_errors=True)


if __name__ == '__main__':
    os.makedirs(WORK, exist_ok=True)
    t, n = read_all(CORPUS)
    print(f'corpus: {len(survey(CORPUS))} files, {physical_kb(CORPUS) / 1e6:.2f} GB physical, '
          f'uncompressed read-back {n / 1e9 / t:.2f} GB/s', flush=True)
    for name in (sys.argv[3:] or VARIANTS):
        run(name, VARIANTS[name])
