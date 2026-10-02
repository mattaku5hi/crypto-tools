#!/usr/bin/env python3
"""P0.7 part 2: GMGN 30d wallet stats (Tier 2, unverified) for our K>=2 vs GMGN's K>=2 sets.

GMGN figures are a cross-check only (ADR-008 Tier 2); nothing here enters scout-ledger.
"""
import json, os
from collections import Counter
from decimal import Decimal

here = os.path.dirname(os.path.abspath(__file__))
def load_set(name): return [l.strip() for l in open(f'{here}/{name}') if l.strip()]
def stats(w):
    p = f'{here}/gmgn_wallet_stats_30d/{w}.json'
    if not os.path.exists(p): return None
    d = json.load(open(p))
    return d if isinstance(d, dict) and 'realized_profit' in d else None
def med(xs):
    xs = sorted(xs); n = len(xs)
    if not n: return None
    return xs[n // 2] if n % 2 else (xs[n // 2 - 1] + xs[n // 2]) / 2

def summarize(label, wallets):
    rows = [(w, stats(w)) for w in wallets]
    ok = [s for _, s in rows if s]
    rp = [Decimal(s['realized_profit']) for s in ok if s.get('realized_profit') is not None]
    wr = [Decimal(str(s['pnl_stat']['winrate'])) for s in ok if (s.get('pnl_stat') or {}).get('winrate') is not None]
    tn = [int(s['pnl_stat']['token_num']) for s in ok if (s.get('pnl_stat') or {}).get('token_num') is not None]
    tags = Counter(t for s in ok for t in ((s.get('common') or {}).get('tags') or []))
    fund = Counter(((s.get('common') or {}).get('fund_from') or 'unknown') for s in ok)
    pos = sum(1 for x in rp if x > 0)
    print(f'### {label}\n')
    print(f'- wallets: {len(wallets)}; GMGN stats available: {len(ok)} (missing/err: {len(wallets) - len(ok)})')
    print(f'- realized_profit 30d USD: median {med(rp):.0f}, positive {pos}/{len(rp)}' if rp else '- realized_profit: n/a')
    print(f'- winrate: median {med(wr)}' if wr else '- winrate: n/a')
    print(f'- tokens traded 30d: median {med(tn)}' if tn else '- tokens traded: n/a')
    print(f'- tags: {dict(tags.most_common(8)) or "none"}')
    print(f'- funded from (top): {dict(fund.most_common(5))}\n')
    return {w for w, s in rows if s}

ours = load_set('runB_ours_k2_wallets.txt'); gm = load_set('gmgn_k2_wallets.txt')
summarize("Ours: confident bonding-curve buyers of >=2 runners (run B)", ours)
summarize("GMGN: top-100-by-profit on >=2 runners", gm)
