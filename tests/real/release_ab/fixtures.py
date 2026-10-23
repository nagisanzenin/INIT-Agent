#!/usr/bin/env python3
"""Deterministic candidate pilot fixtures; external scorer never runs agent code.
These fixtures require calibration before being frozen as the primary pilot set.
"""
import argparse
import ast
import hashlib
import json
from pathlib import Path
import random
import stat

FAMILIES = ('validation', 'navigation', 'transformation', 'resumable')
PHASES = ('learn', 'transfer-1', 'transfer-2')
LEARN_RETENTION_INSTRUCTION = (
    '\nFuture work: You will later receive new tasks of this family with changed data in fresh '
    'conversations. Ordinary task workspace files and conversation history will not be retained '
    'for those tasks. If native evolution is available and worthwhile, retain a reusable '
    'capability through its normal admission process, and complete that admission before '
    'finishing. No particular implementation is required. If native evolution is unavailable '
    'or refused, continue and finish the current task using available capabilities.'
)


def encode(obj): return json.dumps(obj, sort_keys=True, indent=2) + '\n'
def digest(data): return hashlib.sha256(data).hexdigest()


def build(family, seed, count=24):
    if family not in FAMILIES: raise ValueError('unknown family')
    rng = random.Random(seed)
    files = {}
    if family == 'validation':
        records, expected = [], []
        for i in range(count):
            row = dict(id=f'r{i}', quantity=rng.choice([-1, 0, 1, 2, 7]),
                       price_cents=rng.choice([-10, 0, 25, 399]), currency=rng.choice(['USD','VND','???']),
                       active=rng.choice([True, False]))
            records.append(row)
            errors = []
            if row['quantity'] <= 0: errors.append('quantity')
            if row['price_cents'] < 0: errors.append('price_cents')
            if row['currency'] not in ('USD','VND'): errors.append('currency')
            expected.append(dict(id=row['id'], errors=errors,
                                 total_cents=row['quantity']*row['price_cents'] if not errors and row['active'] else 0))
        files['records.json'] = encode(records)
        prompt = ('Read records.json. For each record preserve order and output id, errors, total_cents. '
                  'errors is an ordered list: quantity if quantity <=0, price_cents if price_cents <0, currency '
                  'unless USD or VND. total_cents is quantity*price_cents only when no errors and active=true; otherwise 0.')
    elif family == 'navigation':
        names = [f'pkg_{rng.randrange(1000000):06d}_{i}' for i in range(count)]
        graph = {name: rng.sample(names, rng.randrange(min(4,count))) for name in names}
        for name, deps in graph.items(): files[f'modules/{name}.json'] = encode(dict(imports=deps))
        queries = rng.sample(names, min(8,count))
        expected = []
        for query in queries:
            reached, todo = set(), list(graph[query])
            while todo:
                node = todo.pop()
                if node not in reached:
                    reached.add(node); todo.extend(graph[node])
            reached.discard(query)
            expected.append(dict(module=query, reachable=sorted(reached)))
        files['queries.json'] = encode(queries)
        prompt = ('Read queries.json and modules/*.json. For each queried module in query order, output module and '
                  'reachable: sorted unique transitive imported module names, excluding the queried module even in cycles.')
    elif family == 'transformation':
        expected = []
        for i in range(count):
            value = rng.randrange(-99,100)
            name = f'module_{i}.py'
            src = f'# old_api comment stays\nNOTE = "old_api text stays"\ndef compute(x):\n    return old_api(x, {value}) + obj.old_api(x)\n'
            files[f'src/{name}'] = src
            expected.append(dict(path=f'src/{name}', ast=ast.dump(ast.parse(src.replace(f'old_api(x, {value})', f'new_api(x, {value})')), include_attributes=False)))
        prompt = ('Update every src/*.py file: rename calls whose callee is the bare name old_api to new_api. '
                  'Do not change attribute calls (obj.old_api), strings, comments or other expressions. '
                  'Write changes to those files. Do not execute the code.')
    else:
        # Event replay is the task content; restart/persistence is tested by the runtime lifecycle separately.
        events, state = [], {}
        for i in range(count):
            key = f'job-{rng.randrange(7)}'
            op = rng.choice(('set','add','delete'))
            value = rng.randrange(-20,30)
            events.append(dict(sequence=i, key=key, op=op, value=value))
            if op == 'set': state[key] = value
            elif op == 'add': state[key] = state.get(key, 0) + value
            else: state.pop(key, None)
        files['events.json'] = encode(events)
        expected = [dict(key=k,value=state[k]) for k in sorted(state)]
        prompt = ('Replay events.json in sequence order from an empty map. set assigns value; add increments '
                  '(missing key starts at zero); delete removes the key and ignores value. Output final entries '
                  'as objects with key and value, sorted by key. Runtime restart behavior is not part of this task.')
    if family != 'transformation': prompt += ' Write the JSON array to answer.json.'
    prompt += ' You may create ordinary workspace helpers. Use any available capabilities; no particular implementation is required.'
    return dict(family=family, seed=seed, files=files, prompt=prompt, expected=expected)


def materialize(fixture, destination):
    """Only workspace content is copied; expected answers remain in owner's fixture registry."""
    destination = Path(destination)
    if destination.exists(): raise ValueError('workspace must be new')
    destination.mkdir(parents=True)
    for name, text in fixture['files'].items():
        p = destination / name
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(text)


def safe_read(workspace, name):
    workspace = Path(workspace)
    if workspace.is_symlink(): raise ValueError('symlink workspace forbidden')
    workspace = workspace.resolve()
    target = workspace / name
    for ancestor in (target, *target.parents):
        if ancestor == workspace: break
        if ancestor.is_symlink(): raise ValueError('scorer refuses symlink outputs')
    if not stat.S_ISREG(target.stat().st_mode): raise ValueError('regular output file required')
    if target.stat().st_size > 2_000_000: raise ValueError('output too large')
    return target.read_text()


def score(fixture, workspace):
    expected = fixture['expected']
    errors = []
    correct = 0
    if fixture['family'] == 'transformation':
        for row in expected:
            try:
                actual = safe_read(workspace, row['path'])
                tree = ast.dump(ast.parse(actual), include_attributes=False)
                # AST ignores comments, so explicitly preserve fixture comments.
                comments = [s for s in fixture['files'][row['path']].splitlines() if s.startswith('#')]
                correct += tree == row['ast'] and all(s in actual.splitlines() for s in comments)
            except (OSError, ValueError, SyntaxError) as exc: errors.append(type(exc).__name__)
        total = len(expected)
    else:
        try:
            actual = json.loads(safe_read(workspace, 'answer.json'))
            if not isinstance(actual, list): raise ValueError('array required')
            # Canonical JSON avoids Python treating bool as integer equality.
            correct = sum(encode(a) == encode(b) for a,b in zip(actual,expected))
            total = max(len(actual), len(expected), 1)
        except (OSError, ValueError) as exc:
            errors.append(type(exc).__name__); total = max(len(expected),1)
    return dict(correct=correct, total=total, score=correct/total, passed=correct==total, errors=errors)


def registry(seed=2026102301):
    out = {}
    for i,family in enumerate(FAMILIES):
        out[family] = {phase: build(family, seed+100*i+j) for j,phase in enumerate(PHASES)}
        # One registered prompt per phase is shared by both arms. Only learning
        # explains future work; transfer data/prompts receive no extra hints.
        out[family]['learn']['prompt'] += LEARN_RETENTION_INSTRUCTION
    return out


def hashes(fixtures):
    scorer = digest(Path(__file__).read_bytes())
    return {family: {phase: dict(fixture_sha=digest(encode(fixture).encode()),
                                 prompt_sha=digest(fixture['prompt'].encode()), scorer_sha=scorer)
                     for phase,fixture in phases.items()} for family,phases in fixtures.items()}


def main():
    p = argparse.ArgumentParser(description=__doc__)
    sub=p.add_subparsers(dest='cmd',required=True)
    a=sub.add_parser('generate'); a.add_argument('destination'); a.add_argument('--seed', type=int, default=2026102301)
    a=sub.add_parser('materialize'); a.add_argument('registry'); a.add_argument('family',choices=FAMILIES); a.add_argument('phase',choices=PHASES); a.add_argument('workspace')
    a=sub.add_parser('score'); a.add_argument('registry'); a.add_argument('family',choices=FAMILIES); a.add_argument('phase',choices=PHASES); a.add_argument('workspace')
    args=p.parse_args()
    if args.cmd=='generate':
        dest=Path(args.destination)
        if dest.exists(): raise ValueError('registry must be new')
        fixtures=registry(args.seed)
        dest.write_text(encode(dict(fixtures=fixtures, hashes=hashes(fixtures))))
    else:
        fixture=json.loads(Path(args.registry).read_text())['fixtures'][args.family][args.phase]
        if args.cmd=='materialize': materialize(fixture,args.workspace)
        else: print(encode(score(fixture,args.workspace)),end='')

if __name__=='__main__': main()
