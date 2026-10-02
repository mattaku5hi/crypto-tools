#!/usr/bin/env python3
"""P0.7 overlap: GMGN top-100-by-profit traders vs our confident bonding-curve buyers.

Inputs (all in this directory): runner_set.txt, gmgn_top_traders/<mint>.json,
runA_ours_buyer_intersect_k1.jsonl (or argv[1]). Pure stdlib; deterministic output.
"""
import json, os, sys
from collections import defaultdict

A = '123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz'
def b58e(b):
    n = int.from_bytes(bytes(b), 'big'); s = ''
    while n: n, r = divmod(n, 58); s = A[r] + s
    return '1' * (len(b) - len(bytes(b).lstrip(b'\0'))) + s

here = os.path.dirname(os.path.abspath(__file__))
mints = [l.strip().split(':', 1)[1] for l in open(f'{here}/runner_set.txt') if l.strip()]

ours_file = sys.argv[1] if len(sys.argv) > 1 else 'runA_ours_buyer_intersect_k1.jsonl'
ours = defaultdict(set)  # mint -> wallets
for line in open(f'{here}/{ours_file}'):
    r = json.loads(line)
    if r.get('kind') != 'buyer_match': continue
    addr = r['wallet']['address']
    w = addr if isinstance(addr, str) else b58e(addr['Solana'])  # v2 string / runA byte-array shape
    for a in r['matched_assets']:
        ours[a['token'] if 'token' in a else b58e(a['Token'][1]['Solana'])].add(w)

gmgn = {}; gmgn_rows = {}
for m in mints:
    d = json.load(open(f'{here}/gmgn_top_traders/{m}.json'))
    lst = d['list'] if 'list' in d else d['data']['list']
    gmgn_rows[m] = lst
    gmgn[m] = [x['address'] for x in lst]

print('| token | our buyers | GMGN top-100 | overlap | overlap in GMGN top-10 | overlap in GMGN top-20 | GMGN-tagged sniper among overlap |')
print('|---|---:|---:|---:|---:|---:|---:|')
tot_o = tot_g = 0
for m in mints:
    g = gmgn[m]; o = ours[m]; inter = [w for w in g if w in o]
    tags = {x['address']: set(x.get('tags') or []) | set(x.get('maker_token_tags') or []) for x in gmgn_rows[m]}
    snip = sum(1 for w in inter if any('sniper' in t for t in tags[w]))
    print(f'| `{m[:8]}…` | {len(o)} | {len(g)} | {len(inter)} | {len([w for w in g[:10] if w in o])} | {len([w for w in g[:20] if w in o])} | {snip} |')
    tot_o += len(o); tot_g += len(g)

# cross-token sets
def multi(sets, k):
    c = defaultdict(int)
    for s in sets.values():
        for w in s: c[w] += 1
    return {w for w, n in c.items() if n >= k}
g_sets = {m: set(v) for m, v in gmgn.items()}
for k in (2, 3):
    om, gm = multi(ours, k), multi(g_sets, k)
    print(f'\nK>={k}: ours={len(om)} gmgn={len(gm)} intersection={len(om & gm)}')
    for w in sorted(om & gm): print('  ', w)
all_o = set().union(*ours.values()); all_g = set().union(*g_sets.values())
print(f'\nunion: ours={len(all_o)} gmgn={len(all_g)} intersection={len(all_o & all_g)}')

# profit of overlapping vs non-overlapping GMGN traders (GMGN's own numbers, Tier 2)
def med(xs):
    xs = sorted(xs); n = len(xs)
    return None if not n else (xs[n//2] if n % 2 else (xs[n//2-1] + xs[n//2]) / 2)
ov, nov = [], []
for m in mints:
    for x in gmgn_rows[m]:
        p = x.get('realized_profit')
        if p is None: continue
        (ov if x['address'] in ours[m] else nov).append(float(p))
print(f'\nGMGN realized_profit (USD, Tier-2, unverified) median: in-our-set={med(ov)} (n={len(ov)}) not-in-our-set={med(nov)} (n={len(nov)})')
