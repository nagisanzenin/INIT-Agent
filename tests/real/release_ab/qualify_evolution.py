#!/usr/bin/env python3
"""Synthetic-upstream, real native L0 body evolution/restart/rollback qualification.
No reward or level bypass, no paid transport, no synthesized Root evidence.
"""
import argparse
import json
import os
from pathlib import Path
import pwd
import secrets
import shlex
import shutil
import socket
import subprocess
import sys
import time
from bootstrap import clean_env, launch_observation
from lifecycle import BASE, Lifecycle
from runner import assert_policy, digest, fingerprints, rpc, run_prompt, wait_for, write_once, verify_lineage, reset_runtime
from qualify_native import FROZEN

SKILL='ab-proof'
CAP='skill/'+SKILL
SKILL_TEXT='''---
name: ab-proof
description: Read the reversible native qualification marker when the owner requests it.
---
The native qualification marker is INIT_AB_NATIVE_REUSE_20261023.
'''


def tool_step(name,args,identifier):
    return dict(label=identifier,delta=dict(role='assistant',content='',tool_calls=[dict(index=0,id=identifier,type='function',function=dict(name=name,arguments=json.dumps(args)))]),finish_reason='tool_calls')


def learn_command(home):
    body=home/'body'; ctl=home/'kernel/bin/initctl'
    message='feat(skill): add reversible qualification skill\n\nWhy: Exercise real reversible body admission and restart reuse without paid model calls.\nPrefix-change: yes\n'
    return '\n'.join(['set -eu','cd '+shlex.quote(str(body)),
        'git switch -c evo/ab-proof','mkdir -p skills/ab-proof',
        'printf %s '+shlex.quote(SKILL_TEXT)+' > skills/ab-proof/SKILL.md',
        # Free T0 admission has no G1/build stage; Root derives skills from the
        # exact committed T0 tar. Genesis has no built extension set to map.
        'git add -- skills/ab-proof/SKILL.md',
        'git -c user.name=init-native-qualification -c user.email=qualification@init.invalid -c commit.gpgsign=false commit -m '+shlex.quote(message),
        shlex.quote(str(ctl))+' --json --timeout 600 promote evo/ab-proof --wait'])


def rows(path):
    if not path.is_file(): return []
    return [json.loads(line) for line in path.read_text().splitlines() if line]


def usage_rows(home):
    return [row for p in sorted((home/'extend/usage').glob('*.jsonl')) for row in rows(p)]


def owner_line(home,text):
    with socket.socket(socket.AF_UNIX) as sock:
        sock.settimeout(5); sock.connect(str(home/'run/owner.sock'))
        sock.sendall(b'{"client":"release_ab_owner","replay":false,"t":"attach"}\n')
        sock.sendall((json.dumps({'t':'line','text':text})+'\n').encode())
        # The command's effect is checked through independent Root status/lineage.
        time.sleep(.2)
        sock.sendall(b'{"t":"detach"}\n')


def session(life,out,port,phase,scenario,prompt=None,owner_action=None,wait_for_cap=None):
    out.mkdir(); write_once(out/'scenario.json',scenario)
    token=secrets.token_hex(32); key=life.root_home/'keys/providers/deepseek'
    key.parent.mkdir(parents=True,exist_ok=True); key.write_text(token); key.chmod(0o600)
    policy=dict(v=1,run_id='body-proof-'+out.name,phase=phase,gateway_host=f'127.0.0.1:{port}',provider='deepseek',model='deepseek-flash',request_max_tokens=4096)
    raw=json.dumps(policy,sort_keys=True).encode(); sidecar=life.root_home/'benchmark.json'; sidecar.write_bytes(raw); sidecar.chmod(0o600)
    env=clean_env(pwd.getpwuid(os.getuid()).pw_dir); env.update(INIT_ROOT_HOME=str(life.root_home),INIT_HOME=str(life.init_home))
    root_launch=launch_observation(env,key,token,__file__)
    start_lineage=len(rows(life.root_home/'lineage/lineage.jsonl')); start_usage=len(usage_rows(life.root_home))
    before=fingerprints(life.init_home,FROZEN); state=life.state(); state['stopped']=False; life.save(state)
    mock=root=None; stopped=False; trace=None; action_result=None
    try:
        mock=subprocess.Popen([sys.executable,str(Path(__file__).with_name('mock_gateway.py')),'--scenario',str(out/'scenario.json'),'--output',str(out/'gateway'),'--port',str(port)],env=dict(env,INIT_AB_GATEWAY_TOKEN=token),stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
        def gateway_ready():
            if mock.poll() is not None: raise RuntimeError('mock gateway exited')
            with socket.create_connection(('127.0.0.1',port),timeout=.2): return True
        wait_for(gateway_ready,10)
        with (out/'root.log').open('xb') as log:
            root=subprocess.Popen([str(life.root_home/'bin/init-root'),'start','--foreground'],env=env,stdout=log,stderr=log)
        wait_for(lambda:(rpc(life.root_home,'status').get('kernel') or {}).get('ready'),120)
        assert_policy(rpc(life.root_home,'egress.stats'),policy,raw)
        if prompt: trace=run_prompt(life.root_home,prompt,900)
        if wait_for_cap:
            # Root's turn closes after 30 seconds of quiet. Owner busy->idle
            # does not mean the Root-attributed usage row has been persisted.
            wait_for(lambda:any(c.get('id')==wait_for_cap and c.get('evidence')=='root' and c.get('uses',0)>0
                for row in usage_rows(life.root_home)[start_usage:] for c in row.get('caps',[])),45)
        if owner_action: action_result=owner_action()
        write_once(out/'status.json',rpc(life.root_home,'status'))
    finally:
        try:
            if root is not None:
                rpc(life.root_home,'stop')
                def idle():
                    st=rpc(life.root_home,'status')
                    return st.get('slot')=='stopped' and not st.get('instances') and st.get('kernel') is None
                wait_for(idle,30); stopped=True
        finally:
            for process in (mock,root):
                if process is not None and process.poll() is None:
                    process.terminate()
                    try: process.wait(timeout=10)
                    except subprocess.TimeoutExpired: process.kill(); process.wait(timeout=5)
            key.unlink(missing_ok=True)
        if stopped: life.stopped(True)
    if not stopped: raise ValueError('runtime shutdown unqualified')
    lineage=rows(life.root_home/'lineage/lineage.jsonl')[start_lineage:]
    usage=usage_rows(life.root_home)[start_usage:]
    write_once(out/'lineage-segment.json',lineage); write_once(out/'usage-segment.json',usage)
    write_once(out/'owner-trace.json',trace)
    verification=subprocess.run([str(life.root_home/'bin/init-root'),'audit','verify','--full'],env=env,capture_output=True,timeout=60)
    (out/'lineage-verify.log').write_bytes(verification.stdout+verification.stderr)
    after=fingerprints(life.init_home,FROZEN)
    result=dict(paid_calls=0,transport='synthetic',phase=phase,lineage_verified=verification.returncode==0,
                root_launch_observation=root_launch,
                lineage_sha256=digest(out/'lineage-segment.json'),usage_sha256=digest(out/'usage-segment.json'),
                before_artifacts=before,after_artifacts=after,owner_completed=trace is not None and trace['status']=='completed',owner_action=action_result)
    write_once(out/'session.json',result)
    return result,lineage,usage


def qualify(life,out,port):
    life.restore('genesis')
    root_sha256=digest(life.root_home/'bin/init-root')
    genesis_verification=verify_lineage(life.root_home,life.init_home,root_sha256)
    genesis_reset=reset_runtime(life,genesis_verification)
    skill=life.init_home/'body/skills'/SKILL/'SKILL.md'
    if skill.exists(): raise ValueError('genesis already contains qualification skill')
    steps=[tool_step('bash',dict(command=learn_command(life.init_home),timeout=660),'admit-skill'),dict(label='learning-finish')]
    learned,lineage,_=session(life,out/'learn',port,'learn',steps,'Perform the owner requested reversible skill admission qualification.')
    opens=[r for r in lineage if r.get('kind')=='change_open' and any(c.get('id')==CAP and c.get('evidence')=='root' and c.get('op')=='add' for c in r.get('data',{}).get('caps',[]))]
    admitted=bool(opens) and learned['lineage_verified'] and skill.is_file() and skill.read_text()==SKILL_TEXT
    result=dict(paid_calls=0,transport='synthetic',scope='L0 body T0 admission; not kernel evolution or model quality',admitted=admitted,restart_reuse=False,rollback=False,ready_for_paid_ab=False,
                genesis_reset=genesis_reset,genesis_lineage_verification=genesis_verification,
                residual_memory_confound=True,information_free_reset=False)
    if not admitted:
        result['blockers']=['No verified Root change_open for exact retained skill; stop, do not fabricate admission.']; write_once(out/'result.json',result); return
    parent_tuple=opens[-1]['data'].get('parent_tuple'); tuple_id=opens[-1]['data'].get('tuple')
    learned_verification=verify_lineage(life.root_home,life.init_home,root_sha256)
    learned_reset=reset_runtime(life,learned_verification)
    result.update(learned_reset=learned_reset,learned_lineage_verification=learned_verification,
                  admitted_skill_retained_after_reset=skill.is_file() and skill.read_text()==SKILL_TEXT)
    if not result['admitted_skill_retained_after_reset']:
        result['blockers']=['Exact admitted skill was not retained after stopped-runtime reconstruction.']; write_once(out/'result.json',result); return
    life.snapshot('qualification-learned'); life.restore('qualification-learned')
    reuse,_,usage=session(life,out/'restart-reuse',port,'transfer',[tool_step('read',{'path':str(skill)},'reuse-skill'),dict(label='reuse-finish')],'Read the admitted qualification skill after this fresh boot.',wait_for_cap=CAP)
    result['restart_reuse']=reuse['lineage_verified'] and reuse['owner_completed'] and reuse['before_artifacts']==reuse['after_artifacts'] and any(c.get('id')==CAP and c.get('evidence')=='root' and c.get('uses',0)>0 for row in usage for c in row.get('caps',[]))
    life.restore('qualification-learned')
    def rollback():
        initial=rpc(life.root_home,'status')
        if initial.get('tuples',{}).get('current')!=tuple_id: raise ValueError('rollback source tuple changed')
        owner_line(life.root_home,'/rollback '+CAP)
        def restored():
            current=rpc(life.root_home,'status'); kernel=current.get('kernel') or {}
            return kernel.get('ready') is True and current.get('tuples',{}).get('current')==parent_tuple and not skill.exists()
        wait_for(restored,120)
        return {'owner_command':'/rollback '+CAP,'skill_absent':not skill.exists(),'parent_tuple_restored':parent_tuple}
    reverted,events,_=session(life,out/'rollback',port,'learn',[],owner_action=rollback)
    confirmed=[r for r in events if (r.get('kind')=='rollback' and r.get('data',{}).get('to')==parent_tuple) or (r.get('kind')=='body_rollback' and r.get('data',{}).get('to_sha')==opens[-1]['data'].get('from_sha'))]
    result['rollback']=reverted['lineage_verified'] and not skill.exists() and bool(confirmed)
    result.update(parent_tuple=parent_tuple,admitted_tuple=tuple_id,blockers=['Output compaction/probe and direct Root bypass qualification remain separate.'])
    write_once(out/'result.json',result)


def main():
    p=argparse.ArgumentParser(description=__doc__); p.add_argument('--output',required=True); p.add_argument('--gateway-port',type=int,default=18763)
    a=p.parse_args(); out=Path(a.output).resolve()
    if BASE not in out.parents: p.error('output must remain under target/ab-homes')
    out.mkdir(parents=True); qualify(Lifecycle(),out,a.gateway_port)

if __name__=='__main__': main()
