"""Inventory of a donated journal folder: files by name format and year, bytes, lines,
event-kind histogram (by count and by bytes), and the share EDDA's ingest keeps."""
import os, re, sys, collections
sys.stdout.reconfigure(encoding='utf-8')
D = sys.argv[1]
old = re.compile(r'^Journal\.(\d{2})(\d{2})\d{2}\d{6}\.\d+\.log$')
new = re.compile(r'^Journal\.(\d{4})-\d{2}-\d{2}T\d{6}\.\d+\.log$')
ev = re.compile(rb'"event":"([A-Za-z]+)"')
by_year = collections.Counter(); fmt = collections.Counter(); size_by_year = collections.Counter()
kinds = collections.Counter(); kind_bytes = collections.Counter()
files = lines = total = 0; first = last = None
for name in sorted(os.listdir(D)):
    m = old.match(name); n = new.match(name)
    if m: year = '20' + m.group(1); fmt['old YYMMDD'] += 1
    elif n: year = n.group(1); fmt['new YYYY-MM-DD'] += 1
    else: continue
    p = os.path.join(D, name); sz = os.path.getsize(p)
    files += 1; total += sz; by_year[year] += 1; size_by_year[year] += sz
    with open(p, 'rb') as f:
        for line in f:
            lines += 1
            k = ev.search(line)
            if k:
                kinds[k.group(1)] += 1; kind_bytes[k.group(1)] += len(line)
            if first is None and line.startswith(b'{ "timestamp"'): first = line[15:35].decode()
    last_line = line if lines else b''
    if last_line.startswith(b'{ "timestamp"'): last = last_line[15:35].decode()
print(f"files {files}, {total/1e6:.0f} MB, {lines} lines; span {first} .. {last}")
print("formats:", dict(fmt))
print("files by year:", dict(sorted(by_year.items())))
print("MB by year:", {y: round(s/1e6) for y, s in sorted(size_by_year.items())})
print(f"\n{len(kinds)} event kinds. Top 25 by count (count, MB, share of bytes):")
for k, n in kinds.most_common(25):
    print(f"  {n:>9} {kind_bytes[k]/1e6:>7.1f} MB {100*kind_bytes[k]/total:5.1f}%  {k}")
