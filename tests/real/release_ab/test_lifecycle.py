from pathlib import Path
import hashlib
import json
import tempfile
import unittest
from fixtures import build, encode, hashes, materialize, registry, score
from lifecycle import BASE, Lifecycle, clone_or_copy, tree_hash
from harness import Ledger, plan, sha
import sqlite3

class LifecycleTests(unittest.TestCase):
    def setUp(self):
        BASE.mkdir(parents=True,exist_ok=True)
        self.tmp=tempfile.TemporaryDirectory(dir=BASE)
        self.addCleanup(self.tmp.cleanup)
        self.c=Lifecycle(Path(self.tmp.name)/'test')
        self.c.initialize()
        (self.c.init_home/'body.txt').write_text('genesis')
        self.c.stopped(True)
        self.genesis=self.c.snapshot('genesis')

    def test_transfer_reset_preserves_learned_capability(self):
        task=next(t for t in plan()['tasks'] if t['arm']=='B' and t['phase']=='learn')
        self.c.restore('genesis')
        cfg=self.c.arm_config(task)
        self.assertTrue(cfg['evolution_allowed'])
        self.assertRaises(ValueError,self.c.snapshot,'bad')
        (self.c.init_home/'body.txt').write_text('learned')
        self.c.stopped(True)
        name=f"{task['block']}-B-learned"
        learned=self.c.snapshot(name)
        for phase in ('transfer-1','transfer-2'):
            self.c.restore(name)
            self.assertEqual(tree_hash(self.c.live),learned)
            cfg=self.c.arm_config(dict(task,phase=phase))
            self.assertFalse(cfg['evolution_allowed'])
            self.assertTrue(cfg['preserve_existing_capabilities'])
            (self.c.init_home/'body.txt').write_text('transfer-write')
            self.c.stopped(True)
        self.assertNotEqual(learned,self.genesis)

    def test_whole_task_admission_is_once_only(self):
        ledger=Ledger(Path(self.tmp.name)/'ledger.sqlite')
        self.addCleanup(ledger.db.close)
        ledger.initialize(plan(),10000000)
        task=next(t for t in plan()['tasks'] if t['arm']=='A' and t['phase']=='learn')
        self.c.restore('genesis')
        cfg=self.c.admit_task(ledger,task)
        self.assertEqual(cfg['root_additional_budget_uusd'],210000)
        self.assertEqual(ledger.summary()['accounted_uusd'],210000)
        self.assertRaises(ValueError,self.c.finish_task,ledger,0)
        self.c.stopped(True)
        self.c.finish_task(ledger,0)
        self.c.restore('genesis')
        self.assertRaises(sqlite3.IntegrityError,self.c.admit_task,ledger,task)

    def test_cumulative_root_money_survives_restore(self):
        usage=self.c.root_home/'egress/usage'
        usage.mkdir(parents=True)
        (usage/'current.jsonl').write_text('synthetic cost 10')
        self.c.snapshot('with-spend')
        self.assertFalse((self.c.snapshots/'with-spend/rh/egress').exists())
        (usage/'current.jsonl').write_text('synthetic cost 25')
        self.c.restore('genesis')
        self.assertEqual((usage/'current.jsonl').read_text(),'synthetic cost 25')

    def test_hash_frames_file_type_and_content_length(self):
        left=Path(self.tmp.name)/'left'; right=Path(self.tmp.name)/'right'
        left.mkdir(); right.mkdir()
        (left/'item').write_bytes(b''); (right/'item').mkdir()
        (left/'item').chmod(0o700); (right/'item').chmod(0o700)
        self.assertNotEqual(tree_hash(left),tree_hash(right))
        (left/'item').write_bytes(b'arbitrary\0framing')
        self.assertNotEqual(tree_hash(left),tree_hash(right))

    def test_arm_a_and_wrong_snapshot(self):
        task=next(t for t in plan()['tasks'] if t['arm']=='A' and t['phase']=='learn')
        self.c.restore('genesis')
        self.assertFalse(self.c.arm_config(task)['evolution_allowed'])
        self.c.stopped(True)
        self.assertRaises(ValueError,self.c.arm_config,dict(task,phase='transfer-1'))

    def test_no_live_or_symlink_homes(self):
        self.assertRaises(ValueError,Lifecycle,Path.home()/'.init')
        (self.c.live/'evil').symlink_to(Path.home())
        self.assertRaises(ValueError,self.c.snapshot,'evil')

    def test_tampered_snapshot_and_secrets(self):
        (self.c.snapshots/'genesis/ih/body.txt').write_text('tampered')
        self.assertRaises(ValueError,self.c.restore,'genesis')
        keys=self.c.root_home/'keys/providers'; keys.mkdir(parents=True)
        (keys/'test').write_text('synthetic-test-secret')
        self.assertRaises(ValueError,self.c.snapshot,'secret')

    def completed(self, ledger, output, task):
        folder=output/task['id']; folder.mkdir(parents=True)
        (folder/'workspace').mkdir(); (folder/'workspace/answer.json').write_text('[]')
        artifacts={'workspace':tree_hash(folder/'workspace',allowed_root=folder)}
        for name in ('owner-trace.json','ceilings.json','lineage.jsonl'):
            (folder/name).write_text('{}\n'); artifacts[name]=hashlib.sha256((folder/name).read_bytes()).hexdigest()
        receipt=dict(id=task['id'],owner_artifacts=artifacts)
        (folder/'receipt.json').write_text(json.dumps(receipt))
        ledger.db.execute('INSERT INTO executions VALUES(?,?,?)',(task['id'],'completed',sha(receipt)))

    def disposal_ledger(self):
        ledger=Ledger(Path(self.tmp.name)/'disposal.sqlite'); ledger.initialize(plan(),10000000)
        ledger.db.execute('CREATE TABLE executions(id TEXT PRIMARY KEY,status TEXT,receipt_sha TEXT)')
        self.addCleanup(ledger.db.close); return ledger

    def test_only_completed_hash_bound_previous_image_is_pruned_and_names_never_reuse(self):
        ledger=self.disposal_ledger(); output=Path(self.tmp.name)/'evidence'
        task=plan()['tasks'][0]
        self.c.restore('genesis'); self.c.arm_config(task)
        (self.c.init_home/'body.txt').write_text('learning outcome')
        self.c.stopped(True); self.completed(ledger,output,task)
        self.c.restore('genesis')
        unregistered=self.c.base/'previous-9999'; unregistered.mkdir(); (unregistered/'keep').write_text('not registered')
        removed=self.c.prune_previous(ledger,output)
        self.assertEqual(removed,['previous-0001'])
        self.assertTrue((self.c.base/'previous-0000').exists()) # no outcome receipt
        self.assertTrue(unregistered.exists())
        self.c.restore('genesis')
        self.assertTrue((self.c.base/'previous-0002').exists())
        self.assertEqual(len(ledger.db.execute('SELECT * FROM executions').fetchall()),1)

    def test_pruning_refuses_changed_outcome_and_running_runtime(self):
        ledger=self.disposal_ledger(); output=Path(self.tmp.name)/'evidence'; task=plan()['tasks'][0]
        self.c.restore('genesis'); self.c.arm_config(task); self.c.stopped(True)
        self.completed(ledger,output,task); self.c.restore('genesis')
        (output/task['id']/'workspace/answer.json').write_text('changed')
        self.assertRaises(ValueError,self.c.prune_previous,ledger,output)
        self.assertTrue((self.c.base/'previous-0001').exists())
        state=self.c.state(); state['stopped']=False; self.c.save(state)
        self.assertRaises(ValueError,self.c.prune_previous,ledger,output)

    def test_snapshot_eviction_requires_all_three_verified_completed_outcomes(self):
        ledger=self.disposal_ledger(); output=Path(self.tmp.name)/'evidence'
        learn=plan()['tasks'][0]; prefix=learn['id'][:-len('-learn')]
        name=prefix+'-learned'; fingerprint=self.c.snapshot(name)
        related=[t for t in plan()['tasks'] if t['id'].startswith(prefix+'-')]
        for task in related[:2]: self.completed(ledger,output,task)
        self.assertEqual(self.c.evict_completed_snapshots(ledger,output),[])
        self.completed(ledger,output,related[2])
        self.assertEqual(self.c.evict_completed_snapshots(ledger,output),[name])
        self.assertEqual(self.c.state()['snapshots'][name],fingerprint)
        self.assertEqual(self.c.state()['evicted_snapshots'][name]['sha256'],fingerprint)
        self.assertTrue((self.c.snapshots/'genesis').is_dir())
        self.assertRaises(ValueError,self.c.restore,name)
        self.assertRaises(ValueError,self.c.snapshot,name)
        self.assertEqual(self.c.evict_completed_snapshots(ledger,output),[])

    def test_copy_is_independent_inode_and_source_is_unchanged(self):
        source=Path(self.tmp.name)/'original'; target=Path(self.tmp.name)/'copy'
        source.write_text('private bytes'); clone_or_copy(source,target)
        self.assertNotEqual(source.stat().st_ino,target.stat().st_ino)
        target.write_text('changed'); self.assertEqual(source.read_text(),'private bytes')

class FixtureTests(unittest.TestCase):
    def setUp(self):
        BASE.mkdir(parents=True,exist_ok=True)
        self.tmp=tempfile.TemporaryDirectory(dir=BASE)
        self.addCleanup(self.tmp.cleanup)

    def test_deterministic_disjoint_instances(self):
        a=registry(); b=registry()
        self.assertEqual(a,b)
        for family, phases in hashes(a).items():
            self.assertEqual(len({v['fixture_sha'] for v in phases.values()}),3)

    def test_external_scoring_and_no_answer_leak(self):
        for family in ('validation','navigation','transformation','resumable'):
            fixture=build(family,42)
            ws=Path(self.tmp.name)/family
            materialize(fixture,ws)
            self.assertFalse((ws/'expected.json').exists())
            self.assertFalse(score(fixture,ws)['passed'])
            if family=='transformation':
                for name,text in fixture['files'].items():
                    (ws/name).write_text(text.replace('return old_api(', 'return new_api('))
            else: (ws/'answer.json').write_text(encode(fixture['expected']))
            self.assertTrue(score(fixture,ws)['passed'])

    def test_scorer_rejects_output_link(self):
        f=build('validation',42); ws=Path(self.tmp.name)/'w'
        materialize(f,ws)
        outside=Path(self.tmp.name)/'outside.json'; outside.write_text(encode(f['expected']))
        (ws/'answer.json').symlink_to(outside)
        self.assertFalse(score(f,ws)['passed'])

if __name__=='__main__': unittest.main()
