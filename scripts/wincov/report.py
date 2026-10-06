#!/usr/bin/env python3
"""Line coverage of a Windows exe built by scripts/wincov.sh, from the addresses a run recorded.

    report.py HITS_DIR [--src SUBSTR] [--list]

HITS_DIR holds one `<exe name>.txt` per executable, next to which the exe itself must be found at
somewhere under HITS_DIR/../x86_64-pc-windows-gnu/debug/ (what scripts/wincov.sh lays out). Every instrumented basic block starts with a call of __sanitizer_cov_trace_pc; the
address after that call is the block's id. The hits files hold hex offsets from the image base,
written at exit by the runtime. A source line counts as covered when some block that starts on it
ran in any of the executables, so this is block-start coverage: it finds code nothing ever
reached, not every untaken branch. With --list the uncovered lines are printed too.
"""
import re, subprocess, sys, collections

args = sys.argv[1:]
import glob, os
hits_dir = args.pop(0)
src_filter, listing = "src/", False
while args:
    a = args.pop(0)
    if a == "--src":
        src_filter = args.pop(0)
    elif a == "--list":
        listing = True

BASE = 0x140000000
lines = collections.defaultdict(lambda: [0, 0])  # (file, line) -> [sites, hits]


def exe_for(name):
    root = os.path.join(hits_dir, "..", "x86_64-pc-windows-gnu", "debug")
    found = glob.glob(os.path.join(root, "**", name + ".exe"), recursive=True)
    return found[0] if found else None


def add_exe(exe, hits_file):
    dis = subprocess.run(["x86_64-w64-mingw32-objdump", "-d", "--no-show-raw-insn", exe],
                         capture_output=True, text=True, check=True).stdout
    sites = []
    pending = False
    for line in dis.splitlines():
        m = re.match(r"\s*([0-9a-f]+):\s+(\S.*)", line)
        if not m:
            continue
        if pending:
            sites.append(int(m.group(1), 16))
            pending = False
        if "call" in m.group(2) and "<__sanitizer_cov_trace_pc>" in m.group(2):
            pending = True
    with open(hits_file) as fh:
        hit = {int(x, 16) + BASE for x in fh if x.strip()}
    a2l = subprocess.run(["x86_64-w64-mingw32-addr2line", "-e", exe] + [hex(s) for s in sites],
                         capture_output=True, text=True, check=True).stdout.splitlines()
    seen = {}
    for s, loc in zip(sites, a2l):
        path, _, ln = loc.rpartition(":")
        ln = ln.split(" ")[0]
        if src_filter not in path or "library" in path or not ln.isdigit():
            continue
        path = path[path.index(src_filter):]
        e = seen.setdefault((path, int(ln)), [0, 0])
        e[0] += 1
        e[1] += s in hit
    for key, (n, h) in seen.items():
        e = lines[key]
        e[0] += n
        e[1] += h


for hits_file in sorted(glob.glob(os.path.join(hits_dir, "*.txt"))):
    name = os.path.basename(hits_file)[:-4]
    exe = exe_for(name)
    if exe:
        add_exe(exe, hits_file)

per_file = collections.defaultdict(lambda: [0, 0])
for (path, ln), (n, h) in lines.items():
    per_file[path][0] += 1
    per_file[path][1] += h > 0
tot = [0, 0]
for path in sorted(per_file):
    n, c = per_file[path]
    tot[0] += n
    tot[1] += c
    print(f"{path:45} {c:5}/{n:5} lines  {100 * c / n:5.1f}%")
if tot[0]:
    print(f"{'TOTAL':45} {tot[1]:5}/{tot[0]:5} lines  {100 * tot[1] / tot[0]:5.1f}%")
if listing:
    cur = None
    for (path, ln), (n, h) in sorted(lines.items()):
        if h == 0:
            print(f"{path}:{ln}")
