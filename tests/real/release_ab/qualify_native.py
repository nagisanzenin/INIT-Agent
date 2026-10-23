#!/usr/bin/env python3
"""Native, $0 qualification execution. Never creates a blanket passed/ready receipt.
Runs an actual compiled Linux isolation test, then a real Body against a synthetic
upstream through the same external Gateway validation/accounting and Root route.
"""
import argparse
import hashlib
import http.client
import json
import os
from pathlib import Path
import pwd
import re
import secrets
import shlex
import socket
import subprocess
import sys
from bootstrap import clean_env, launch_observation
from lifecycle import BASE, Lifecycle
from runner import assert_policy, digest, fingerprints, rpc, run_prompt, wait_for, write_once

FROZEN=('body','bin','kernel','work/body','work/kernel')
FREEZE_REF='evo/ab-freeze-proof'


def root_egress_probe(root_home, model, max_tokens, policy_raw):
    """Actual owner-host HTTP over Root's socket, bypassing kernel route filters."""
    socket_path=Path(root_home)/'run/egress.sock'
    sent=[]
    class UnixHTTP(http.client.HTTPConnection):
        def connect(self):
            self.sock=socket.socket(socket.AF_UNIX,socket.SOCK_STREAM); self.sock.settimeout(self.timeout)
            self.sock.connect(str(socket_path))
        def send(self,data):
            if not isinstance(data,bytes): raise ValueError('unexpected owner probe wire type')
            sent.append(data)
            if sum(map(len,sent))>8192: raise ValueError('owner probe request exceeds bound')
            super().send(data)
    body=json.dumps(dict(model=model,messages=[dict(role='user',content='synthetic Root negative qualification')],max_tokens=max_tokens,stream=False),sort_keys=True).encode()
    connection=UnixHTTP('localhost',timeout=5)
    try:
        connection.request('POST','/deepseek/chat/completions',body=body,
            headers={'Content-Type':'application/json','Connection':'close'})
        response=connection.getresponse(); payload=response.read(65537)
        if len(payload)>65536: raise ValueError('owner probe response exceeds bound')
        result=dict(v=1,source='owner-host-root-egress',policy_sha256=hashlib.sha256(policy_raw).hexdigest(),
            root_binary_sha256=digest(Path(root_home)/'bin/init-root'),
            request_wire_utf8=b''.join(sent).decode(),response_status=response.status,
            response_headers=dict((k.lower(),v) for k,v in response.getheaders()),response_body_utf8=payload.decode())
        result['request_wire_sha256']=hashlib.sha256(b''.join(sent)).hexdigest()
        result['response_body_sha256']=hashlib.sha256(payload).hexdigest()
        return result
    finally: connection.close()


def valid_root_denial(proof, root_binary_sha256, policy_sha256, model, max_tokens):
    """A proxy refusal or assistant claim can never stand in for Root evidence."""
    try:
        wire=proof['request_wire_utf8'].encode(); response=proof['response_body_utf8'].encode()
        head,body=wire.split(b'\r\n\r\n',1); value=json.loads(body)
        reply=json.loads(response)
        return (proof.get('source')=='owner-host-root-egress' and proof.get('root_binary_sha256')==root_binary_sha256
            and proof.get('policy_sha256')==policy_sha256
            and hashlib.sha256(wire).hexdigest()==proof['request_wire_sha256']
            and hashlib.sha256(response).hexdigest()==proof['response_body_sha256']
            and head.split(b'\r\n',1)[0]==b'POST /deepseek/chat/completions HTTP/1.1'
            and value['model']==model and type(value['max_tokens']) is int and value['max_tokens']==max_tokens
            and type(proof['response_status']) is int and proof['response_status']==503
            and proof['response_headers'].get('x-init-egress-error')=='root.benchmark'
            and reply['error']['code']=='root.benchmark' and reply['init']['source']=='root' and reply['init']['code']=='root.benchmark')
    except (KeyError,ValueError,TypeError,AttributeError): return False


def prepare_freeze_candidate(life, output):
    """Owner-only setup while stopped; the agent must still use normal admission.

    Keep main and its files unchanged. Only the prepared ref remains in .git and is
    included in the frozen baseline. T0's free admission has no build/G1 stage.
    """
    if life.state().get('stopped') is not True: raise ValueError('prepare only while stopped')
    body=life.init_home/'body'; git=['/usr/bin/git','-C',str(body),'-c','core.hooksPath=/dev/null','-c','commit.gpgsign=false']
    env=clean_env(pwd.getpwuid(os.getuid()).pw_dir)
    def run(*args):
        return subprocess.run(git+list(args),env=env,check=True,capture_output=True,timeout=30).stdout.decode().strip()
    if run('status','--porcelain','--untracked-files=all'): raise ValueError('frozen candidate requires clean genesis source')
    previous=run('symbolic-ref','--short','HEAD')
    initial=run('rev-parse','HEAD')
    if subprocess.run(git+['show-ref','--verify','--quiet','refs/heads/'+FREEZE_REF],env=env).returncode!=1:
        raise ValueError('freeze candidate ref already exists or cannot be checked')
    run('switch','-c',FREEZE_REF)
    skill=body/'skills/ab-freeze-proof/SKILL.md'
    skill.parent.mkdir(); skill.write_text('---\nname: ab-freeze-proof\ndescription: Synthetic admission denial qualification.\n---\nRead the owner requested qualification marker.\n')
    run('add','--','skills/ab-freeze-proof/SKILL.md')
    run('-c','user.name=init-native-qualification','-c','user.email=qualification@init.invalid','commit','-m',
        'skill: add reversible frozen admission probe\n\nWhy: Exercise real Root refusal through normal T0 admission.\nPrefix-change: yes\n')
    candidate=run('rev-parse','HEAD'); run('switch',previous)
    if run('rev-parse','HEAD')!=initial or skill.exists() or run('status','--porcelain','--untracked-files=all'):
        raise ValueError('candidate preparation failed to restore genesis working tree')
    write_once(output/'prepared-candidate.json',dict(ref=FREEZE_REF,parent_sha=initial,candidate_sha=candidate,admitted=False))


def probe_command(home, paths):
    commands=[]
    for index,path in enumerate(paths):
        target=home/path
        if not target.is_dir() or target.is_symlink(): raise ValueError('freeze probe requires real existing directory '+path)
        marker=target/'.ab-denial-probe'
        if marker.exists(): raise ValueError('freeze probe marker already exists')
        commands.append(f"if (printf probe > {shlex.quote(str(marker))}) 2>/dev/null; then printf 'AB_WRITE_ALLOWED_{index}\\n'; else printf 'AB_WRITE_DENIED_{index}\\n'; fi")
    # Must be observed as root.benchmark_frozen, not inferred from any nonzero exit.
    commands.append('cd '+shlex.quote(str(home/'body'))+'\n'+shlex.quote(str(home/'kernel/bin/initctl'))+' --json --timeout 60 promote '+FREEZE_REF+' --wait 2>&1')
    bypass=json.dumps(dict(model='benchmark-forbidden-model',messages=[dict(role='user',content='synthetic route denial')],max_tokens=1,stream=False))
    commands.append('printf \'AB_ROOT_EGRESS_PROBE\\n\'; /usr/bin/curl --max-time 10 -sS --unix-socket '+shlex.quote(str(home/'run/proxy.sock'))+' -H \'Content-Type: application/json\' --data '+shlex.quote(bypass)+' http://x/deepseek/chat/completions 2>&1')
    oversized=json.dumps(dict(model='deepseek-flash',messages=[dict(role='user',content='synthetic output limit denial')],max_tokens=4097,stream=False))
    commands.append('printf \'AB_ROOT_OUTPUT_PROBE\\n\'; /usr/bin/curl --max-time 10 -sS --unix-socket '+shlex.quote(str(home/'run/proxy.sock'))+' -H \'Content-Type: application/json\' --data '+shlex.quote(oversized)+' http://x/deepseek/chat/completions 2>&1')
    return '\n'.join(commands)


def native_probe(test_binary, root_binary, output):
    env=clean_env(pwd.getpwuid(os.getuid()).pw_dir); env['INIT_TEST_ROOT_BIN']=str(root_binary)
    command=[str(test_binary),'linux_zone_real_isolation','--ignored','--nocapture']
    result=subprocess.run(command,env=env,capture_output=True,timeout=180)
    log=output/'native-probe.log'; log.write_bytes(result.stdout+result.stderr)
    # Zero matching tests is not success. These assertions live inside native test.
    passed=result.returncode==0 and re.search(rb'test result: ok\. 1 passed; 0 failed;',result.stdout) is not None
    receipt=dict(scope='native zone primitive isolation only',passed=passed,returncode=result.returncode,
                 root_binary_sha256=digest(root_binary),test_binary_sha256=digest(test_binary),log_sha256=digest(log),paid_calls=0)
    write_once(output/'native-probe.json',receipt)
    return passed


def frozen_session(life, output, port, phase):
    life.restore('genesis')
    prepare_freeze_candidate(life,output)
    keypath=life.root_home/'keys/providers/deepseek'; keypath.parent.mkdir(parents=True,exist_ok=True)
    token=secrets.token_hex(32); keypath.write_text(token); keypath.chmod(0o600)
    policy=dict(v=1,run_id='qualify-'+phase,phase=phase,gateway_host=f'127.0.0.1:{port}',provider='deepseek',model='deepseek-flash',request_max_tokens=4096)
    raw=json.dumps(policy,sort_keys=True).encode(); path=life.root_home/'benchmark.json'; path.write_bytes(raw); path.chmod(0o600)
    baseline=fingerprints(life.init_home,FROZEN)
    command=probe_command(life.init_home,FROZEN)
    scenario=[dict(label='normal',delta=dict(role='assistant',content='Synthetic truncated response'),finish_reason='length'),
              dict(label='intended-length-retry',delta=dict(role='assistant',content='',tool_calls=[dict(index=0,id='qualification-denials',type='function',function=dict(name='bash',arguments=json.dumps(dict(command=command,timeout=30))))]),finish_reason='tool_calls'),
              dict(label='tool-results',delta=dict(role='assistant',content='Synthetic qualification done.'),finish_reason='stop')]
    write_once(output/'scenario.json',scenario)
    env=clean_env(pwd.getpwuid(os.getuid()).pw_dir)
    env.update(INIT_ROOT_HOME=str(life.root_home),INIT_HOME=str(life.init_home))
    state=life.state(); state['stopped']=False; life.save(state)
    root_launch=launch_observation(env,keypath,token,__file__)
    mock=None; root=None; trace=None; stopped=False
    try:
        mock=subprocess.Popen([sys.executable,str(Path(__file__).with_name('mock_gateway.py')),'--scenario',str(output/'scenario.json'),'--output',str(output/'gateway'),'--port',str(port)],env=dict(env,INIT_AB_GATEWAY_TOKEN=token),stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
        def listening():
            if mock.poll() is not None: raise RuntimeError('synthetic gateway failed')
            with socket.create_connection(('127.0.0.1',port),timeout=.2): return True
        wait_for(listening,10)
        with (output/'root.log').open('xb') as log:
            root=subprocess.Popen([str(life.root_home/'bin/init-root'),'start','--foreground'],env=env,stdout=log,stderr=log)
        wait_for(lambda: (rpc(life.root_home,'status').get('kernel') or {}).get('ready') is True,120)
        stats=rpc(life.root_home,'egress.stats'); assert_policy(stats,policy,raw)
        denials=dict(wrong_model=root_egress_probe(life.root_home,'benchmark-forbidden-model',1,raw),
                     oversized_output=root_egress_probe(life.root_home,'deepseek-flash',4097,raw))
        write_once(output/'root-egress-denials.json',denials)
        trace=run_prompt(life.root_home,'Run the isolated benchmark qualification requested by the owner.',120)
        write_once(output/'owner-trace.json',trace); write_once(output/'ceilings.json',rpc(life.root_home,'egress.stats'))
    finally:
        try:
            if root is not None:
                rpc(life.root_home,'stop')
                def idle():
                    status=rpc(life.root_home,'status')
                    return status.get('slot')=='stopped' and not status.get('instances') and status.get('kernel') is None
                wait_for(idle,30); stopped=True
        finally:
            for process in (mock,root):
                if process is not None and process.poll() is None:
                    process.terminate()
                    try: process.wait(timeout=10)
                    except subprocess.TimeoutExpired: process.kill(); process.wait(timeout=5)
            keypath.unlink(missing_ok=True)
        if stopped: life.stopped(True)
    if not stopped: raise ValueError('native runtime did not stop cleanly')
    captures=sorted((output/'gateway').glob('request-[0-9][0-9][0-9].json'))
    # Verify tool-result channel, never model assistant claims of denial.
    results=[]
    for capture in captures:
        for message in json.loads(capture.read_text()).get('messages',[]):
            if message.get('role')=='tool' and message.get('tool_call_id')=='qualification-denials':
                results.append(message.get('content',''))
    text='\n'.join(results)
    denied=all(f'AB_WRITE_DENIED_{i}' in text for i in range(len(FROZEN))) and 'AB_WRITE_ALLOWED_' not in text
    after=fingerprints(life.init_home,FROZEN)
    result=dict(scope='synthetic upstream / real native Root + Kernel + Body',phase=phase,paid_calls=0,
                root_policy_observed=True,owner_completed=trace is not None and trace['status']=='completed',
                artifact_writes_denied=denied,artifacts_unchanged=baseline==after,
                root_change_open_denied='root.benchmark_frozen' in text,
                direct_root_model_bypass_denied=valid_root_denial(denials['wrong_model'],digest(life.root_home/'bin/init-root'),hashlib.sha256(raw).hexdigest(),'benchmark-forbidden-model',1),
                root_egress_denials=dict(path=str(output/'root-egress-denials.json'),sha256=digest(output/'root-egress-denials.json')),
                root_launch_observation=root_launch,
                before_artifacts=baseline,after_artifacts=after,
                captures=[dict(path=str(p),sha256=digest(p)) for p in captures],
                missing_qualifications=['runtime compaction request','runtime probe request','learned capability restart reuse','rollback','direct provider HTTP bypass denial through Root'],
                ready_for_paid_ab=False)
    write_once(output/'session-result.json',result)
    return result


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--test-binary',required=True); p.add_argument('--root-binary',required=True)
    p.add_argument('--output',required=True); p.add_argument('--gateway-port',type=int,default=18763)
    p.add_argument('--primitive-only',action='store_true')
    args=p.parse_args(); out=Path(args.output).resolve()
    if BASE not in out.parents: p.error('output must be under repository target/ab-homes')
    out.mkdir(parents=True)
    if not native_probe(Path(args.test_binary).resolve(),Path(args.root_binary).resolve(),out):
        print(json.dumps(dict(ready=False,blockers=['native isolation test failed; inspect log'],paid_calls=0))); raise SystemExit(2)
    if args.primitive_only: return
    life=Lifecycle()
    for phase in ('genesis','transfer'):
        folder=out/phase; folder.mkdir(); frozen_session(life,folder,args.gateway_port,phase)
    print(json.dumps(dict(ready=False,paid_calls=0,scope='partial actual qualification; inspect session-result.json remaining gates')))

if __name__=='__main__': main()
