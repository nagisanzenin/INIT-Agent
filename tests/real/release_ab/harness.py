#!/usr/bin/env python3
"""Offline release A/B coordination. No model client: Root must call reserve before egress.
Money uses integer micro-USD throughout. Artifact inputs must be owner-controlled.
"""
import argparse
import hashlib
import json
from pathlib import Path
import random
import sqlite3

FAMILIES = ('validation', 'navigation', 'transformation', 'resumable')
ENVELOPES = {'smoke': 400000, 'calibration': 800000, 'main': 7200000,
             'reliability': 800000, 'reserve': 800000}
USAGE_TOKEN_FIELDS = ('prompt_tokens','completion_tokens','prompt_cache_hit_tokens','prompt_cache_miss_tokens')


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(',', ':'), allow_nan=False).encode()


def sha(value):
    return hashlib.sha256(canonical(value)).hexdigest()


def plan(seed=20261023):
    rng = random.Random(seed)
    tasks = []
    for family in FAMILIES:
        first = rng.choice(('A', 'B'))
        for repeat in range(2):
            arms = (first, 'B' if first == 'A' else 'A')
            if repeat:
                arms = arms[::-1]
            block = f'{family}-{repeat}'
            for arm in arms:
                for phase, cap, wall in [('learn', 210000, 1200), ('transfer-1', 120000, 600), ('transfer-2', 120000, 600)]:
                    tasks.append(dict(id=f'{block}-{arm}-{phase}', family=family,
                                      block=block, repeat=repeat, arm=arm, phase=phase,
                                      cap_uusd=cap, wall_s=wall, order=len(tasks)))
    return dict(version=2, seed=seed, provider='deepseek', model='deepseek-flash',
                global_budget_policy=dict(input_token_bound=1048576,output_token_bound=4096,reservation_uusd=319488,task_envelopes='byte-estimates-until-usage'),
                envelopes_uusd=ENVELOPES, ceiling_uusd=10000000, tasks=tasks)


def freeze(manifest, fixtures, binary, repo_sha):
    """fixtures: family -> learn/transfer-1/transfer-2 -> fixture/scorer hashes.
    No task selection from results. Repeat shares task instances, fresh agent state.
    """
    if not repo_sha or len(repo_sha) != 40 or any(c not in '0123456789abcdef' for c in repo_sha):
        raise ValueError('full repo commit SHA required')
    for family in FAMILIES:
        for phase in ('learn', 'transfer-1', 'transfer-2'):
            entry = fixtures[family][phase]
            for key in ('fixture_sha', 'scorer_sha', 'prompt_sha'):
                if len(entry[key]) != 64 or any(c not in '0123456789abcdef' for c in entry[key]):
                    raise ValueError('invalid fixture hash')
    out = dict(manifest, fixtures=fixtures, binary_sha=hashlib.sha256(Path(binary).read_bytes()).hexdigest(), repo_sha=repo_sha)
    out['protocol_sha'] = sha(out)
    return out


def integer(value):
    if type(value) is not int or value < 0:
        raise ValueError('nonnegative integer required')
    return value


def usage_tokens(value=None):
    """Validated provider observations; absent cache fields remain unknown."""
    if value is None: return dict.fromkeys(USAGE_TOKEN_FIELDS)
    if type(value) is not dict: raise ValueError('provider token usage must be an object')
    prompt=integer(value.get('prompt_tokens')); output=integer(value.get('completion_tokens'))
    if not prompt: raise ValueError('positive reported prompt tokens required')
    result=dict(prompt_tokens=prompt,completion_tokens=output)
    for field in USAGE_TOKEN_FIELDS[2:]:
        count=value.get(field)
        if count is not None and integer(count)>prompt: raise ValueError('cache tokens exceed reported prompt')
        result[field]=count
    hit,miss=(result[field] for field in USAGE_TOKEN_FIELDS[2:])
    if hit is not None and miss is not None and hit+miss!=prompt: raise ValueError('inconsistent reported cache usage')
    return result


def worst_case_uusd(input_tokens_bound, output_tokens_bound, input_rate, output_rate):
    """Rates are micro-USD per million tokens; no cache/discount assumptions.
    Bounds must include ALL billing tokens including hidden reasoning output.
    Root must enforce output bound; no token estimate can establish an input bound.
    """
    args = [integer(v) for v in (input_tokens_bound, output_tokens_bound, input_rate, output_rate)]
    return (args[0] * args[2] + args[1] * args[3] + 999999) // 1000000


class Ledger:
    def __init__(self, path):
        self.db = sqlite3.connect(path, timeout=30, isolation_level=None)
        self.db.execute('PRAGMA synchronous=FULL')
        self.db.executescript('''CREATE TABLE IF NOT EXISTS config(k TEXT PRIMARY KEY,v TEXT NOT NULL);
          CREATE TABLE IF NOT EXISTS runs(id TEXT PRIMARY KEY,category TEXT NOT NULL,cap INTEGER NOT NULL);
          CREATE TABLE IF NOT EXISTS requests(id TEXT PRIMARY KEY,run TEXT NOT NULL,held INTEGER NOT NULL,
            actual INTEGER,status TEXT NOT NULL,global_held INTEGER NOT NULL,
            prompt_tokens INTEGER,completion_tokens INTEGER,prompt_cache_hit_tokens INTEGER,prompt_cache_miss_tokens INTEGER);''')
        columns={row[1] for row in self.db.execute('PRAGMA table_info(requests)')}
        if 'global_held' not in columns:
            # Existing local offline ledgers retain every historical hold. No reset.
            with self.transaction():
                self.db.execute('ALTER TABLE requests ADD COLUMN global_held INTEGER NOT NULL DEFAULT 0')
                self.db.execute('UPDATE requests SET global_held=held')
        # Migration never invents observations for old requests or releases holds.
        for field in USAGE_TOKEN_FIELDS:
            if field not in columns:
                with self.transaction(): self.db.execute('ALTER TABLE requests ADD COLUMN '+field+' INTEGER')

    def initialize(self, manifest, available_uusd, account_reserve_uusd=0):
        integer(available_uusd); integer(account_reserve_uusd)
        ceiling = min(10000000, available_uusd - account_reserve_uusd)
        if ceiling <= 0:
            raise ValueError('no authorized spendable balance')
        with self.transaction():
            if self.db.execute('SELECT count(*) FROM config').fetchone()[0]:
                raise ValueError('ledger already initialized')
            for k, v in {'ceiling': ceiling, 'halt': False, 'protocol': sha(manifest), 'global_request_bound': manifest.get('global_budget_policy',{}).get('reservation_uusd',0)}.items():
                self.db.execute('INSERT INTO config VALUES (?,?)', (k, json.dumps(v)))
            for task in manifest['tasks']:
                self.db.execute('INSERT INTO runs VALUES (?,?,?)', (task['id'], 'main', task['cap_uusd']))

    def transaction(self):
        ledger = self
        class Tx:
            def __enter__(self): ledger.db.execute('BEGIN IMMEDIATE')
            def __exit__(self, typ, value, tb): ledger.db.execute('ROLLBACK' if typ else 'COMMIT')
        return Tx()

    def config(self, key):
        row = self.db.execute('SELECT v FROM config WHERE k=?', (key,)).fetchone()
        if row is None:
            raise ValueError('ledger not initialized')
        return json.loads(row[0])

    def add_run(self, run, category, cap):
        integer(cap)
        if category not in ENVELOPES or category == 'main':
            raise ValueError('main runs must be frozen at initialization')
        with self.transaction():
            self.config('ceiling')
            self.db.execute('INSERT INTO runs VALUES (?,?,?)', (run, category, cap))

    def reserve(self, request, run, bound, global_bound=None):
        integer(bound)
        global_bound=bound if global_bound is None else global_bound
        integer(global_bound)
        if global_bound < bound: raise ValueError("global bound must cover task estimate")
        if not bound: raise ValueError('zero reservation forbidden')
        with self.transaction():
            if self.config('halt'): raise ValueError('ledger halted after accounting overrun')
            # Single in-flight request across every test home and category.
            if self.db.execute("SELECT 1 FROM requests WHERE status='pending'").fetchone():
                raise ValueError('unresolved request: settle conservatively before next egress')
            row = self.db.execute('SELECT category,cap FROM runs WHERE id=?', (run,)).fetchone()
            if row is None: raise ValueError('unregistered run')
            category, cap = row
            total = self.db.execute('SELECT coalesce(sum(global_held),0) FROM requests').fetchone()[0]
            spent = self.db.execute('SELECT coalesce(sum(held),0) FROM requests WHERE run=?', (run,)).fetchone()[0]
            subtotal = self.db.execute('SELECT coalesce(sum(q.held),0) FROM requests q JOIN runs r ON r.id=q.run WHERE r.category=?', (category,)).fetchone()[0]
            if total + global_bound > self.config('ceiling') or spent + bound > cap or subtotal + bound > ENVELOPES[category]:
                raise ValueError('budget refusal before request')
            self.db.execute("INSERT INTO requests (id,run,held,actual,status,global_held) VALUES (?,?,?,NULL,'pending',?)", (request, run, bound,global_bound))

    def settle(self, request, actual=None, reported_usage=None):
        if actual is not None: integer(actual)
        tokens=usage_tokens(reported_usage)
        overrun = False
        with self.transaction():
            row = self.db.execute('SELECT held,status,global_held FROM requests WHERE id=?', (request,)).fetchone()
            if row is None or row[1] != 'pending': raise ValueError('unknown or settled request')
            held = row[0]
            if actual is not None and actual > held:
                overrun = True
                self.db.execute("UPDATE config SET v='true' WHERE k='halt'")
            self.db.execute('UPDATE requests SET held=?,global_held=?,actual=?,status=?,prompt_tokens=?,completion_tokens=?,prompt_cache_hit_tokens=?,prompt_cache_miss_tokens=? WHERE id=?',
                            (held if actual is None else actual, row[2] if actual is None else actual, actual, 'unknown' if actual is None else 'settled',
                             *(tokens[field] for field in USAGE_TOKEN_FIELDS),request))
        if overrun: raise ValueError('actual exceeds reservation: halted; bound implementation invalid')

    def cache_summary(self, run=None):
        rows=self.db.execute('SELECT prompt_tokens,completion_tokens,prompt_cache_hit_tokens,prompt_cache_miss_tokens FROM requests'
                             +(' WHERE run=?' if run is not None else ''),() if run is None else (run,)).fetchall()
        known=[row for row in rows if row[0] is not None and row[2] is not None and row[3] is not None]
        covered=sum(row[0] for row in known); prompt=sum(row[0] for row in rows if row[0] is not None)
        return dict(requests=len(rows),token_known_requests=sum(row[0] is not None for row in rows),
            cache_known_requests=len(known),cache_unknown_requests=len(rows)-len(known),
            observed_prompt_tokens=prompt if any(row[0] is not None for row in rows) else None,
            observed_completion_tokens=sum(row[1] for row in rows if row[1] is not None) if any(row[1] is not None for row in rows) else None,
            cache_covered_prompt_tokens=covered,prompt_cache_hit_tokens=sum(row[2] for row in known) if known else None,
            prompt_cache_miss_tokens=sum(row[3] for row in known) if known else None,
            cache_hit_percent=100*sum(row[2] for row in known)/covered if covered else None,
            cache_prompt_coverage_percent=100*covered/prompt if prompt else None)

    def summary(self):
        return dict(ceiling_uusd=self.config('ceiling'), halted=self.config('halt'),
                    accounted_uusd=self.db.execute('SELECT coalesce(sum(global_held),0) FROM requests').fetchone()[0],
                    task_estimated_uusd=self.db.execute('SELECT coalesce(sum(held),0) FROM requests').fetchone()[0],
                    unresolved=self.db.execute("SELECT count(*) FROM requests WHERE status='pending'").fetchone()[0],
                    missing_usage=self.db.execute("SELECT count(*) FROM requests WHERE status='unknown'").fetchone()[0])


def analyze(manifest, receipts):
    if 'protocol_sha' in manifest:
        body = {k: v for k, v in manifest.items() if k != 'protocol_sha'}
        if sha(body) != manifest['protocol_sha']:
            raise ValueError('manifest hash mismatch')
    expected = {t['id']: t for t in manifest['tasks']}
    seen = {}
    for row in receipts:
        rid = row['id']
        if rid not in expected or rid in seen: raise ValueError('unknown/duplicate receipt')
        if row['protocol_sha'] != manifest.get('protocol_sha', sha(manifest)): raise ValueError('protocol mismatch')
        if row['arm'] != expected[rid]['arm'] or row['phase'] != expected[rid]['phase']: raise ValueError('receipt arm/phase mismatch')
        if type(row['passed']) is not bool: raise ValueError('passed must be boolean')
        integer(row['charged_uusd'])
        if row['status'] != 'completed' and row['passed']: raise ValueError('failed/capped run cannot pass')
        seen[rid] = row
    complete = len(seen) == len(expected)
    results = {}
    for arm in ('A', 'B'):
        rows = [r for r in seen.values() if r['arm'] == arm]
        transfer = [r for r in rows if r['phase'].startswith('transfer')]
        success = sum(r['passed'] for r in transfer)
        cost = sum(r['charged_uusd'] for r in rows)
        results[arm] = dict(tasks_recorded=len(rows), transfer_recorded=len(transfer), transfer_passes=success,
                            total_charged_uusd=cost, cost_per_success_uusd=cost / success if success else None)
    family_deltas = {}
    if complete:
        for family in FAMILIES:
            counts = {arm: sum(seen[t['id']]['passed'] for t in expected.values() if t['family'] == family and t['arm'] == arm and t['phase'].startswith('transfer')) for arm in ('A', 'B')}
            family_deltas[family] = (counts['B'] - counts['A']) / 4
    return dict(complete=complete, missing_ids=sorted(set(expected)-set(seen)), arms=results,
                family_transfer_deltas=family_deltas,
                mean_transfer_delta=sum(family_deltas.values())/4 if complete else None,
                interpretation='Pilot only; no causal self-evolution attribution without separately verified Root evidence.')


def main():
    p = argparse.ArgumentParser(description=__doc__)
    sub = p.add_subparsers(dest='cmd', required=True)
    sub.add_parser('plan')
    a = sub.add_parser('analyze'); a.add_argument('manifest'); a.add_argument('receipts')
    a = sub.add_parser('summary'); a.add_argument('ledger')
    a = p.parse_args()
    if a.cmd == 'plan': out = plan()
    elif a.cmd == 'summary': out = Ledger(a.ledger).summary()
    else: out = analyze(json.loads(Path(a.manifest).read_text()), [json.loads(l) for l in Path(a.receipts).read_text().splitlines() if l.strip()])
    print(json.dumps(out, indent=2, allow_nan=False))

if __name__ == '__main__': main()
