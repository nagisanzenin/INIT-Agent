import hashlib
import os
import shutil
import subprocess
import json
from pathlib import Path
import socket
import sqlite3
import tempfile
import threading
import time
from datetime import datetime, timezone
from types import SimpleNamespace
import unittest
from unittest.mock import patch
from harness import Ledger, plan
from lifecycle import BASE, Lifecycle, tree_hash
from runner import assert_policy, claim, run_prompt, write_once, usage_snapshot, wait_for_usage, usage_segment, verify_lineage, sha, reset_runtime, current_tuple, complete_execution

class RunnerTests(unittest.TestCase):
    def setUp(self):
        BASE.mkdir(parents=True,exist_ok=True)
        self.tmp=tempfile.TemporaryDirectory(dir=BASE)
        self.addCleanup(self.tmp.cleanup)
        self.home=Path(self.tmp.name)
    def ledger(self):
        ledger=Ledger(self.home/'ledger.sqlite'); ledger.initialize(plan(),10000000)
        self.addCleanup(ledger.db.close); return ledger
    def test_execution_claim_survives_crash_and_never_reserves_whole_task(self):
        ledger=self.ledger(); task=plan()['tasks'][0]; claim(ledger,task)
        self.assertEqual(ledger.summary()['accounted_uusd'],0)
        self.assertRaises(ValueError,claim,ledger,task)
        self.assertRaises(ValueError,claim,ledger,plan()['tasks'][1])
        # Actual external gateway per-request admission remains possible.
        ledger.reserve('request-once',task['id'],100)
        self.assertEqual(ledger.summary()['accounted_uusd'],100)
    def test_pending_or_halt_stops_execution(self):
        ledger=self.ledger(); task=plan()['tasks'][0]
        ledger.reserve('crashed',task['id'],100)
        self.assertRaises(ValueError,claim,ledger,task)
        ledger.settle('crashed',None)
        ledger.db.execute("UPDATE config SET v='true' WHERE k='halt'")
        self.assertRaises(ValueError,claim,ledger,task)
    def test_root_policy_is_exact_and_bound_to_raw_bytes(self):
        policy=dict(v=1,run_id='task',phase='transfer',gateway_host='127.0.0.1:9000',provider='deepseek',model='deepseek-flash',request_max_tokens=4096)
        raw=json.dumps(policy).encode()
        stats={'benchmark':dict(policy,frozen=True,policy_sha256=hashlib.sha256(raw).hexdigest())}
        assert_policy(stats,policy,raw)
        self.assertRaises(ValueError,assert_policy,stats,policy,raw+b' ')
        stats['benchmark']['frozen']=False
        self.assertRaises(ValueError,assert_policy,stats,policy,raw)
        self.assertRaises(ValueError,assert_policy,{},policy,raw)
    def test_immutable_receipt(self):
        path=self.home/'receipt.json'; write_once(path,{'score':0})
        self.assertRaises(FileExistsError,write_once,path,{'score':1})
    def test_owner_channel_requires_busy_then_idle_not_text(self):
        (self.home/'run').mkdir(); server=socket.socket(socket.AF_UNIX)
        server.bind(str(self.home/'run/owner.sock')); server.listen(1)
        self.addCleanup(server.close)
        received=[]
        def peer():
            with server.accept()[0] as conn:
                stream=conn.makefile('rb'); received.append(json.loads(stream.readline()))
                conn.sendall(b'{"t":"state","busy":false}\n')
                received.append(json.loads(stream.readline()))
                conn.sendall(b'{"t":"notice","text":"done"}\n{"t":"state","busy":true}\n{"t":"state","busy":false}\n')
        thread=threading.Thread(target=peer); thread.start()
        result=run_prompt(self.home,'fixture prompt',5); thread.join(timeout=2)
        self.assertEqual(result['status'],'completed'); self.assertEqual(received[1]['text'],'fixture prompt')
        self.assertEqual(len(result['frames']),3)
    def test_snapshot_internal_links_preserved_external_links_denied(self):
        root=self.home/'tree'; root.mkdir(); (root/'real').write_text('body')
        (root/'link').symlink_to('real'); first=tree_hash(root)
        (root/'link').unlink(); (root/'link').symlink_to('../outside')
        self.assertRaises(ValueError,tree_hash,root)
        self.assertEqual(len(first),64)

    def usage(self,row):
        directory=self.home/'extend/usage';directory.mkdir(parents=True,exist_ok=True)
        path=directory/'2026-10-23.jsonl'
        with path.open('ab') as stream:stream.write(json.dumps(row).encode()+b'\n')
        return path

    def owner_usage(self,stamp,**extra):
        value=dict(v=1,kind='owner',start=datetime.fromtimestamp(stamp,timezone.utc).isoformat(),caps=[dict(id='skill/reuse',evidence='root',uses=1)])
        value.update(extra);return value

    def test_usage_wait_requires_actual_new_current_owner_row_not_idle_or_stale_usage(self):
        now=time.time();self.usage(self.owner_usage(now-100))
        baseline=usage_snapshot(self.home)
        self.usage(self.owner_usage(now-50));self.usage(self.owner_usage(now,kind='background'))
        missing=wait_for_usage(self.home,baseline,now,now+1,timeout=0)
        self.assertEqual(missing['status'],'missing');self.assertEqual(missing['owner_row_indices'],[])
        self.usage(self.owner_usage(now))
        observed=wait_for_usage(self.home,baseline,now,now+1,timeout=0)
        self.assertEqual(observed['status'],'observed');self.assertEqual(observed['matched_owner_rows'],1)
        self.assertEqual(observed['owner_row_indices'],[2]);self.assertEqual(observed['rows_sha256'],sha(observed['rows']))
        self.assertEqual(observed['segments'][0]['start_bytes'],len(next(iter(baseline.values()))))

    def test_usage_wait_observes_real_delayed_append_and_ignores_partial_tail(self):
        now=time.time();path=self.usage(self.owner_usage(now-100));baseline=usage_snapshot(self.home)
        complete=json.dumps(self.owner_usage(now)).encode()
        with path.open('ab') as stream:stream.write(complete)
        self.assertEqual(usage_segment(self.home,baseline,now,now+1)['rows'],[])
        def append():
            time.sleep(.01)
            with path.open('ab') as stream:stream.write(b'\n')
        thread=threading.Thread(target=append);thread.start()
        observed=wait_for_usage(self.home,baseline,now,now+1,timeout=1);thread.join(timeout=1)
        self.assertEqual(observed['status'],'observed');self.assertLess(observed['wait_s'],1)

    def test_usage_prefix_rewrite_and_symlink_are_not_valid_evidence(self):
        now=time.time();path=self.usage(self.owner_usage(now-100));baseline=usage_snapshot(self.home)
        path.write_text('{}\n')
        result=wait_for_usage(self.home,baseline,now,now+1,timeout=0)
        self.assertEqual(result['status'],'unavailable')
        path.unlink();path.symlink_to('/etc/passwd')
        with self.assertRaises(OSError):usage_snapshot(self.home)

    def audit_fixture(self):
        rh=self.home/'rh';ih=self.home/'ih';ih.mkdir();(rh/'bin').mkdir(parents=True);(rh/'lineage').mkdir()
        binary=rh/'bin/init-root';binary.write_bytes(b'fixed-test-root')
        lineage=rh/'lineage/lineage.jsonl';lineage.write_text('{"signed":"fixture"}\n')
        return rh,ih,binary,lineage

    def test_readonly_full_audit_uses_only_explicit_test_homes_and_bounds_summary(self):
        rh,ih,binary,lineage=self.audit_fixture()
        output=b'lineage ok: 3 records, head seq 3 '+b'a'*64+b' (full, 1 ms)\n'
        with patch('runner.subprocess.run',return_value=SimpleNamespace(returncode=0,stdout=output,stderr=b'')) as run,patch.dict('os.environ',{'DEEPSEEK_API_KEY':'must-not-inherit','MODAL_TOKEN_SECRET':'must-not-inherit'}):
            proof=verify_lineage(rh,ih,hashlib.sha256(binary.read_bytes()).hexdigest())
        self.assertTrue(proof['verified']);self.assertTrue(proof['full_chain_success'])
        self.assertEqual(proof['lineage_sha256'],hashlib.sha256(lineage.read_bytes()).hexdigest())
        self.assertEqual(proof['log_sha256'],hashlib.sha256(output).hexdigest())
        self.assertEqual(run.call_args.args[0],[str(binary),'audit','verify','--full'])
        env=run.call_args.kwargs['env'];self.assertEqual(env['INIT_ROOT_HOME'],str(rh));self.assertEqual(env['INIT_HOME'],str(ih))
        self.assertEqual(set(env),{'PATH','HOME','INIT_ROOT_HOME','INIT_HOME'});self.assertEqual(run.call_args.kwargs['timeout'],60)

    def test_failed_timeout_mutated_or_wrong_root_audit_never_verifies(self):
        rh,ih,binary,lineage=self.audit_fixture();expected=hashlib.sha256(binary.read_bytes()).hexdigest()
        good=b'lineage ok: 3 records, head seq 3 '+b'a'*64+b' (full, 1 ms)\n'
        for result in (SimpleNamespace(returncode=1,stdout=good,stderr=b''),SimpleNamespace(returncode=0,stdout=b'lineage ok but not full\n',stderr=b'')):
            with patch('runner.subprocess.run',return_value=result):self.assertFalse(verify_lineage(rh,ih,expected)['verified'])
        import subprocess
        with patch('runner.subprocess.run',side_effect=subprocess.TimeoutExpired('audit',60,output=b'partial')):
            value=verify_lineage(rh,ih,expected);self.assertTrue(value['timed_out']);self.assertFalse(value['verified'])
        def changed(*a,**kw):lineage.write_text('changed');return SimpleNamespace(returncode=0,stdout=good,stderr=b'')
        with patch('runner.subprocess.run',side_effect=changed):self.assertFalse(verify_lineage(rh,ih,expected)['verified'])
        with patch('runner.subprocess.run') as run:self.assertFalse(verify_lineage(rh,ih,'b'*64)['verified']);run.assert_not_called()


    def reset_fixture(self):
        life=Lifecycle(self.home/'managed');life.initialize()
        ih=life.init_home;rh=life.root_home
        for relative in ('lineage','bin','state/kernel/evo','state/sessions','work/tasks/learn','cache','vault'):(ih/relative).mkdir(parents=True,exist_ok=True)
        (ih/'state/session').write_text('learn-session')
        (ih/'state/sessions/learn-session.jsonl').write_text('old conversation')
        (ih/'state/kernel/evo/frictions.jsonl').write_text('retained operational diagnostic')
        (ih/'work/tasks/learn/answer.json').write_text('ordinary task answer')
        (ih/'cache/native').write_text('kernel cache');(ih/'vault/lineage.jsonl').write_text('kernel outbox')
        (ih/'bin/init-body').write_bytes(b'admitted-binary')
        seed=self.home/'seed';seed.mkdir();(seed/'SELF.md').write_text('admitted skill')
        clean={'PATH':'/usr/bin:/bin','HOME':str(self.home),'GIT_CONFIG_NOSYSTEM':'1','GIT_CONFIG_GLOBAL':'/dev/null'}
        def git(args,cwd=None):
            value=subprocess.run(['/usr/bin/git']+args,cwd=cwd,env=clean,stdout=subprocess.PIPE,stderr=subprocess.PIPE,check=True)
            return value.stdout.decode().strip()
        git(['init','-b','main'],seed);git(['config','user.email','fixture@example.invalid'],seed);git(['config','user.name','fixture'],seed)
        git(['add','SELF.md'],seed);git(['commit','-m','admitted'],seed);commit=git(['rev-parse','HEAD'],seed)
        git(['clone','--bare',str(seed),str(ih/'lineage/body.git')]);git(['clone',str(ih/'lineage/body.git'),str(ih/'body')])
        (ih/'body/SELF.md').write_text('unadmitted trial')
        (ih/'body/untracked-answer').write_text('learn memory')
        (ih/'body/ignored-answer').write_text('ignored memory')
        (ih/'body/.git/info/exclude').write_text('ignored-answer\n')
        marker=self.home/'must-not-execute'
        # If the driver executes status against the original checkout, Git would
        # execute this owner-level hook outside the agent sandbox.
        git(['config','core.fsmonitor',str(marker)],ih/'body')
        value=dict(v=1,body=dict(sha=commit,binary=hashlib.sha256(b'admitted-binary').hexdigest()),ext=dict(set=None,components=[]),kernel={})
        tuple_id='t'+sha(value)[:12]
        (rh/'lineage').mkdir();(rh/'store/tuples').mkdir(parents=True)
        (rh/'store/tuples'/(tuple_id+'.json')).write_text(json.dumps(value))
        # A kernel-authored lookalike must never move the Root current pointer.
        rows=[dict(by='root',kind='tuple',data=dict(id=tuple_id)),dict(by='kernel:123456789abc',kind='tuple',data=dict(id='teeeeeeeeeeee'))]
        lineage=rh/'lineage/lineage.jsonl';lineage.write_text(''.join(json.dumps(row)+'\n' for row in rows))
        proof=dict(verified=True,lineage_sha256=hashlib.sha256(lineage.read_bytes()).hexdigest())
        life.stopped(True)
        return life,proof,commit,marker

    def test_stopped_reset_preserves_only_admitted_source_and_discloses_retained_memory(self):
        life,proof,commit,marker=self.reset_fixture();ih=life.init_home
        with patch.dict(os.environ,{'DEEPSEEK_API_KEY':'must-not-inherit','MODAL_TOKEN_SECRET':'must-not-inherit'}):
            witness=reset_runtime(life,proof)
        self.assertEqual(witness['admitted_body_sha'],commit)
        self.assertEqual((ih/'body/SELF.md').read_text(),'admitted skill')
        self.assertFalse((ih/'body/untracked-answer').exists());self.assertFalse((ih/'body/ignored-answer').exists());self.assertFalse(marker.exists())
        self.assertTrue(witness['discarded_unadmitted_body_changes'])
        self.assertTrue(witness['fresh_conversation']);self.assertTrue(witness['ordinary_task_state_cleared'])
        self.assertFalse((ih/'state/session').exists());self.assertEqual(list((ih/'state/sessions').iterdir()),[])
        self.assertEqual({p.name for p in (ih/'work').iterdir()},{'body','kernel'})
        self.assertEqual((ih/'state/kernel/evo/frictions.jsonl').read_text(),'retained operational diagnostic')
        self.assertEqual((ih/'vault/lineage.jsonl').read_text(),'kernel outbox');self.assertTrue((ih/'cache/native').is_file())
        self.assertTrue(witness['residual_memory_confound']);self.assertFalse(witness['information_free_reset'])
        self.assertEqual({row['path'] for row in witness['retained_runtime_state']},{'state/kernel','cache','vault'})
        second=reset_runtime(life,proof)
        self.assertFalse(second['discarded_unadmitted_body_changes']);self.assertEqual(witness['body_after'],second['body_after'])
        self.assertEqual(witness['after'],second['after']);self.assertEqual(witness['retained_runtime_state'],second['retained_runtime_state'])

    def test_reset_requires_verified_current_tuple_and_stopped_exact_native_binary(self):
        life,proof,commit,marker=self.reset_fixture()
        with self.assertRaises(ValueError):reset_runtime(life,dict(proof,verified=False))
        (life.init_home/'bin/init-body').write_bytes(b'unadmitted binary')
        with self.assertRaises(ValueError):reset_runtime(life,proof)
        self.assertTrue((life.init_home/'work/tasks/learn/answer.json').exists())
        (life.init_home/'bin/init-body').write_bytes(b'admitted-binary')
        state=life.state();state['stopped']=False;life.save(state)
        with self.assertRaises(ValueError):reset_runtime(life,proof)
        life.stopped(True)
        (life.root_home/'lineage/lineage.jsonl').write_text('changed')
        with self.assertRaises(ValueError):reset_runtime(life,proof)

    def test_reset_refuses_external_links_or_unadmitted_extensions_before_deleting_any_state(self):
        life,proof,commit,marker=self.reset_fixture();ih=life.init_home
        (ih/'work/escape').symlink_to(self.home/'outside')
        with self.assertRaises(ValueError):reset_runtime(life,proof)
        self.assertTrue((ih/'work/tasks/learn/answer.json').exists())
        (ih/'work/escape').unlink();(ih/'bin/ext').mkdir()
        with self.assertRaises(ValueError):reset_runtime(life,proof)
        self.assertEqual((ih/'body/SELF.md').read_text(),'unadmitted trial')

    def test_completed_ack_precedes_both_lifecycle_pruning_operations(self):
        ledger=self.ledger();task=plan()['tasks'][0];claim(ledger,task);calls=[]
        life=SimpleNamespace(prune_previous=lambda *a:calls.append('prune'),evict_completed_snapshots=lambda *a:calls.append('evict'))
        def ack(config,db,identifier,mutation):
            calls.append('checkpoint');mutation();calls.append('durable_ack')
        with patch('runner.owner_checkpoint',side_effect=ack):complete_execution({},ledger,life,self.home,task['id'],{'id':task['id']})
        self.assertEqual(calls,['checkpoint','durable_ack','prune','evict'])
        calls.clear()
        def lost(*args):raise TimeoutError('lost ACK')
        with patch('runner.owner_checkpoint',side_effect=lost):
            with self.assertRaises(TimeoutError):complete_execution({},ledger,life,self.home,task['id'],{'id':task['id']})
        self.assertEqual(calls,[])

if __name__=='__main__': unittest.main()
