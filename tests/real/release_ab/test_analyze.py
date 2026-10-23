"""Synthetic offline analysis fixtures only; none are real pilot observations."""
import copy
import json
from pathlib import Path
import sqlite3
import tempfile
import unittest

from analyze import analyze, digest, ledger_snapshot, load_run, read_json, validate_manifest, runtime_binary_provenance, token_usage, root_audit_verified, task_root_usage, evolution_observations
from fixtures import hashes, registry, score
from harness import Ledger, plan, sha
from lifecycle import BASE, Lifecycle


class AnalysisTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.home = Path(self.tmp.name)
        self.fixtures = registry()
        self.manifest = dict(plan(), fixtures=hashes(self.fixtures), repo_sha='a' * 40, binary_sha='b' * 64)
        self.manifest['protocol_sha'] = sha(self.manifest)
        self.provenance = dict(runtime_binaries={'/synthetic/init-root': 'b' * 64, '/synthetic/init-kernel': 'c' * 64}, source_archive_sha256='d' * 64)
        self.path = self.home / 'ledger.sqlite'
        self.ledger = Ledger(self.path)
        self.addCleanup(self.ledger.db.close)
        self.ledger.initialize(self.manifest, 10000000)
        self.ledger.db.execute('CREATE TABLE executions(id TEXT PRIMARY KEY,status TEXT NOT NULL,receipt_sha TEXT)')

    def receipts(self, count=48, *, unknown_at=None, requests=True):
        rows = []
        for task in self.manifest['tasks'][:count]:
            if requests:
                self.ledger.reserve('synthetic:' + task['id'], task['id'], 1000, global_bound=319488)
                self.ledger.settle('synthetic:' + task['id'], None if task['order'] == unknown_at else 100)
            charged = 319488 if task['order'] == unknown_at and requests else (100 if requests else 0)
            row = dict(id=task['id'], arm=task['arm'], phase=task['phase'], status='completed',
                       passed=task['arm'] == 'B', charged_uusd=charged, wall_s=task['order'] + 1,
                       **{k: self.manifest[k] for k in ('repo_sha', 'binary_sha', 'protocol_sha')}, **self.provenance)
            self.ledger.db.execute('INSERT INTO executions VALUES (?, ?, ?)', (task['id'], 'completed', sha(row)))
            rows.append(row)
        return rows

    def state(self): return ledger_snapshot(self.path, self.manifest)

    def analyze(self, rows, **flags): return analyze(self.manifest, rows, self.state(), self.provenance, **flags)

    def rebind(self, row):
        self.ledger.db.execute('UPDATE executions SET receipt_sha=? WHERE id=?', (sha(row), row['id']))

    def test_complete_paired_descriptive_pilot_includes_learning_cost(self):
        report = self.analyze(self.receipts())
        self.assertTrue(report['complete'])
        self.assertFalse(report['production_upstream_verified'])
        self.assertFalse(report['actual_deepseek_comparison_available'])
        self.assertEqual(len(report['paired_transfers']), 16)
        self.assertEqual(len(report['paired_blocks']), 8)
        self.assertEqual(report['mean_transfer_difference_b_minus_a'], 1)
        self.assertEqual(report['arms']['B']['total_cost_including_learning']['known_settled_usage_uusd'], 2400)
        self.assertEqual(report['arms']['B']['learning_cost']['known_settled_usage_uusd'], 800)
        self.assertEqual(report['arms']['B']['cost_per_transfer_success_uusd'], 150)
        self.assertEqual(report['arms']['A']['median_wall_s'], 23.0)
        self.assertEqual(report['families']['validation']['paired_block_differences'], [1, 1])
        self.assertEqual(report['evolution_attribution']['status'], 'unresolved')
        self.assertEqual(len(report['evolution_attribution']['unresolved_receipt_ids']), 24)

    def test_unknown_full_hold_is_not_reported_as_actual_spend(self):
        report = self.analyze(self.receipts(unknown_at=1))
        cost = report['all_benchmark_cost']
        self.assertEqual(cost['known_settled_usage_uusd'], 4700)
        self.assertEqual(cost['unresolved_full_global_holds_uusd'], 319488)
        self.assertEqual(cost['accounted_upper_bound_uusd'], 324188)
        self.assertEqual(cost['unknown_requests'], 1)
        arm = self.manifest['tasks'][1]['arm']
        self.assertIsNone(report['arms'][arm]['cost_per_transfer_success_uusd'])

    def test_partial_requires_explicit_prefix_and_suppresses_full_comparison(self):
        rows = self.receipts(6)
        self.assertRaisesRegex(ValueError, 'allow-partial', self.analyze, rows)
        report = self.analyze(rows, allow_partial=True, production_verified=True)
        self.assertTrue(report['partial_prefix'])
        self.assertFalse(report['actual_deepseek_comparison_available'])
        self.assertIsNone(report['mean_transfer_difference_b_minus_a'])
        self.assertEqual(len(report['paired_transfers']), 2)
        self.assertIsNone(report['families']['validation']['full_family_difference_b_minus_a'])
        self.assertIsNone(report['arms']['B']['cost_per_transfer_success_uusd'])
        self.assertEqual(len(report['missing_ids']), 42)
        self.assertRaisesRegex(ValueError, 'prefix', self.analyze, rows[1:], allow_partial=True)

    def test_empty_run_does_not_invent_observations(self):
        report = self.analyze([], allow_partial=True)
        self.assertEqual(report['arms']['A']['tasks_recorded'], 0)
        self.assertIsNone(report['arms']['A']['median_wall_s'])
        self.assertEqual(report['all_benchmark_cost']['known_settled_usage_uusd'], 0)
        self.assertEqual(len(report['missing_ids']), 48)
        self.assertIsNone(report['cache_usage']['overall']['cache_hit_percent'])
        self.assertIsNone(report['runtime_reset_observations']['residual_memory_confound'])

    def test_retained_operational_memory_is_explicit_without_causal_reset_claim(self):
        rows = self.receipts(2)
        reset = dict(v=1, status='completed', ordinary_task_state_cleared=True, fresh_conversation=True,
                     residual_memory_confound=True, information_free_reset=False,
                     retained_runtime_state=[dict(path=name, present=True, sha256='f'*64, entries=1)
                                            for name in ('state/kernel', 'cache', 'vault')])
        rows[0].update(pre_task_reset=reset, runtime_reset=reset, residual_memory_confound=True)
        self.rebind(rows[0])
        observations = self.analyze(rows, allow_partial=True)['runtime_reset_observations']
        self.assertEqual(observations['driver_reported_reset_before_and_after_ids'], [rows[0]['id']])
        self.assertEqual(observations['missing_reset_receipt_ids'], [rows[1]['id']])
        self.assertTrue(observations['residual_memory_confound'])
        self.assertFalse(observations['all_recorded_tasks_report_resets'])
        self.assertFalse(observations['information_free_reset_proven'])
        self.assertEqual(observations['retained_operational_state'], ['cache', 'state/kernel', 'vault'])

    def test_token_weighted_cache_ratio_and_first_vs_later_task_cohorts(self):
        rows = self.receipts(6, requests=False)
        task_id = rows[0]['id']
        for request, prompt, hit in [('z-first', 100, 0), ('a-later', 900, 900)]:
            self.ledger.reserve(request, task_id, 1000, global_bound=319488)
            self.ledger.settle(request, 100, dict(prompt_tokens=prompt, completion_tokens=5,
                prompt_cache_hit_tokens=hit, prompt_cache_miss_tokens=prompt - hit))
        rows[0]['charged_uusd'] = 200; self.rebind(rows[0])
        report = self.analyze(rows, allow_partial=True)
        cache = report['cache_usage']
        self.assertEqual(cache['overall']['cache_hit_percent'], 90)
        self.assertEqual(cache['phases']['learn']['cache_hit_percent'], 90)
        self.assertIsNone(cache['phases']['transfer-1']['cache_hit_percent'])
        cohorts = cache['request_order_cohorts']
        self.assertEqual(cohorts['first_request_per_task']['cache_hit_percent'], 0)
        self.assertEqual(cohorts['subsequent_requests_per_task']['cache_hit_percent'], 100)
        self.assertIn('does not prove a cold', cohorts['interpretation'])
        self.assertEqual(report['arms'][rows[0]['arm']]['usage_tokens']['cache_hit_percent'], 90)
        requests = self.state()['requests']
        self.assertEqual([r['id'] for r in requests], ['z-first', 'a-later'])
        self.assertLess(requests[0]['request_sequence'], requests[1]['request_sequence'])

    def test_known_tokens_with_unknown_cost_are_not_dropped_or_called_settled(self):
        rows = self.receipts(1, requests=False)
        self.ledger.reserve('usage-only', rows[0]['id'], 1000, global_bound=319488)
        self.ledger.settle('usage-only', None, dict(prompt_tokens=200, completion_tokens=10,
            prompt_cache_hit_tokens=150, prompt_cache_miss_tokens=50))
        rows[0]['charged_uusd'] = 319488; self.rebind(rows[0])
        report = self.analyze(rows, allow_partial=True)
        self.assertEqual(report['cache_usage']['overall']['cache_hit_percent'], 75)
        self.assertEqual(report['cache_usage']['overall']['cache_known_requests'], 1)
        self.assertEqual(report['all_benchmark_cost']['known_settled_usage_uusd'], 0)
        self.assertEqual(report['all_benchmark_cost']['unresolved_full_global_holds_uusd'], 319488)

    def test_partial_and_missing_cache_usage_stay_unknown_with_explicit_coverage(self):
        usage = token_usage([dict(prompt_tokens=100, completion_tokens=3, prompt_cache_hit_tokens=90, prompt_cache_miss_tokens=10),
                             dict(prompt_tokens=900, completion_tokens=5, prompt_cache_hit_tokens=800, prompt_cache_miss_tokens=None),
                             dict(prompt_tokens=None, completion_tokens=None, prompt_cache_hit_tokens=None, prompt_cache_miss_tokens=None)])
        self.assertEqual(usage['cache_hit_percent'], 90)
        self.assertEqual(usage['cache_known_requests'], 1)
        self.assertEqual(usage['cache_unknown_requests'], 2)
        self.assertEqual(usage['cache_prompt_coverage_percent_of_observed'], 10)
        self.assertFalse(usage['all_prompt_usage_known'])
        self.assertFalse(usage['all_cache_usage_known'])
        self.assertIsNone(token_usage([{}])['cache_hit_percent'])
        self.assertIsNone(token_usage([])['cache_hit_percent'])

    def test_old_ledger_has_null_usage_without_read_only_migration(self):
        self.receipts(1)
        self.ledger.db.executescript('''ALTER TABLE requests RENAME TO new_requests;
            CREATE TABLE requests(id TEXT PRIMARY KEY,run TEXT NOT NULL,held INTEGER NOT NULL,
                actual INTEGER,status TEXT NOT NULL,global_held INTEGER NOT NULL);
            INSERT INTO requests SELECT id,run,held,actual,status,global_held FROM new_requests;
            DROP TABLE new_requests;''')
        before = self.path.read_bytes(); snapshot = self.state()
        self.assertEqual(self.path.read_bytes(), before)
        self.assertIsNone(snapshot['requests'][0]['prompt_tokens'])
        self.assertIsNone(snapshot['requests'][0]['prompt_cache_hit_tokens'])
        self.assertEqual(token_usage(snapshot['requests'])['cache_unknown_requests'], 1)

    def test_root_audit_requires_exact_root_lineage_and_full_success(self):
        row = dict(lineage_sha256='f' * 64)
        row['lineage_verification'] = dict(v=1, verified=True, full_chain_success=True, returncode=0,
            timed_out=False, log_truncated=False, root_binary_sha256=self.manifest['binary_sha'],
            lineage_sha256=row['lineage_sha256'], log_sha256='e' * 64, log_bytes=99,
            summary='lineage ok: 10 records, head abc (full, signatures verified)')
        self.assertTrue(root_audit_verified(row, self.manifest))
        for update in (dict(returncode=True), dict(root_binary_sha256='0' * 64), dict(full_chain_success=False), dict(log_truncated=True)):
            bad = copy.deepcopy(row); bad['lineage_verification'].update(update)
            self.assertRaises(ValueError, root_audit_verified, bad, self.manifest)
        row['lineage_verification']['verified'] = False
        self.assertFalse(root_audit_verified(row, self.manifest))

    def test_usage_selects_only_exact_current_owner_rows_not_background_or_old(self):
        now = 1000000
        rows = [dict(v=1, kind='owner', start='1970-01-01T00:00:00+00:00', caps=[dict(id='skill/x', evidence='root', uses=99)]),
                dict(v=1, kind='background', start='1970-01-01T00:16:40+00:00', caps=[]),
                dict(v=1, kind='owner', start='1970-01-01T00:16:40+00:00', caps=[dict(id='skill/x', evidence='root', uses=2)])]
        usage = dict(v=1, status='observed', rows=rows, rows_sha256=sha(rows), owner_row_indices=[2],
                     matched_owner_rows=1, prompt_start_ms=now, prompt_end_ms=now + 10000)
        self.assertEqual(task_root_usage(dict(root_usage=usage)), rows[2:])
        for index in (0, 1):
            bad = dict(usage, owner_row_indices=[index])
            self.assertRaises(ValueError, task_root_usage, dict(root_usage=bad))
        no_selection = dict(usage); del no_selection['owner_row_indices']
        self.assertIsNone(task_root_usage(dict(root_usage=no_selection)))
        self.assertIsNone(task_root_usage(dict(root_usage={'status': 'missing'})))

    def test_verified_admission_reuse_counts_do_not_claim_causal_quality_effect(self):
        block = self.manifest['tasks'][0]['block']
        tasks = [t for t in self.manifest['tasks'] if t['block'] == block and t['arm'] == 'B']
        rows = [dict(id=t['id'], arm='B') for t in tasks]
        facts = {t['id']: dict(audited=True, owner_usage=[dict(caps=[])], admissions=[]) for t in tasks}
        facts[tasks[0]['id']]['admissions'] = [dict(scope='t0', sha='a' * 40, tuple='tuple-new', group=1,
            caps=[dict(id='skill/learned', evidence='root', op='add')])]
        facts[tasks[1]['id']]['owner_usage'] = [dict(caps=[dict(id='skill/learned', evidence='root', uses=3),
            dict(id='skill/learned', evidence='zone', uses=50), dict(id='skill/other', evidence='root', uses=90)])]
        report = evolution_observations(self.manifest, rows, facts)
        self.assertEqual(report['status'], 'root_observations_verified')
        self.assertEqual(report['verified_body_admission_count'], 1)
        self.assertEqual(report['verified_transfer_reuse_uses'], 3)
        self.assertEqual(report['verified_transfer_reuse_receipts'], 1)
        self.assertEqual(report['causal_quality_effect'], 'not_proven')
        facts[tasks[0]['id']]['audited'] = False
        report = evolution_observations(self.manifest, rows, facts)
        self.assertIsNone(report['verified_body_admission_count'])
        self.assertIsNone(report['verified_transfer_reuse_uses'])
        self.assertIn(tasks[0]['id'], report['unresolved_receipt_ids'])

    def test_full_zero_request_receipts_are_not_actual_deepseek_comparison(self):
        report = self.analyze(self.receipts(requests=False), production_verified=True)
        self.assertTrue(report['complete'])
        self.assertFalse(report['actual_deepseek_comparison_available'])
        self.assertEqual(len(report['task_ids_without_upstream_requests']), 48)

    def test_interrupted_next_task_cost_and_auxiliary_cost_remain_visible(self):
        rows = self.receipts(1)
        next_id = self.manifest['tasks'][1]['id']
        self.ledger.db.execute('INSERT INTO executions VALUES (?, ?, NULL)', (next_id, 'started'))
        self.ledger.reserve('synthetic:interrupted', next_id, 700, global_bound=319488)
        report = self.analyze(rows, allow_partial=True)
        self.assertEqual(report['interrupted_execution_ids'], [next_id])
        self.assertEqual(report['unreceipted_task_cost']['pending_requests'], 1)
        self.assertEqual(report['all_benchmark_cost']['accounted_upper_bound_uusd'], 319588)
        self.ledger.settle('synthetic:interrupted', None)
        self.ledger.add_run('synthetic:smoke', 'smoke', 10000)
        self.ledger.reserve('synthetic:aux', 'synthetic:smoke', 1000, global_bound=319488)
        self.ledger.settle('synthetic:aux', 500)
        report = self.analyze(rows, allow_partial=True)
        self.assertEqual(report['non_task_cost']['known_settled_usage_uusd'], 500)
        self.assertEqual(report['all_benchmark_cost']['known_settled_usage_uusd'], 600)

    def test_frozen_selection_and_duplicate_binding_attacks_rejected(self):
        rows = self.receipts(2)
        self.assertRaisesRegex(ValueError, 'duplicate', self.analyze, rows + rows[:1], allow_partial=True)
        for key, value in [('protocol_sha', 'f' * 64), ('source_archive_sha256', 'e' * 64), ('binary_sha', 'f' * 64), ('arm', 'B' if rows[0]['arm'] == 'A' else 'A')]:
            bad = copy.deepcopy(rows); bad[0][key] = value; self.rebind(bad[0])
            with self.subTest(key=key): self.assertRaises(ValueError, self.analyze, bad, allow_partial=True)
            self.rebind(rows[0])
        bad_manifest = copy.deepcopy(self.manifest)
        bad_manifest['tasks'] = bad_manifest['tasks'][:2]
        bad_manifest['protocol_sha'] = sha({k: v for k, v in bad_manifest.items() if k != 'protocol_sha'})
        self.assertRaisesRegex(ValueError, 'preregistered', validate_manifest, bad_manifest)

    def test_bad_results_and_unjournaled_receipts_rejected(self):
        rows = self.receipts(1)
        for update in (dict(status='timeout', passed=True), dict(wall_s=float('nan')), dict(wall_s=True), dict(charged_uusd=0), dict(passed=1)):
            bad = copy.deepcopy(rows); bad[0].update(update)
            with self.subTest(update=update): self.assertRaises(ValueError, self.analyze, bad, allow_partial=True)
        rows[0]['wall_s'] = 123
        self.assertRaisesRegex(ValueError, 'journal', self.analyze, rows, allow_partial=True)

    def test_read_only_ledger_cannot_release_unknown_hold_or_alter_protocol(self):
        self.receipts(1, unknown_at=0)
        before = self.path.read_bytes()
        ledger_snapshot(self.path, self.manifest)
        self.assertEqual(self.path.read_bytes(), before)
        self.ledger.db.execute("UPDATE requests SET global_held=held")
        self.assertRaisesRegex(ValueError, 'full global hold', self.state)
        self.ledger.db.execute('UPDATE requests SET global_held=319488')
        self.ledger.db.execute("UPDATE config SET v='\"wrong\"' WHERE k='protocol'")
        self.assertRaisesRegex(ValueError, 'protocol', self.state)

    def test_missing_completed_receipt_and_main_execution_selection_rejected(self):
        rows = self.receipts(2)
        self.assertRaisesRegex(ValueError, 'missing completed receipt', self.analyze, rows[:1], allow_partial=True)
        self.ledger.db.execute('DELETE FROM executions WHERE id=?', (rows[1]['id'],))
        self.assertRaisesRegex(ValueError, 'execution admission', self.analyze, rows[:1], allow_partial=True)

    def test_strict_json_rejects_duplicate_and_nonfinite_values(self):
        path = self.home / 'invalid.json'
        for payload in ('{"id":1,"id":2}', '{"wall_s":NaN}'):
            path.write_text(payload)
            self.assertRaises(ValueError, read_json, path)

    def file_config(self):
        # A synthetic preserved-artifact prefix for integrity checks, never real.
        root_bin = self.home / 'init-root'; root_bin.write_bytes(b'synthetic binary')
        archive = self.home / 'source.tar'; archive.write_bytes(b'synthetic source')
        self.manifest['binary_sha'] = digest(root_bin)
        self.manifest['protocol_sha'] = sha({k: v for k, v in self.manifest.items() if k != 'protocol_sha'})
        self.ledger.db.execute('UPDATE config SET v=? WHERE k=?', (json.dumps(sha(self.manifest)), 'protocol'))
        self.provenance = dict(runtime_binaries={str(root_bin): digest(root_bin)}, source_archive_sha256=digest(archive))
        rows = self.receipts(1, requests=False)
        task = self.manifest['tasks'][0]; row = rows[0]
        folder = self.home / 'tasks' / task['id']; (folder / 'workspace').mkdir(parents=True)
        observed = score(self.fixtures[task['family']][task['phase']], folder / 'workspace')
        policy = dict(v=1, run_id=task['id'], phase='learn' if task['arm'] == 'B' else 'genesis', gateway_host='127.0.0.1:9001', provider='deepseek', model='deepseek-flash', request_max_tokens=4096)
        row.update(score=observed, passed=False, source_snapshot='genesis', source_snapshot_sha='f' * 64, policy=policy, before_artifacts={'body': 'a' * 64}, after_artifacts={'body': 'a' * 64})
        self.rebind(row)
        (folder / 'receipt.json').write_text(json.dumps(row))
        import hashlib
        (folder / 'ceilings.json').write_text(json.dumps({'benchmark': dict(policy, frozen=policy['phase'] != 'learn', policy_sha256=hashlib.sha256(json.dumps(policy, sort_keys=True).encode()).hexdigest())}))
        evidence = self.home / 'evidence.json'; evidence.write_text(json.dumps({'manifest': self.manifest}))
        fixtures = self.home / 'fixtures.json'; fixtures.write_text(json.dumps(self.fixtures))
        config = dict(evidence=str(evidence), fixtures=str(fixtures), ledger=str(self.path), output=str(folder.parent), binaries=self.provenance['runtime_binaries'], source_archive={'path': str(archive), 'sha256': digest(archive)}, frozen_artifacts=['body'], gateway_port=9001)
        return config, folder

    def test_preserved_artifact_integrity_does_not_imply_production_or_qualification(self):
        config, folder = self.file_config()
        report = load_run(config, allow_partial=True)
        self.assertTrue(report['artifact_integrity_verified'])
        self.assertFalse(report['production_upstream_verified'])
        self.assertFalse(report['qualification']['ready'])
        (folder / 'workspace' / 'answer.json').write_text('[]')
        self.assertRaisesRegex(ValueError, 'output score', load_run, config, allow_partial=True)

    def test_source_binary_and_actual_root_policy_tampering_rejected(self):
        config, folder = self.file_config()
        archive = Path(config['source_archive']['path']); original = archive.read_bytes(); archive.write_bytes(b'changed')
        self.assertRaisesRegex(ValueError, 'source archive', load_run, config, allow_partial=True)
        archive.write_bytes(original)
        ceilings = read_json(folder / 'ceilings.json'); ceilings['benchmark']['frozen'] = 'yes'
        (folder / 'ceilings.json').write_text(json.dumps(ceilings))
        self.assertRaisesRegex(ValueError, 'Root policy', load_run, config, allow_partial=True)

    def test_production_proof_is_exact_and_cannot_bypass_incomplete_or_unqualified(self):
        config, _ = self.file_config()
        proof = dict(status='passed', upstream='https://api.deepseek.com/chat/completions', transport='gateway.upstream',
                     ledger_sha256=digest(self.path), **self.provenance,
                     **{k: self.manifest[k] for k in ('repo_sha', 'binary_sha', 'protocol_sha')})
        path = self.home / 'synthetic-production-proof.json'; path.write_text(json.dumps(proof))
        config['production_upstream'] = dict(path=str(path), sha256=digest(path))
        report = load_run(config, allow_partial=True)
        self.assertTrue(report['production_upstream_verified'])
        self.assertFalse(report['actual_deepseek_comparison_available'])
        proof['ledger_sha256'] = '0' * 64; path.write_text(json.dumps(proof)); config['production_upstream']['sha256'] = digest(path)
        self.assertRaisesRegex(ValueError, 'binding mismatch', load_run, config, allow_partial=True)

    def genesis_binary_config(self):
        BASE.mkdir(parents=True, exist_ok=True)
        temporary = tempfile.TemporaryDirectory(dir=BASE); self.addCleanup(temporary.cleanup)
        life = Lifecycle(Path(temporary.name) / 'runtime'); life.initialize()
        body = life.init_home / 'bin/init-body'; body.parent.mkdir(); body.write_bytes(b'initial body')
        kernel = life.init_home / 'bin/init-kernel'; kernel.write_bytes(b'immutable kernel')
        root = life.root_home / 'bin/init-root'; root.parent.mkdir(); root.write_bytes(b'immutable Root')
        config = dict(base=str(life.base), binaries={str(p): digest(p) for p in (root, kernel, body)})
        life.stopped(True); life.snapshot('genesis')
        return config, life, body, root, kernel

    def test_legitimate_body_evolution_uses_verified_immutable_genesis(self):
        config, life, body, _, _ = self.genesis_binary_config()
        body.write_bytes(b'evolved Body binary')
        proof = runtime_binary_provenance(config)
        self.assertEqual(proof['evolved_body_binary']['current_sha256'], digest(body))
        self.assertEqual(proof['evolved_body_binary']['initial_sha256'], config['binaries'][str(body)])
        self.assertEqual(proof['immutable_genesis_snapshot']['tree_sha256'], life.state()['snapshots']['genesis'])
        (life.snapshots / 'genesis/ih/bin/init-body').write_bytes(b'tampered initial body')
        self.assertRaisesRegex(ValueError, 'Genesis snapshot', runtime_binary_provenance, config)

    def test_root_and_kernel_binary_mutation_never_qualify_as_body_evolution(self):
        config, _, _, root, kernel = self.genesis_binary_config()
        for path in (root, kernel):
            original = path.read_bytes(); path.write_bytes(b'changed immutable binary')
            self.assertRaisesRegex(ValueError, 'immutable Root/Kernel', runtime_binary_provenance, config)
            path.write_bytes(original)

    def test_evolved_body_symlink_cannot_hash_external_owner_file(self):
        config, _, body, _, _ = self.genesis_binary_config()
        outside = self.home / 'external-private'; outside.write_bytes(b'private owner data')
        body.unlink(); body.symlink_to(outside)
        self.assertRaisesRegex(ValueError, 'escapes live home', runtime_binary_provenance, config)

    def test_reported_body_source_binding_is_only_current_segment_and_unverified(self):
        config, folder = self.file_config()
        source = '1' * 40; binary = '2' * 64
        prior = json.dumps(dict(by='root', kind='change_open', data=dict(scope='t2', sha='3' * 40, tuple='old'))).encode() + b'\n'
        segment = [dict(by='root', kind='change_open', data=dict(scope='t2', sha=source, from_sha='4' * 40, tuple='new')),
                   dict(by='root', kind='tuple', data=dict(id='new', tuple=dict(body=dict(sha=source, binary=binary))))]
        raw = prior + b''.join(json.dumps(row).encode() + b'\n' for row in segment)
        lineage = folder / 'lineage.jsonl'; lineage.write_bytes(raw)
        row = read_json(folder / 'receipt.json')
        import hashlib
        row.update(lineage_sha256=digest(lineage), lineage_start_bytes=len(prior), lineage_start_sha256=hashlib.sha256(prior).hexdigest())
        self.rebind(row); (folder / 'receipt.json').write_text(json.dumps(row))
        report = load_run(config, allow_partial=True)
        changes = report['observed_body_source_bindings']
        self.assertEqual(len(changes), 1)
        self.assertEqual(changes[0]['source_sha'], source)
        self.assertEqual(changes[0]['body_binary_sha256'], binary)
        self.assertFalse(changes[0]['independently_verified'])
        self.assertEqual(report['evolution_attribution']['status'], 'unresolved')


if __name__ == '__main__': unittest.main()
