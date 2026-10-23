#!/usr/bin/env python3
"""Read-only descriptive analysis of owner-held native DeepSeek pilot artifacts.

No model calls, imports with network side effects, ledger mutation or significance
tests. Settled costs are usage-priced estimates, never provider invoice totals.
Unknown/pending reservations are upper bounds and are never called money spent.
"""
import argparse
from contextlib import closing
from datetime import datetime
import hashlib
import json
import math
from pathlib import Path
import re
import sqlite3
from statistics import median

from fixtures import hashes, score
from harness import FAMILIES, integer, plan, sha
from readiness import check as qualification_check
from lifecycle import BASE, Lifecycle, tree_hash

PHASES = ('learn', 'transfer-1', 'transfer-2')
BINDINGS = ('repo_sha', 'binary_sha', 'protocol_sha')
UPSTREAM = 'https://api.deepseek.com/chat/completions'
TOKEN_FIELDS = ('prompt_tokens', 'completion_tokens', 'prompt_cache_hit_tokens', 'prompt_cache_miss_tokens')


def digest(path):
    h = hashlib.sha256()
    with Path(path).open('rb') as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b''): h.update(chunk)
    return h.hexdigest()


def read_json(path):
    def pairs(items):
        result = {}
        for key, value in items:
            if key in result: raise ValueError('duplicate JSON key')
            result[key] = value
        return result
    def invalid(_): raise ValueError('nonfinite JSON number')
    return json.loads(Path(path).read_text(), object_pairs_hook=pairs, parse_constant=invalid)


def require(ok, message):
    if not ok: raise ValueError(message)


def hash_value(value, size=64):
    return type(value) is str and re.fullmatch('[0-9a-f]{' + str(size) + '}', value) is not None


def validate_manifest(manifest):
    require(type(manifest) is dict, 'manifest must be an object')
    require(hash_value(manifest.get('protocol_sha')), 'frozen protocol missing')
    require(sha({k: v for k, v in manifest.items() if k != 'protocol_sha'}) == manifest['protocol_sha'], 'frozen protocol hash mismatch')
    require(type(manifest.get('seed')) is int, 'protocol seed required')
    # A valid hash alone cannot authorize selecting a favorable task subset.
    expected = plan(manifest['seed'])
    require(all(manifest.get(k) == v for k, v in expected.items()), 'manifest differs from preregistered v2 design')
    require(hash_value(manifest.get('repo_sha'), 40) and hash_value(manifest.get('binary_sha')), 'source/root binding missing')
    fixtures = manifest.get('fixtures', {})
    require(set(fixtures) == set(FAMILIES), 'frozen fixture families missing')
    for family in FAMILIES:
        require(set(fixtures[family]) == set(PHASES), 'frozen fixture phases missing')
        for phase in PHASES:
            entry = fixtures[family][phase]
            require(type(entry) is dict and set(entry) == {'fixture_sha', 'scorer_sha', 'prompt_sha'} and all(hash_value(v) for v in entry.values()), 'invalid frozen fixture hash')


def runtime_binary_provenance(config):
    """Initial Body is proven by immutable Genesis if a B learn replaces it.

    Root, Kernel, ctl and every other binary must still match initial hashes.
    Current Body's relationship to Root admission is reported separately; this
    provenance check alone never attributes a benefit to self-evolution.
    """
    expected = config['binaries']
    if config.get('base'):
        live = Lifecycle(config['base']).live
        for path in expected:
            binary = Path(path)
            if live == binary or live in binary.parents:
                resolved = binary.resolve(strict=True)
                require(live == resolved or live in resolved.parents, 'runtime binary symlink escapes live home')
    current = {path: digest(path) for path in expected}
    changed = [path for path in expected if current[path] != expected[path]]
    result = dict(initial_runtime_binaries=expected, current_runtime_binaries=current,
                  evolved_body_binary=None, immutable_genesis_snapshot=None)
    if not changed: return result
    life = Lifecycle(config.get('base', str(BASE)))
    body_path = str(life.init_home / 'bin/init-body')
    require(changed == [body_path], 'immutable Root/Kernel/runtime binary file mismatch')
    state = life.state(); require(state.get('stopped') is True, 'runtime must be stopped for initial Body provenance')
    genesis = life.snapshots / 'genesis'
    fingerprint = tree_hash(genesis)
    require(hash_value(state.get('snapshots', {}).get('genesis')) and fingerprint == state['snapshots']['genesis'], 'immutable Genesis snapshot mismatch')
    original = genesis / Path(body_path).relative_to(life.live)
    require(digest(original) == expected[body_path], 'Genesis Body binary differs from frozen initial provenance')
    result.update(evolved_body_binary=dict(path=body_path, initial_sha256=expected[body_path], current_sha256=current[body_path]),
                  immutable_genesis_snapshot=dict(name='genesis', tree_sha256=fingerprint))
    return result


def ledger_snapshot(path, manifest):
    """Use SQLite read-only snapshot; do not call Ledger, which creates/migrates.
    The captured state may include one interrupted execution after the prefix.
    """
    uri = Path(path).resolve().as_uri() + '?mode=ro'
    with closing(sqlite3.connect(uri, uri=True)) as db:
        db.row_factory = sqlite3.Row
        db.execute('PRAGMA query_only=ON')
        db.execute('BEGIN')
        require(db.execute('PRAGMA quick_check').fetchone()[0] == 'ok', 'ledger integrity check failed')
        config = {r['k']: json.loads(r['v']) for r in db.execute('SELECT k,v FROM config')}
        runs = [dict(r) for r in db.execute('SELECT id,category,cap FROM runs')]
        columns = {r[1] for r in db.execute('PRAGMA table_info(requests)')}
        token_select = ','.join(name if name in columns else 'NULL AS ' + name for name in TOKEN_FIELDS)
        requests = [dict(r) for r in db.execute('SELECT rowid AS request_sequence,id,run,held,actual,status,global_held,' + token_select + ' FROM requests ORDER BY rowid')]
        exists = db.execute("SELECT 1 FROM sqlite_master WHERE type='table' AND name='executions'").fetchone()
        executions = [dict(r) for r in db.execute('SELECT id,status,receipt_sha FROM executions')] if exists else []
    require(config.get('protocol') == sha(manifest), 'ledger frozen protocol mismatch')
    ceiling = integer(config.get('ceiling'))
    require(0 < ceiling <= 10000000 and type(config.get('halt')) is bool, 'invalid authorized ledger ceiling/halt')
    require(config.get('global_request_bound') == manifest['global_budget_policy']['reservation_uusd'], 'ledger global bound mismatch')
    expected = {t['id']: t for t in manifest['tasks']}
    run_map = {r['id']: r for r in runs}
    require(len(run_map) == len(runs), 'duplicate ledger run')
    for task in manifest['tasks']:
        require(run_map.get(task['id']) == dict(id=task['id'], category='main', cap=task['cap_uusd']), 'ledger task selection/cap mismatch')
    for run in runs:
        require(run['category'] in manifest['envelopes_uusd'], 'unknown ledger category')
        integer(run['cap'])
        require(run['category'] != 'main' or run['id'] in expected, 'unregistered main task')
    for request in requests:
        require(request['run'] in run_map, 'request run not registered')
        integer(request['held']); integer(request['global_held'])
        require(request['status'] in ('settled', 'pending', 'unknown'), 'invalid request accounting status')
        for field in TOKEN_FIELDS:
            if request[field] is not None: integer(request[field])
        prompt, hit, miss = (request[k] for k in ('prompt_tokens', 'prompt_cache_hit_tokens', 'prompt_cache_miss_tokens'))
        if prompt is not None:
            require(all(v is None or v <= prompt for v in (hit, miss)), 'cache token count exceeds prompt')
        if hit is not None and miss is not None:
            require(prompt is not None and hit + miss == prompt, 'cache hit/miss total differs from prompt')
        if request['status'] == 'settled':
            integer(request['actual'])
            require(request['actual'] == request['held'] == request['global_held'], 'settled usage accounting mismatch')
        else:
            require(request['actual'] is None and 0 < request['held'] <= request['global_held'], 'invalid unresolved reservation')
            require(request['global_held'] == config['global_request_bound'], 'unknown request lost full global hold')
    require(len({r['id'] for r in requests}) == len(requests), 'duplicate request id')
    require(sum(r['status'] == 'pending' for r in requests) <= 1, 'multiple in-flight upstream requests')
    return dict(config=config, runs=runs, requests=requests, executions=executions)


def token_usage(requests):
    """Token-weighted observed cache ratio, independent of billing settlement."""
    prompt_rows = [r for r in requests if r.get('prompt_tokens') is not None]
    output_rows = [r for r in requests if r.get('completion_tokens') is not None]
    covered = [r for r in requests if all(r.get(k) is not None for k in ('prompt_tokens', 'prompt_cache_hit_tokens', 'prompt_cache_miss_tokens'))
               and r['prompt_cache_hit_tokens'] + r['prompt_cache_miss_tokens'] == r['prompt_tokens']]
    prompt = sum(r['prompt_tokens'] for r in prompt_rows) if prompt_rows else None
    denominator = sum(r['prompt_tokens'] for r in covered) if covered else None
    hits = sum(r['prompt_cache_hit_tokens'] for r in covered) if covered else None
    misses = sum(r['prompt_cache_miss_tokens'] for r in covered) if covered else None
    return dict(requests=len(requests), prompt_usage_known_requests=len(prompt_rows),
                completion_usage_known_requests=len(output_rows), cache_known_requests=len(covered),
                cache_unknown_requests=len(requests) - len(covered),
                observed_prompt_tokens=prompt,
                observed_completion_tokens=sum(r['completion_tokens'] for r in output_rows) if output_rows else None,
                cache_covered_prompt_tokens=denominator, cache_covered_hit_tokens=hits, cache_covered_miss_tokens=misses,
                cache_hit_percent=100 * hits / denominator if denominator else None,
                cache_prompt_coverage_percent_of_observed=100 * denominator / prompt if prompt and denominator is not None else None,
                all_prompt_usage_known=bool(requests) and len(prompt_rows) == len(requests),
                all_cache_usage_known=bool(requests) and len(covered) == len(requests),
                definition='100 * sum(reported cache hits) / sum(cache-covered prompt tokens); token-weighted, not a mean of request percentages')


def request_cohorts(requests):
    """Input must retain SQLite rowid order; first means first observed per task."""
    first, later, seen = [], [], set()
    for row in requests:
        (later if row['run'] in seen else first).append(row)
        seen.add(row['run'])
    return dict(first_request_per_task=token_usage(first), subsequent_requests_per_task=token_usage(later),
                interpretation='Descriptive first-observed versus later requests per task; first does not prove a cold provider cache, which may persist across tasks.')


def root_audit_verified(row, manifest):
    witness = row.get('lineage_verification')
    if not witness or witness.get('verified') is not True: return False
    require(witness.get('v') == 1 and witness.get('full_chain_success') is True
            and witness.get('returncode') == 0 and type(witness.get('returncode')) is int
            and witness.get('timed_out') is False and witness.get('log_truncated') is False
            and witness.get('root_binary_sha256') == manifest['binary_sha']
            and witness.get('lineage_sha256') == row.get('lineage_sha256')
            and hash_value(witness.get('lineage_sha256')) and hash_value(witness.get('log_sha256')),
            'Root full-chain audit witness binding mismatch')
    integer(witness.get('log_bytes'))
    require(type(witness.get('summary')) is str
            and re.fullmatch(r'lineage ok: [1-9][0-9]* records,.*\(full,.*', witness['summary']),
            'Root full-chain success summary missing')
    return True


def task_root_usage(row):
    usage = row.get('root_usage')
    if not usage or usage.get('status') != 'observed': return None
    require(usage.get('v') == 1 and type(usage.get('rows')) is list
            and sha(usage['rows']) == usage.get('rows_sha256'), 'Root usage rows binding mismatch')
    indices = usage.get('owner_row_indices')
    if indices is None: return None  # A count alone cannot identify current-task rows.
    require(type(indices) is list and len(set(indices)) == len(indices)
            and all(type(i) is int and 0 <= i < len(usage['rows']) for i in indices)
            and type(usage.get('matched_owner_rows')) is int and len(indices) == usage['matched_owner_rows'],
            'Root usage owner-row selection mismatch')
    rows = [usage['rows'][i] for i in indices]
    start, end = usage.get('prompt_start_ms'), usage.get('prompt_end_ms')
    require(type(start) is int and type(end) is int and 0 <= start <= end, 'Root usage prompt time window missing')
    require(rows and all(type(r) is dict and r.get('kind') == 'owner' and r.get('v') == 1 for r in rows), 'Root usage lacks current-task owner turn')
    for usage_row in rows:
        stamp = datetime.fromisoformat(usage_row['start'])
        require(stamp.tzinfo is not None and start - 1000 <= stamp.timestamp() * 1000 <= end + 1000, 'Root owner turn outside current prompt window')
        require(type(usage_row.get('caps')) is list, 'Root usage capability rows missing')
        for cap in usage_row['caps']:
            require(type(cap) is dict and type(cap.get('id')) is str and cap.get('evidence') in ('root', 'zone'), 'invalid Root capability usage')
            integer(cap.get('uses'))
    return rows


def evolution_observations(manifest, receipts, facts):
    expected = {t['id']: t for t in manifest['tasks']}
    b_rows = [r for r in receipts if r['arm'] == 'B']
    audited = [r for r in b_rows if facts.get(r['id'], {}).get('audited')]
    usage_observed = [r for r in b_rows if facts.get(r['id'], {}).get('owner_usage') is not None]
    admitted = {}; admissions = []
    audited_learning = [r for r in audited if expected[r['id']]['phase'] == 'learn']
    for row in audited_learning:
        task = expected[row['id']]
        if task['phase'] != 'learn': continue
        caps = admitted.setdefault(task['block'], set())
        for change in facts[row['id']]['admissions']:
            ids = [c['id'] for c in change.get('caps', []) if c.get('evidence') == 'root' and c.get('op') in ('add', 'modify') and type(c.get('id')) is str]
            caps.update(ids)
            admissions.append(dict(receipt_id=row['id'], block=task['block'], source_sha=change.get('sha'),
                                   scope=change.get('scope'), group=change.get('group'), tuple=change.get('tuple'),
                                   root_evidenced_capability_ids=ids))
    reused = []; total_uses = 0
    eligible_transfer_receipts = []
    for row in audited:
        task = expected[row['id']]; fact = facts[row['id']]
        if task['phase'] == 'learn' or fact.get('owner_usage') is None or task['block'] not in admitted: continue
        eligible_transfer_receipts.append(row['id'])
        caps = admitted.get(task['block'], set())
        for usage_row in fact['owner_usage']:
            for cap in usage_row['caps']:
                if cap['id'] in caps and cap['evidence'] == 'root' and cap['uses'] > 0:
                    total_uses += cap['uses']
                    reused.append(dict(receipt_id=row['id'], block=task['block'], phase=task['phase'],
                                       capability_id=cap['id'], uses=cap['uses']))
    covered = bool(b_rows) and len(audited) == len(b_rows) and len(usage_observed) == len(b_rows)
    return dict(status='root_observations_verified' if covered else ('partial_root_evidence' if audited else 'unresolved'),
                b_receipts=len(b_rows), verified_root_audit_receipts=len(audited),
                current_owner_usage_receipts=len(usage_observed),
                unresolved_receipt_ids=[r['id'] for r in b_rows if not facts.get(r['id'], {}).get('audited') or facts.get(r['id'], {}).get('owner_usage') is None],
                verified_body_admission_count=len(admissions) if audited_learning else None,
                admitted_root_capability_ids=sorted(set().union(*admitted.values())) if admitted else [],
                verified_transfer_usage_receipts=len(eligible_transfer_receipts),
                verified_transfer_reuse_uses=total_uses if eligible_transfer_receipts else None,
                verified_transfer_reuse_receipts=len({r['receipt_id'] for r in reused}) if eligible_transfer_receipts else None,
                admissions=admissions, transfer_reuse=reused, causal_quality_effect='not_proven',
                reason='Trusted driver captures full Root lineage audits and Root-owned current-task usage; admission/reuse observations alone do not prove a causal quality improvement.')


def costs(requests):
    settled = [r for r in requests if r['status'] == 'settled']
    unresolved = [r for r in requests if r['status'] != 'settled']
    known = sum(r['actual'] for r in settled)
    hold = sum(r['global_held'] for r in unresolved)
    return dict(requests=len(requests), settled_requests=len(settled),
                unknown_requests=sum(r['status'] == 'unknown' for r in requests),
                pending_requests=sum(r['status'] == 'pending' for r in requests),
                known_settled_usage_uusd=known, known_settled_usage_usd=known / 1000000,
                unresolved_full_global_holds_uusd=hold, unresolved_full_global_holds_usd=hold / 1000000,
                accounted_upper_bound_uusd=known + hold,
                total_usage_cost_known=not unresolved)


def reset_observations(receipts):
    """Report trusted driver's reset scope without inventing an information reset."""
    reported = []
    retained = set()
    confounded = []
    for row in receipts:
        resets = [row.get(name) for name in ('pre_task_reset', 'runtime_reset')]
        def captured(value):
            return (isinstance(value, dict) and type(value.get('v')) is int and value['v'] == 1
                    and value.get('status') == 'completed' and value.get('ordinary_task_state_cleared') is True
                    and value.get('fresh_conversation') is True and value.get('information_free_reset') is False
                    and isinstance(value.get('retained_runtime_state'), list))
        if all(captured(value) for value in resets):
            reported.append(row['id'])
            for value in resets:
                retained.update(entry['path'] for entry in value['retained_runtime_state']
                                if isinstance(entry, dict) and entry.get('path') in ('state/kernel', 'cache', 'vault'))
        if row.get('residual_memory_confound') is True or any(isinstance(value, dict) and value.get('residual_memory_confound') is True for value in resets):
            confounded.append(row['id'])
    return dict(driver_reported_reset_before_and_after_ids=reported,
                missing_reset_receipt_ids=[row['id'] for row in receipts if row['id'] not in reported],
                all_recorded_tasks_report_resets=bool(receipts) and len(reported) == len(receipts) if receipts else None,
                retained_operational_state=sorted(retained), residual_memory_confound=True if confounded else None,
                residual_memory_confound_task_ids=confounded, information_free_reset_proven=False,
                interpretation='Driver resets ordinary task state and reconstructs admitted Body source; coupled kernel state, cache and vault remain possible memory channels. Reset receipts do not prove an information-free or causal evolution comparison.')


def analyze(manifest, receipts, ledger, provenance, *, allow_partial=False, production_verified=False):
    """Analyze validated captured state. File integrity is checked by load_run()."""
    validate_manifest(manifest)
    require(type(allow_partial) is bool and type(production_verified) is bool, 'analysis flags must be boolean')
    require(type(provenance.get('runtime_binaries')) is dict and provenance['runtime_binaries'] and all(hash_value(v) for v in provenance['runtime_binaries'].values()), 'runtime binary bindings missing')
    require(manifest['binary_sha'] in provenance['runtime_binaries'].values() and hash_value(provenance.get('source_archive_sha256')), 'source archive/root binary binding mismatch')
    expected = {t['id']: t for t in manifest['tasks']}
    seen = {}
    by_run = {run: [r for r in ledger['requests'] if r['run'] == run] for run in expected}
    execution = {r['id']: r for r in ledger['executions']}
    require(len(execution) == len(ledger['executions']), 'duplicate execution id')
    for row in receipts:
        task = expected.get(row.get('id'))
        require(task is not None and task['id'] not in seen, 'unknown/duplicate task receipt')
        rid = task['id']
        require(all(row.get(k) == manifest[k] for k in BINDINGS), 'receipt frozen source/root/protocol mismatch')
        require(all(row.get(k) == provenance[k] for k in ('runtime_binaries', 'source_archive_sha256')), 'receipt exact source/binary set mismatch')
        require(all(row.get(k) == task[k] for k in ('arm', 'phase')), 'receipt arm/phase mismatch')
        require(row.get('status') in ('completed', 'timeout') and type(row.get('passed')) is bool, 'invalid task result')
        require(not row['passed'] or row['status'] == 'completed', 'unfinished runtime cannot pass')
        wall = row.get('wall_s')
        require(type(wall) in (int, float) and math.isfinite(wall) and wall >= 0, 'invalid wall duration')
        integer(row.get('charged_uusd'))
        require(row['charged_uusd'] == sum(r['global_held'] for r in by_run[rid]), 'receipt hold differs from ledger')
        require(execution.get(rid) == dict(id=rid, status='completed', receipt_sha=sha(row)), 'receipt not bound to completed execution journal')
        seen[rid] = row
    ids = [t['id'] for t in manifest['tasks']]
    require(set(seen) == set(ids[:len(seen)]), 'partial results must be the completed preregistered prefix; selected subsets forbidden')
    complete = len(seen) == len(ids)
    require(complete or allow_partial, 'incomplete pilot requires explicit --allow-partial')
    # Interrupted next execution is recorded separately; it can have real costs.
    for rid, entry in execution.items():
        require(rid in expected, 'unknown execution id')
        if rid not in seen:
            require(len(seen) < len(ids) and rid == ids[len(seen)] and entry['status'] == 'started' and entry['receipt_sha'] is None, 'missing completed receipt or non-prefix execution')
    for request in ledger['requests']:
        if request['run'] in expected: require(request['run'] in execution, 'main request lacks execution admission')
    arms = {}
    for arm in ('A', 'B'):
        rows = [seen[t['id']] for t in manifest['tasks'] if t['id'] in seen and t['arm'] == arm]
        transfer = [r for r in rows if r['phase'] != 'learn']
        requests = [r for r in ledger['requests'] if r['run'] in expected and expected[r['run']]['arm'] == arm]
        learning = [r for r in requests if expected[r['run']]['phase'] == 'learn']
        success = sum(r['passed'] for r in transfer)
        bill = costs(requests)
        arms[arm] = dict(tasks_recorded=len(rows), transfer_recorded=len(transfer), transfer_passes=success,
                         transfer_pass_rate=success / len(transfer) if transfer else None,
                         runtime_completed=sum(r['status'] == 'completed' for r in rows),
                         median_wall_s=median(r['wall_s'] for r in rows) if rows else None,
                         median_transfer_wall_s=median(r['wall_s'] for r in transfer) if transfer else None,
                         total_cost_including_learning=bill, learning_cost=costs(learning),
                         usage_tokens=token_usage(requests), request_order_cohorts=request_cohorts(requests),
                         phases={phase: token_usage([r for r in requests if expected[r['run']]['phase'] == phase]) for phase in PHASES},
                         cost_per_transfer_success_uusd=bill['known_settled_usage_uusd'] / success if complete and bill['total_usage_cost_known'] and success else None)
    pairs = []
    block_results = []
    for block in dict.fromkeys(t['block'] for t in manifest['tasks']):
        group = [t for t in manifest['tasks'] if t['block'] == block]
        block_pairs = []
        for phase in PHASES[1:]:
            tasks = {t['arm']: t for t in group if t['phase'] == phase}
            if all(t['id'] in seen for t in tasks.values()):
                a, b = (seen[tasks[arm]['id']] for arm in ('A', 'B'))
                pair = dict(block=block, family=group[0]['family'], phase=phase,
                            a_passed=a['passed'], b_passed=b['passed'], b_minus_a=int(b['passed']) - int(a['passed']),
                            wall_difference_b_minus_a_s=b['wall_s'] - a['wall_s'])
                pairs.append(pair); block_pairs.append(pair)
        if len(block_pairs) == 2:
            block_results.append(dict(block=block, family=group[0]['family'],
                                      transfer_pass_rate_difference_b_minus_a=sum(p['b_minus_a'] for p in block_pairs) / 2))
    family_results = {}
    for family in FAMILIES:
        blocks = [r for r in block_results if r['family'] == family]
        family_results[family] = dict(complete_paired_blocks=len(blocks),
                                    paired_block_differences=[r['transfer_pass_rate_difference_b_minus_a'] for r in blocks],
                                    full_family_difference_b_minus_a=sum(r['transfer_pass_rate_difference_b_minus_a'] for r in blocks) / 2 if len(blocks) == 2 else None)
    main_ids = set(expected)
    main_requests = [r for r in ledger['requests'] if r['run'] in main_ids]
    unresolved_attribution = [r['id'] for r in receipts if r['arm'] == 'B']
    return dict(schema_version=1, complete=complete, partial_prefix=not complete,
                comparison_scope='full preregistered descriptive pilot' if complete else 'completed prefix only; no full A/B comparison',
                production_upstream_verified=production_verified,
                actual_deepseek_comparison_available=complete and production_verified and all(by_run[rid] for rid in ids),
                task_ids_without_upstream_requests=[rid for rid in ids if rid in seen and not by_run[rid]],
                protocol_sha=manifest['protocol_sha'], repo_sha=manifest['repo_sha'], binary_sha=manifest['binary_sha'],
                source_archive_sha256=provenance['source_archive_sha256'], runtime_binaries=provenance['runtime_binaries'],
                recorded_ids=ids[:len(seen)], missing_ids=ids[len(seen):],
                interrupted_execution_ids=[rid for rid in execution if rid not in seen],
                arms=arms, paired_transfers=pairs, paired_blocks=block_results, families=family_results,
                mean_transfer_difference_b_minus_a=sum(p['b_minus_a'] for p in pairs) / len(pairs) if complete else None,
                all_benchmark_cost=costs(ledger['requests']),
                cache_usage=dict(overall=token_usage(ledger['requests']), main_tasks=token_usage(main_requests),
                                 phases={phase: token_usage([r for r in main_requests if expected[r['run']]['phase'] == phase]) for phase in PHASES},
                                 request_order_cohorts=request_cohorts(main_requests)),
                non_task_cost=costs([r for r in ledger['requests'] if r['run'] not in main_ids]),
                unreceipted_task_cost=costs([r for r in ledger['requests'] if r['run'] in main_ids and r['run'] not in seen]),
                authorized_ceiling_uusd=ledger['config']['ceiling'], accounting_halted=ledger['config']['halt'],
                accounting_within_authorized_ceiling=sum(r['global_held'] for r in ledger['requests']) <= ledger['config']['ceiling'],
                evolution_attribution=dict(status='unresolved', unresolved_receipt_ids=unresolved_attribution,
                                           reason='Task receipts and transfer scores alone do not independently verify Root admission, reuse, and causal benefit.'),
                runtime_reset_observations=reset_observations(receipts),
                interpretation=['Descriptive pilot with repeated small fixtures; no significance test or population-generalization claim.',
                                'Known settled usage USD uses observed tokens at frozen peak tariffs: an estimate/upper bound that may exceed off-peak charges, not an invoice.',
                                'Pending/unknown full global holds are conservative budget reservations, not observed spending.',
                                'Arm totals include learning and any interrupted next task. Benchmark total also includes auxiliary categories.',
                                'Qualification runs and synthetic upstream counters are not DeepSeek task observations.',
                                'Retained kernel state, cache and vault are residual-memory confounds; ordinary-state reset does not isolate a causal evolution effect.'])


def load_run(config, *, allow_partial=False):
    """Validate preserved artifact files and ledger journal before reporting."""
    evidence = read_json(config['evidence'])
    manifest = evidence['manifest']; validate_manifest(manifest)
    binaries = config['binaries']; source = config['source_archive']
    require(digest(source['path']) == source['sha256'], 'source archive file mismatch')
    runtime_provenance = runtime_binary_provenance(config)
    provenance = dict(runtime_binaries=binaries, source_archive_sha256=source['sha256'])
    fixtures = read_json(config['fixtures'])
    require(hashes(fixtures) == manifest['fixtures'], 'fixture/prompt/scorer differs from frozen manifest')
    receipts = []; observed_changes = []; root_facts = {}
    output = Path(config['output'])
    for path in sorted(output.glob('*/receipt.json')):
        row = read_json(path)
        audited = root_audit_verified(row, manifest)
        root_facts[row['id']] = dict(audited=audited, owner_usage=task_root_usage(row), admissions=[])
        require(path.parent.name == row.get('id'), 'receipt directory/id mismatch')
        task = next((t for t in manifest['tasks'] if t['id'] == row['id']), None)
        require(task is not None, 'unknown task directory')
        observed = score(fixtures[task['family']][task['phase']], path.parent / 'workspace')
        require(observed == row.get('score') and row.get('passed') == (row.get('status') == 'completed' and observed['passed']), 'preserved output score differs from receipt')
        if row.get('lineage_sha256') is not None:
            require(digest(path.parent / 'lineage.jsonl') == row['lineage_sha256'], 'preserved lineage hash mismatch')
            raw_lineage = (path.parent / 'lineage.jsonl').read_bytes()
            records = [json.loads(line) for line in raw_lineage.splitlines() if line.strip()]
            tuples = {r.get('data', {}).get('id'): r.get('data', {}).get('tuple', {}) for r in records if r.get('by') == 'root' and r.get('kind') == 'tuple'}
            offset = row.get('lineage_start_bytes', 0)
            require(type(offset) is int and 0 <= offset <= len(raw_lineage), 'invalid lineage segment boundary')
            prior = raw_lineage[:offset]
            require(row.get('lineage_start_sha256') in ((hashlib.sha256(prior).hexdigest(),) if offset else (None, hashlib.sha256(b'').hexdigest())), 'lineage prefix binding mismatch')
            for record in [json.loads(line) for line in raw_lineage[offset:].splitlines() if line.strip()]:
                data = record.get('data', {})
                if record.get('by') == 'root' and record.get('kind') == 'change_open' and data.get('scope') in ('t0', 't1', 't2', 'dep'):
                    root_facts[row['id']]['admissions'].append(data)
                if record.get('by') == 'root' and record.get('kind') == 'change_open' and data.get('scope') in ('t2', 'dep'):
                    body = tuples.get(data.get('tuple'), {}).get('body', {})
                    if hash_value(data.get('sha'), 40) and hash_value(body.get('binary')) and body.get('sha') == data.get('sha'):
                        observed_changes.append(dict(receipt_id=row['id'], lineage_sha256=row['lineage_sha256'],
                            source_sha=data.get('sha'), from_source_sha=data.get('from_sha'), scope=data.get('scope'),
                            tuple=data.get('tuple'), body_binary_sha256=body.get('binary'),
                            independently_verified=audited))
        expected_source = 'genesis' if task['phase'] == 'learn' else f"{task['block']}-{task['arm']}-learned"
        require(row.get('source_snapshot') == expected_source and hash_value(row.get('source_snapshot_sha')), 'wrong/unbound arm source snapshot')
        expected_phase = 'learn' if task['arm'] == 'B' and task['phase'] == 'learn' else ('genesis' if task['phase'] == 'learn' else 'transfer')
        policy = row.get('policy', {})
        require(policy.get('phase') == expected_phase and policy.get('run_id') == task['id'] and policy.get('v') == 1 and policy.get('provider') == manifest['provider'] and policy.get('model') == manifest['model'] and policy.get('request_max_tokens') == 4096, 'runtime benchmark policy mismatch')
        require(policy.get('gateway_host') == '127.0.0.1:' + str(config['gateway_port']) and len(policy) == 7, 'runtime gateway policy mismatch')
        ceilings = read_json(path.parent / 'ceilings.json')
        policy_digest = hashlib.sha256(json.dumps(policy, sort_keys=True).encode()).hexdigest()
        require(ceilings.get('benchmark') == dict(policy, frozen=expected_phase != 'learn', policy_sha256=policy_digest), 'observed immutable Root policy mismatch')
        before, after = row.get('before_artifacts'), row.get('after_artifacts')
        require(type(before) is dict and set(before) == set(config['frozen_artifacts']) and type(after) is dict and set(before) == set(after) and all(hash_value(v) for v in [*before.values(), *after.values()]), 'frozen artifact hash set mismatch')
        require(expected_phase == 'learn' or before == after, 'frozen runtime artifacts changed')
        receipts.append(row)
    ledger = ledger_snapshot(config['ledger'], manifest)
    production = False
    pointer = config.get('production_upstream')
    if pointer:
        require(digest(pointer['path']) == pointer['sha256'], 'production upstream receipt hash mismatch')
        proof = read_json(pointer['path'])
        require(proof.get('status') == 'passed' and proof.get('upstream') == UPSTREAM and proof.get('transport') == 'gateway.upstream', 'production transport not verified')
        require(all(proof.get(k) == manifest[k] for k in BINDINGS) and all(proof.get(k) == v for k, v in provenance.items()) and proof.get('ledger_sha256') == digest(config['ledger']), 'production upstream proof binding mismatch')
        production = True
    report = analyze(manifest, receipts, ledger, provenance, allow_partial=allow_partial, production_verified=production)
    qualification = qualification_check(evidence)
    for name in ('platform', 'evolution_freeze', 'gateway_route'):
        pointer = evidence.get(name, {})
        if pointer.get('path'):
            proof = read_json(pointer['path'])
            if any(proof.get(k) != v for k, v in provenance.items()):
                qualification['blockers'].append(name + ': source/binary set differs from analyzed runtime')
    qualification['ready'] = not qualification['blockers']
    report['qualification'] = qualification
    report['actual_deepseek_comparison_available'] &= qualification['ready']
    report['runtime_binary_provenance'] = runtime_provenance
    report['observed_body_source_bindings'] = observed_changes
    report['evolution_attribution'] = evolution_observations(manifest, receipts, root_facts)
    report['artifact_integrity_verified'] = True
    report['source_artifacts'] = {str(config['evidence']): digest(config['evidence']), str(config['fixtures']): digest(config['fixtures']), str(config['ledger']): digest(config['ledger'])}
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--config', required=True, help='Owner-held runner config; output points to task receipt directory')
    parser.add_argument('--output', required=True, help='New analysis JSON file; never overwrite an earlier report')
    parser.add_argument('--allow-partial', action='store_true', help='Describe completed preregistered prefix without a full A/B claim')
    args = parser.parse_args()
    try:
        report = load_run(read_json(args.config), allow_partial=args.allow_partial)
        with Path(args.output).open('x') as stream:
            json.dump(report, stream, sort_keys=True, indent=2, allow_nan=False); stream.write('\n')
    except (OSError, ValueError, KeyError, TypeError, sqlite3.Error) as error:
        parser.exit(2, 'Analysis refused: ' + str(error) + '\n')
    print(json.dumps(dict(complete=report['complete'], actual_deepseek_comparison_available=report['actual_deepseek_comparison_available'], report=args.output)))


if __name__ == '__main__': main()
