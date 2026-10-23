import json
from pathlib import Path
import tempfile
import unittest
import sqlite3
from concurrent.futures import ThreadPoolExecutor
from harness import Ledger, analyze, freeze, plan, sha, worst_case_uusd

REPO = Path(__file__).resolve().parents[3]

class ProtocolTests(unittest.TestCase):
    def setUp(self):
        home = REPO / 'target/test-homes/release-ab'
        home.mkdir(parents=True, exist_ok=True)
        self.tmp = tempfile.TemporaryDirectory(dir=home)
        self.addCleanup(self.tmp.cleanup)
        self.path = Path(self.tmp.name) / 'budget.sqlite'
        self.manifest = plan()
        self.ledger = Ledger(self.path)
        self.ledger.initialize(self.manifest, 13000000)
        self.addCleanup(self.ledger.db.close)
        self.run = self.manifest['tasks'][0]['id']

    def test_frozen_equal_design(self):
        p = plan()
        self.assertEqual(p, plan())
        self.assertEqual(len(p['tasks']), 48)
        self.assertEqual(sum(t['cap_uusd'] for t in p['tasks']), 7200000)
        for family in ('validation', 'navigation', 'transformation', 'resumable'):
            first = [t['arm'] for t in p['tasks'] if t['family'] == family and t['phase'] == 'learn']
            self.assertEqual(first[0], first[3]); self.assertEqual(first[1], first[2])
        for arm in ('A', 'B'):
            self.assertEqual(sum(t['cap_uusd'] for t in p['tasks'] if t['arm'] == arm), 3600000)

    def test_reservation_and_unknown_bill(self):
        self.ledger.reserve('r1', self.run, 200000)
        self.assertRaises(ValueError, self.ledger.reserve, 'r2', self.run, 1)
        self.ledger.settle('r1')
        self.assertEqual(self.ledger.summary()['accounted_uusd'], 200000)
        self.assertRaises(ValueError, self.ledger.reserve, 'r2', self.run, 10001)
        self.ledger.reserve('r2', self.run, 10000)
        self.ledger.settle('r2', 5000)
        self.assertEqual(self.ledger.summary()['accounted_uusd'], 205000)
        self.assertRaises(ValueError, self.ledger.settle, 'r1', 0)

    def test_cache_totals_are_token_weighted_and_unknown_fields_stay_null(self):
        for name,prompt,hit,miss in [('short',100,99,1),('long',900,0,900),('unknown',1000,None,None)]:
            self.ledger.reserve(name,self.run,1000)
            self.ledger.settle(name,10,reported_usage=dict(prompt_tokens=prompt,completion_tokens=3,
                prompt_cache_hit_tokens=hit,prompt_cache_miss_tokens=miss))
        result=self.ledger.cache_summary()
        self.assertAlmostEqual(result['cache_hit_percent'],9.9)
        self.assertEqual(result['cache_prompt_coverage_percent'],50)
        self.assertEqual((result['cache_known_requests'],result['cache_unknown_requests']),(2,1))
        self.assertEqual(self.ledger.db.execute("SELECT prompt_cache_hit_tokens,prompt_cache_miss_tokens FROM requests WHERE id='unknown'").fetchone(),(None,None))
    def test_no_cache_observation_is_unknown_not_zero_percent(self):
        self.ledger.reserve('u',self.run,1000); self.ledger.settle('u',10,reported_usage=dict(prompt_tokens=100,completion_tokens=1))
        result=self.ledger.cache_summary()
        self.assertIsNone(result['cache_hit_percent']); self.assertIsNone(result['prompt_cache_hit_tokens']); self.assertIsNone(result['prompt_cache_miss_tokens'])
        self.assertEqual(result['cache_unknown_requests'],1)
    def test_invalid_cache_counts_never_release_pending_reservation(self):
        self.ledger.reserve('u',self.run,1000)
        for hit,miss in [(True,99),(-1,101),(101,0),(99,99)]:
            self.assertRaises(ValueError,self.ledger.settle,'u',1,dict(prompt_tokens=100,completion_tokens=1,
                prompt_cache_hit_tokens=hit,prompt_cache_miss_tokens=miss))
        self.assertEqual(self.ledger.summary()['unresolved'],1)
    def test_old_ledger_migration_preserves_holds_and_adds_unknown_token_columns(self):
        path=Path(self.tmp.name)/'legacy.sqlite'; old=sqlite3.connect(path)
        old.execute('CREATE TABLE requests(id TEXT PRIMARY KEY,run TEXT,held INTEGER,actual INTEGER,status TEXT,global_held INTEGER)')
        old.execute("INSERT INTO requests VALUES('legacy',?,10,NULL,'unknown',319488)",(self.run,)); old.commit(); old.close()
        migrated=Ledger(path); self.addCleanup(migrated.db.close)
        row=migrated.db.execute('SELECT held,global_held,prompt_tokens,completion_tokens,prompt_cache_hit_tokens,prompt_cache_miss_tokens FROM requests').fetchone()
        self.assertEqual(row,(10,319488,None,None,None,None))

    def test_overrun_halts_durably(self):
        self.ledger.reserve('r1', self.run, 1)
        self.assertRaises(ValueError, self.ledger.settle, 'r1', 2)
        again = Ledger(self.path)
        self.addCleanup(again.db.close)
        self.assertTrue(again.summary()['halted'])
        self.assertRaises(ValueError, again.reserve, 'r2', self.run, 1)

    def test_parallel_reservations_serialize(self):
        def reserve(i):
            led = Ledger(self.path)
            try:
                led.reserve(f'r{i}', self.run, 1000)
                return True
            except ValueError:
                return False
            finally:
                led.db.close()
        with ThreadPoolExecutor(max_workers=4) as pool:
            results = list(pool.map(reserve, range(4)))
        self.assertEqual(sum(results), 1)

    def test_envelope_and_account_floor(self):
        self.ledger.add_run('s', 'smoke', 900000)
        self.assertRaises(ValueError, self.ledger.reserve, 's1', 's', 400001)
        small = Ledger(Path(self.tmp.name) / 'small.sqlite')
        self.addCleanup(small.db.close)
        small.initialize(plan(), 50000, 10000)
        self.assertRaises(ValueError, small.reserve, 'x', self.run, 40001)
        small.reserve('x', self.run, 40000)

    def test_integer_money_and_rounding(self):
        for value in (-1, 0.5, True):
            self.assertRaises(ValueError, self.ledger.reserve, 'r', self.run, value)
        self.assertEqual(worst_case_uusd(1, 1, 300000, 1200000), 2)

    def test_analysis_does_not_invent_unrun_results(self):
        report = analyze(self.manifest, [])
        self.assertFalse(report['complete'])
        self.assertIsNone(report['mean_transfer_delta'])
        self.assertEqual(len(report['missing_ids']), 48)

    def receipts(self):
        return [dict(id=t['id'], protocol_sha=sha(self.manifest), arm=t['arm'], phase=t['phase'],
                     charged_uusd=1000, passed=t['arm']=='B', status='completed') for t in self.manifest['tasks']]

    def test_analysis_all_arms_and_learning_cost(self):
        rows = self.receipts()
        report = analyze(self.manifest, rows)
        self.assertTrue(report['complete'])
        self.assertEqual(report['mean_transfer_delta'], 1)
        self.assertEqual(report['arms']['B']['cost_per_success_uusd'], 1500)
        self.assertRaises(ValueError, analyze, self.manifest, rows + rows[:1])
        rows[0]['protocol_sha'] = 'wrong'
        self.assertRaises(ValueError, analyze, self.manifest, rows)

    def test_freeze_hash_binding(self):
        binary = Path(self.tmp.name) / 'mock-binary'
        binary.write_bytes(b'mock only')
        fixtures = {f: {p: {key: 'a'*64 for key in ('fixture_sha', 'scorer_sha', 'prompt_sha')}
                       for p in ('learn', 'transfer-1', 'transfer-2')}
                    for f in ('validation', 'navigation', 'transformation', 'resumable')}
        manifest = freeze(plan(), fixtures, binary, 'b'*40)
        self.assertFalse(analyze(manifest, [])['complete'])
        manifest['model'] = 'changed'
        self.assertRaises(ValueError, analyze, manifest, [])

    def test_failed_run_cannot_claim_pass(self):
        rows = self.receipts()
        rows[-1]['passed'] = True
        rows[-1]['status'] = 'cap_stop'
        self.assertRaises(ValueError, analyze, self.manifest, rows)

if __name__ == '__main__': unittest.main()
