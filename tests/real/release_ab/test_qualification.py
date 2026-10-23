import json
from pathlib import Path
import tempfile
import tomllib
import unittest
from unittest.mock import patch
import subprocess
import socket
import threading
from bootstrap import charter, copy_source, vendor_seed, REPO, launch_observation, clean_env
from gateway import Gateway
from harness import Ledger, plan
from lifecycle import BASE, Lifecycle
from mock_gateway import Scripted
from qualify_native import native_probe, probe_command, prepare_freeze_candidate, FREEZE_REF, root_egress_probe, valid_root_denial
from runner import digest
import hashlib
from qualify_evolution import learn_command, tool_step

class QualificationTests(unittest.TestCase):
    def setUp(self):
        BASE.mkdir(parents=True,exist_ok=True)
        self.tmp=tempfile.TemporaryDirectory(dir=BASE); self.addCleanup(self.tmp.cleanup)
        self.root=Path(self.tmp.name)
    def test_charter_disables_home_full_and_external_destination(self):
        value=tomllib.loads(charter(REPO,18763))
        self.assertFalse(value['sandbox']['read_home']); self.assertFalse(value['sandbox']['allow_full'])
        self.assertEqual(value['actions']['providers']['destinations'],['127.0.0.1:18763'])
        self.assertEqual(value['notify']['cmd'],[])
        self.assertEqual(value['extend']['mode'],'free')
        self.assertFalse(value.get('root',{}).get('test_mode',False))
        provider=value['actions']['providers']['providers']['deepseek']
        self.assertTrue(provider['local']); self.assertEqual(provider['api'],'chat')
    def test_source_packaging_rejects_symlink_instead_of_following(self):
        source=self.root/'src'; source.mkdir(); (source/'escape').symlink_to('/etc/passwd')
        self.assertRaises(ValueError,copy_source,source,self.root/'dst')
    def test_linux_crate_is_full_not_stubbed(self):
        repo=self.root/'repo'; vendor=repo/'vendor/linux-raw-sys'; vendor.mkdir(parents=True)
        (vendor/'Cargo.toml').write_text('[package]\nname="linux-raw-sys"\nversion="1.0.0"\n')
        (vendor/'real.rs').write_text('actual native dependency')
        (vendor/'.cargo-checksum.json').write_text(json.dumps(dict(files={name:digest(vendor/name) for name in ('Cargo.toml','real.rs')},package='synthetic-fixture')))
        seed=self.root/'seed'; seed.mkdir()
        (seed/'Cargo.lock').write_text('[[package]]\nname="linux-raw-sys"\nversion="1.0.0"\nsource="registry+https://example.invalid"\n')
        self.assertEqual(vendor_seed(repo,seed),1)
        self.assertEqual((seed/'vendor/linux-raw-sys/real.rs').read_text(),'actual native dependency')
    def test_zero_native_tests_never_qualifies(self):
        binary=self.root/'binary'; binary.write_bytes(b'synthetic unit fixture')
        result=subprocess.CompletedProcess([],0,b'test result: ok. 0 passed; 0 failed;',b'')
        with patch('qualify_native.subprocess.run',return_value=result):
            self.assertFalse(native_probe(binary,binary,self.root))
    def test_mock_upstream_uses_production_guard_and_records_real_request(self):
        ledger=Ledger(self.root/'mock.sqlite'); ledger.initialize(plan(),10000000); self.addCleanup(ledger.db.close)
        transport=Scripted([{'label':'intended-normal'}],self.root)
        gateway=Gateway(ledger,plan()['tasks'][0]['id'],'synthetic',transport)
        request=json.dumps(dict(model='deepseek-flash',messages=[{'role':'user','content':'qualification'}],max_tokens=4096,stream=True)).encode()
        frames=[]; receipt=gateway.forward(request,lambda c:None,frames.append)
        self.assertTrue(receipt['usage_known']); self.assertIn(b'[DONE]',b''.join(frames))
        capture=json.loads((self.root/'request-000.json').read_text())
        self.assertEqual(capture['max_tokens'],4096)
        self.assertEqual(json.loads((self.root/'request-000.receipt.json').read_text())['transport'],'synthetic')
    def test_body_evolution_script_uses_normal_admission_not_lineage_or_level_edits(self):
        command=learn_command(self.root)
        self.assertIn('promote evo/ab-proof --wait',command)
        self.assertIn('Prefix-change: yes',command)
        self.assertNotIn('selfmap',command)
        self.assertNotIn('lineage.jsonl',command); self.assertNotIn('level',command)
        step=tool_step('bash',{'command':command,'timeout':660},'admit-skill')
        call=step['delta']['tool_calls'][0]
        self.assertEqual(json.loads(call['function']['arguments'])['command'],command)
    def test_missing_freeze_target_is_not_denial_proof(self):
        self.assertRaises(ValueError,probe_command,self.root,['missing'])
    def test_owner_wire_probe_captures_root_denial_and_rejects_proxy_error(self):
        (self.root/'run').mkdir(); (self.root/'bin').mkdir(); binary=self.root/'bin/init-root'; binary.write_bytes(b'synthetic unit binary')
        path=self.root/'run/egress.sock'; server=socket.socket(socket.AF_UNIX); server.bind(str(path)); server.listen(1)
        def reply():
            with server,server.accept()[0] as connection:
                connection.settimeout(2); received=b''
                while b'\r\n\r\n' not in received:received+=connection.recv(4096)
                head,body=received.split(b'\r\n\r\n',1)
                length=int(next(line.split(b':',1)[1] for line in head.split(b'\r\n') if line.lower().startswith(b'content-length:')))
                while len(body)<length:body+=connection.recv(4096)
                payload=json.dumps(dict(error=dict(code='root.benchmark'),init=dict(source='root',code='root.benchmark'))).encode()
                connection.sendall(b'HTTP/1.1 503 Service Unavailable\r\nContent-Type: application/json\r\nx-init-egress-error: root.benchmark\r\nContent-Length: '+str(len(payload)).encode()+b'\r\nConnection: close\r\n\r\n'+payload)
        thread=threading.Thread(target=reply); thread.start()
        policy=b'synthetic unit policy'; proof=root_egress_probe(self.root,'benchmark-forbidden-model',1,policy)
        thread.join(2); self.assertFalse(thread.is_alive())
        bound=(digest(binary),hashlib.sha256(policy).hexdigest(),'benchmark-forbidden-model',1)
        self.assertTrue(valid_root_denial(proof,*bound)); self.assertNotIn('Authorization:',proof['request_wire_utf8'])
        proof['response_body_utf8']=json.dumps(dict(error=dict(code='request.model_not_allowed'),init=dict(source='proxy',code='request.model_not_allowed')))
        proof['response_body_sha256']=hashlib.sha256(proof['response_body_utf8'].encode()).hexdigest()
        self.assertFalse(valid_root_denial(proof,*bound),'proxy denial cannot qualify Root enforcement')
    def test_root_launch_observation_records_actual_environment_and_only_ephemeral_key(self):
        key=self.root/'deepseek'; key.write_text('synthetic-marker'); key.chmod(0o600)
        env=clean_env(self.root)
        proof=launch_observation(env,key,'synthetic-marker',__file__)
        self.assertEqual(proof['environment_names'],sorted(env))
        self.assertTrue(proof['upstream_and_cloud_credentials_absent']); self.assertTrue(proof['provider_key_only_ephemeral'])
        env['DEEPSEEK_API_KEY']='not-a-real-key'
        proof=launch_observation(env,key,'different-marker',__file__)
        self.assertFalse(proof['upstream_and_cloud_credentials_absent']); self.assertFalse(proof['provider_key_only_ephemeral'])
        self.assertNotIn('not-a-real-key',json.dumps(proof)); self.assertNotIn('synthetic-marker',json.dumps(proof))
    def test_freeze_ref_is_prepared_without_changing_genesis_working_tree(self):
        life=Lifecycle(self.root/'life'); life.initialize(); life.stopped(True)
        body=life.init_home/'body'; (body/'skills').mkdir(parents=True)
        (body/'GENESIS.md').write_text('actual local git fixture\n')
        prefix=['git','-C',str(body),'-c','user.name=qualification','-c','user.email=qualification@init.invalid','-c','commit.gpgsign=false']
        for args in [('init','-b','main'),('add','-A'),('commit','-m','genesis')]:
            subprocess.run(prefix+list(args),check=True,capture_output=True)
        prepare_freeze_candidate(life,self.root)
        self.assertFalse((body/'skills/ab-freeze-proof/SKILL.md').exists())
        status=subprocess.run(prefix+['status','--porcelain'],check=True,capture_output=True)
        self.assertEqual(status.stdout,b'')
        candidate=subprocess.run(prefix+['show',FREEZE_REF+':skills/ab-freeze-proof/SKILL.md'],check=True,capture_output=True)
        self.assertIn(b'name: ab-freeze-proof',candidate.stdout)
        command=probe_command(life.init_home,['body'])
        self.assertIn('promote '+FREEZE_REF+' --wait',command)
        self.assertNotIn('gate --quick',command)
        self.assertFalse(json.loads((self.root/'prepared-candidate.json').read_text())['admitted'])

if __name__=='__main__': unittest.main()
