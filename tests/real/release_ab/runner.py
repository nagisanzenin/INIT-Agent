#!/usr/bin/env python3
"""Owner-only qualified native A/B driver. Preflight by default; --execute spends.
Install genesis at Lifecycle's stable paths first. This does not build or install.
Never place this script, fixtures, evidence, or ledger inside agent-readable mounts.
"""
import argparse
from datetime import datetime, timezone
import hashlib
import importlib.util
import json
import os
import pwd
import re
from pathlib import Path
import secrets
import shutil
import socket
import subprocess
import stat
import sys
import time

from fixtures import hashes, materialize, score
from harness import Ledger, sha
from lifecycle import BASE, Lifecycle, tree_hash
from readiness import check
from gateway import PRICING_CHECKED

USAGE_MAX_FILE=2*1024*1024
USAGE_MAX_TOTAL=8*1024*1024
USAGE_MAX_SEGMENT=256*1024
USAGE_MAX_ROWS=256
VERIFY_LOG_MAX=64*1024


def digest(path):
    h = hashlib.sha256()
    with Path(path).open('rb') as f:
        for chunk in iter(lambda: f.read(1024*1024), b''): h.update(chunk)
    return h.hexdigest()


def write_once(path, value):
    with Path(path).open('x') as f:
        json.dump(value, f, sort_keys=True, indent=2); f.write('\n')
        f.flush(); os.fsync(f.fileno())


def claim(ledger, task):
    """Permanent execution admission, independent of per-request money holds."""
    with ledger.transaction():
        ledger.db.execute('CREATE TABLE IF NOT EXISTS executions(id TEXT PRIMARY KEY,status TEXT NOT NULL,receipt_sha TEXT)')
        if ledger.config('halt'): raise ValueError('ledger halted')
        if ledger.db.execute("SELECT 1 FROM requests WHERE status='pending'").fetchone():
            raise ValueError('unknown in-flight request requires owner reconciliation')
        if ledger.db.execute("SELECT 1 FROM executions WHERE status!='completed'").fetchone():
            raise ValueError('unfinished execution; automatic replay forbidden')
        ledger.db.execute('INSERT INTO executions VALUES (?, ?, NULL)', (task['id'], 'started'))


def owner_checkpoint(config, ledger, request_id, mutation):
    """Attach current bytes before an owner-only task journal mutation.
    Each gateway attaches independently to the exact acknowledged new digest.
    """
    callback=None
    if config.get('checkpoint_adapter'):
        spec=importlib.util.spec_from_file_location('owner_checkpoint_client',config['checkpoint_adapter'])
        if spec is None or spec.loader is None: raise ValueError('invalid checkpoint client')
        module=importlib.util.module_from_spec(spec); spec.loader.exec_module(module)
        callback=module.build_checkpoint(config['ledger'])
    mutation()
    if callback is not None:
        try: callback('owner',request_id,ledger.db.serialize())
        except BaseException:
            with ledger.transaction(): ledger.db.execute("UPDATE config SET v='true' WHERE k='halt'")
            raise



def complete_execution(config, ledger, life, output, task_id, receipt):
    def complete():
        with ledger.transaction(): ledger.db.execute("UPDATE executions SET status='completed',receipt_sha=? WHERE id=?",(sha(receipt),task_id))
    owner_checkpoint(config,ledger,task_id+':completed',complete)
    # Physical reclamation follows the durable ACK; a lost ACK retains all evidence.
    life.prune_previous(ledger,output)
    life.evict_completed_snapshots(ledger,output)


def rpc(home, cmd, args=None):
    with socket.socket(socket.AF_UNIX) as sock:
        sock.settimeout(5); sock.connect(str(home/'run/owner.sock'))
        sock.sendall((json.dumps(dict(v=1,id='ab-owner',cmd=cmd,args=args or {}))+'\n').encode())
        stream = sock.makefile('rb')
        raw = stream.readline(2**20)
        if not raw.endswith(b'\n'): raise ValueError('invalid owner RPC frame')
        reply = json.loads(raw)
        if 'error' in reply or reply.get('ok') is False: raise ValueError('owner RPC refused '+cmd)
        return reply.get('result', reply)


def wait_for(fn, timeout=60):
    deadline=time.monotonic()+timeout
    while time.monotonic()<deadline:
        try:
            result=fn()
            if result: return result
        except (OSError, ValueError, KeyError): pass
        time.sleep(.2)
    raise TimeoutError('runtime readiness deadline')


def assert_policy(stats, policy, raw):
    got=stats.get('benchmark')
    if not isinstance(got,dict): raise ValueError('Root does not report benchmark enforcement')
    expected=dict(policy, frozen=policy['phase']!='learn',
                  policy_sha256=hashlib.sha256(raw).hexdigest())
    if got!=expected:
        raise ValueError('Root benchmark enforcement differs from frozen plan')
    return got


def run_prompt(home, prompt, wall_s):
    """Real owner channel; require observed busy -> idle, never infer from text."""
    frames=[]; busy=False; started=time.monotonic()
    with socket.socket(socket.AF_UNIX) as sock:
        sock.settimeout(1); sock.connect(str(home/'run/owner.sock'))
        sock.sendall(b'{"client":"release_ab","replay":false,"t":"attach"}\n')
        # Drain initial replay/state before submitting; prevents an old idle completing task.
        sock.settimeout(.2)
        try:
            while sock.recv(65536): pass
        except socket.timeout: pass
        sock.sendall((json.dumps({'t':'line','text':prompt})+'\n').encode())
        sock.settimeout(1); buffer=b''; received_bytes=0
        while time.monotonic()-started<wall_s:
            try: chunk=sock.recv(65536)
            except socket.timeout:
                if not busy and time.monotonic()-started>90: raise TimeoutError('no busy acknowledgement')
                continue
            if not chunk: raise ConnectionError('owner channel closed')
            received_bytes+=len(chunk)
            if received_bytes>16*1024*1024: raise ValueError('owner transcript exceeds 16 MiB bound')
            buffer+=chunk
            if len(buffer)>2**20: raise ValueError('owner frame exceeds bound')
            while b'\n' in buffer:
                raw,buffer=buffer.split(b'\n',1)
                value=json.loads(raw)
                if len(frames)>=20000: raise ValueError('owner trace exceeds bound')
                frames.append(value)
                if value.get('t')=='state':
                    if value.get('busy') is True: busy=True
                    elif busy and value.get('busy') is False:
                        return dict(status='completed',wall_s=time.monotonic()-started,frames=frames)
        sock.sendall(b'{"t":"interrupt"}\n')
        return dict(status='timeout',wall_s=time.monotonic()-started,frames=frames)


def fingerprints(home, relative_paths):
    out={}
    for relative in relative_paths:
        p=Path(relative)
        if p.is_absolute() or '..' in p.parts: raise ValueError('invalid frozen artifact path')
        target=home/p
        if target.is_symlink(): raise ValueError('freeze hash root must not be symlink')
        out[relative]=tree_hash(target,allowed_root=home) if target.is_dir() else digest(target)
    return out


def usage_snapshot(home):
    """Read only complete, bounded Root usage lines; agent mounts cannot write them."""
    directory=Path(home)/'extend/usage'
    if not directory.exists(): return {}
    if directory.is_symlink() or not directory.is_dir(): raise ValueError('invalid Root usage directory')
    result={};total=0
    for path in sorted(directory.glob('*.jsonl')):
        if not re.fullmatch(r'[0-9]{4}-[0-9]{2}-[0-9]{2}\.jsonl',path.name): raise ValueError('invalid Root usage file')
        fd=os.open(path,os.O_RDONLY|os.O_CLOEXEC|os.O_NOFOLLOW|os.O_NONBLOCK)
        try:
            metadata=os.fstat(fd)
            if not stat.S_ISREG(metadata.st_mode) or metadata.st_nlink!=1 or metadata.st_size>USAGE_MAX_FILE: raise ValueError('unbounded Root usage file')
            with os.fdopen(os.dup(fd),'rb') as stream:raw=stream.read(USAGE_MAX_FILE+1)
        finally:os.close(fd)
        if len(raw)>USAGE_MAX_FILE:raise ValueError('unbounded Root usage file')
        # A concurrent append may have an incomplete final line. It becomes
        # evidence only on the poll that observes its terminating newline.
        raw=raw[:raw.rfind(b'\n')+1] if raw else b''
        total+=len(raw)
        if total>USAGE_MAX_TOTAL:raise ValueError('unbounded Root usage history')
        for line in raw.splitlines():
            if type(json.loads(line)) is not dict:raise ValueError('invalid Root usage row')
        result[path.name]=raw
    return result


def usage_segment(home, baseline, prompt_started_at, prompt_ended_at):
    current=usage_snapshot(home);rows=[];segments=[]
    for name,raw in baseline.items():
        if name not in current or not current[name].startswith(raw):raise ValueError('Root usage prefix changed')
    for name,raw in current.items():
        start=len(baseline.get(name,b''));suffix=raw[start:]
        if not suffix:continue
        entries=[json.loads(line) for line in suffix.splitlines()]
        rows.extend(entries)
        segments.append(dict(file=name,start_bytes=start,end_bytes=len(raw),sha256=hashlib.sha256(suffix).hexdigest(),row_count=len(entries)))
    if len(rows)>USAGE_MAX_ROWS or sum(row['end_bytes']-row['start_bytes'] for row in segments)>USAGE_MAX_SEGMENT:
        raise ValueError('Root usage segment exceeds bound')
    selected=[]
    for index,row in enumerate(rows):
        try:
            began=datetime.fromisoformat(row['start'].replace('Z','+00:00')).timestamp()
            if row.get('kind')=='owner' and type(row.get('v')) is int and row['v']==1 and prompt_started_at-1<=began<=prompt_ended_at+1:
                selected.append(index)
        except (KeyError,TypeError,ValueError,OverflowError):pass
    return dict(rows=rows,rows_sha256=sha(rows),segments=segments,matched_owner_rows=len(selected),owner_row_indices=selected)


def wait_for_usage(home, baseline, prompt_started_at, prompt_ended_at, timeout=45):
    """Wait for an actual persisted turn, never substitute busy/idle or a fake turn."""
    started=time.monotonic();deadline=started+timeout
    proof=dict(v=1,status='missing',rows=[],rows_sha256=sha([]),segments=[],matched_owner_rows=0,owner_row_indices=[],
        prompt_start_ms=int(prompt_started_at*1000),prompt_end_ms=int(prompt_ended_at*1000),
        baseline=[dict(file=name,bytes=len(raw),sha256=hashlib.sha256(raw).hexdigest()) for name,raw in sorted(baseline.items())],timeout_s=timeout)
    while True:
        try:proof.update(usage_segment(home,baseline,prompt_started_at,prompt_ended_at))
        except (OSError,ValueError,TypeError):
            proof['status']='unavailable';break
        if proof['matched_owner_rows']:
            proof['status']='observed';break
        remaining=deadline-time.monotonic()
        if remaining<=0:break
        time.sleep(min(.2,remaining))
    proof['wait_s']=time.monotonic()-started
    return proof


def verify_lineage(root_home, init_home, expected_root_sha256):
    """Run only the read-only verifier, after Root exits, in explicit test homes."""
    root_home=Path(root_home);init_home=Path(init_home)
    binary=root_home/'bin/init-root';lineage=root_home/'lineage/lineage.jsonl'
    proof=dict(v=1,verified=False,full_chain_success=False,returncode=None,timed_out=False,
        root_binary_sha256=None,lineage_sha256=None,log_sha256=None,log_bytes=0,log_truncated=False,summary=None,code='verification_unavailable')
    try:
        before=digest(lineage);root_sha=digest(binary)
        proof.update(root_binary_sha256=root_sha,lineage_sha256=before)
        if root_sha!=expected_root_sha256:
            proof['code']='root_binary_mismatch';return proof
        clean=dict(PATH='/usr/bin:/bin',HOME=pwd.getpwuid(os.getuid()).pw_dir,INIT_ROOT_HOME=str(root_home),INIT_HOME=str(init_home))
        try:
            result=subprocess.run([str(binary),'audit','verify','--full'],env=clean,cwd=str(root_home),stdin=subprocess.DEVNULL,capture_output=True,timeout=60)
            raw=result.stdout+result.stderr;proof['returncode']=result.returncode
        except subprocess.TimeoutExpired as error:
            raw=(error.stdout or b'')+(error.stderr or b'');proof['timed_out']=True
        proof.update(log_sha256=hashlib.sha256(raw).hexdigest(),log_bytes=len(raw),log_truncated=len(raw)>VERIFY_LOG_MAX)
        success=re.search(rb'^lineage ok: [1-9][0-9]* records, head seq [1-9][0-9]* [0-9a-f]{64} \(full, [0-9]+ ms\)$',raw[:VERIFY_LOG_MAX],re.MULTILINE)
        proof['full_chain_success']=proof['returncode']==0 and success is not None and b'lineage INVALID' not in raw[:VERIFY_LOG_MAX] and not proof['log_truncated']
        if success is not None:proof['summary']=success.group().decode('ascii')[:256]
        stable=digest(lineage)==before and digest(binary)==root_sha
        proof['verified']=proof['full_chain_success'] and stable
        proof['code']='verified' if proof['verified'] else ('evidence_changed' if not stable else 'audit_failed')
    except (OSError,ValueError):pass
    return proof



def private_bytes(path, limit=16*1024*1024):
    fd=os.open(path,os.O_RDONLY|os.O_CLOEXEC|os.O_NOFOLLOW|os.O_NONBLOCK)
    try:
        meta=os.fstat(fd)
        if not stat.S_ISREG(meta.st_mode) or meta.st_nlink!=1 or meta.st_size>limit:
            raise ValueError('invalid private runtime evidence')
        with os.fdopen(os.dup(fd),'rb') as stream:raw=stream.read(limit+1)
        if len(raw)>limit:raise ValueError('runtime evidence exceeds bound')
        return raw
    finally:os.close(fd)


def current_tuple(life, verification):
    """Fold only fully verified, Root-authored pointer events, just as Root does."""
    raw=private_bytes(life.root_home/'lineage/lineage.jsonl')
    if verification.get('verified') is not True or hashlib.sha256(raw).hexdigest()!=verification.get('lineage_sha256'):
        raise ValueError('reset requires unchanged fully verified Root lineage')
    current=None
    for line in raw.splitlines():
        record=json.loads(line)
        if record.get('by')!='root':continue
        if record.get('kind')=='tuple':current=record['data']['id']
        elif record.get('kind')=='rollback':current=record['data']['to']
    if not isinstance(current,str) or not re.fullmatch(r't[0-9a-f]{12}',current):raise ValueError('missing admitted current tuple')
    value=json.loads(private_bytes(life.root_home/'store/tuples'/(current+'.json'),256*1024))
    if current!='t'+sha(value)[:12]:raise ValueError('admitted tuple digest mismatch')
    body=value.get('body',{})
    if not re.fullmatch(r'[0-9a-f]{40}',body.get('sha','')) or not re.fullmatch(r'[0-9a-f]{64}',body.get('binary','')):
        raise ValueError('admitted seeded Body is required')
    if digest(life.init_home/'bin/init-body')!=body['binary']:raise ValueError('live Body binary differs from admitted tuple')
    ext=value.get('ext',{});extlink=life.init_home/'bin/ext'
    if ext.get('set') is None:
        if ext.get('components')!=[] or extlink.exists() or extlink.is_symlink():raise ValueError('unadmitted live extensions')
    else:
        set_id=ext['set'];components=ext.get('components')
        if not isinstance(set_id,str) or not re.fullmatch(r'[0-9a-f]{12}',set_id) or not isinstance(components,list):raise ValueError('invalid admitted extension set')
        if not extlink.is_symlink() or os.readlink(extlink)!='ext.d/'+set_id:raise ValueError('live extension set differs from admitted tuple')
        expected={}
        for component in components:
            name=component.get('name');want=component.get('sha256')
            if not isinstance(name,str) or not re.fullmatch(r'[a-zA-Z0-9_-]+',name) or not isinstance(want,str) or not re.fullmatch(r'[0-9a-f]{64}',want):raise ValueError('invalid admitted component')
            if name in expected:raise ValueError('duplicate admitted component')
            expected[name+'.wasm']=want
        directory=life.init_home/'bin/ext.d'/set_id
        if directory.is_symlink() or not directory.is_dir():raise ValueError('invalid admitted extension directory')
        if {path.name for path in directory.iterdir()}!=set(expected):raise ValueError('unadmitted extension files')
        for name,want in expected.items():
            if hashlib.sha256(private_bytes(directory/name,512*1024*1024)).hexdigest()!=want:raise ValueError('admitted component changed')
    return current,value


def reset_tree_summary(path, allowed_root, exclude=()):
    """Hash ordinary state without exporting its filenames or text."""
    path=Path(path);allowed_root=Path(allowed_root).resolve()
    if not path.exists() and not path.is_symlink():return dict(present=False,sha256=None,entries=0)
    if path.is_symlink() or not path.is_dir():raise ValueError('reset root must be a real directory')
    entries=[]
    def walk(directory):
        for item in sorted(directory.iterdir()):
            if directory==path and item.name in exclude:continue
            metadata=item.lstat();mode=metadata.st_mode
            relative=item.relative_to(path).as_posix()
            if stat.S_ISLNK(mode):
                target=os.readlink(item);resolved=(item.parent/target).resolve(strict=False)
                if os.path.isabs(target) or (resolved!=allowed_root and allowed_root not in resolved.parents):raise ValueError('reset symlink escapes runtime')
                entries.append([relative,stat.S_IMODE(mode),'link',target])
            elif stat.S_ISREG(mode):
                if metadata.st_nlink!=1:raise ValueError('shared writable runtime inode refused')
                entries.append([relative,stat.S_IMODE(mode),'file',metadata.st_size,digest(item)])
            elif stat.S_ISDIR(mode):
                entries.append([relative,stat.S_IMODE(mode),'dir']);walk(item)
            else:raise ValueError('special ordinary runtime state refused')
    walk(path)
    return dict(present=True,sha256=sha(entries),entries=len(entries))


def body_head_without_git(body):
    """Observe HEAD as data; never execute an agent-controlled .git configuration."""
    try:
        git=body/'.git'
        if git.is_symlink() or not git.is_dir():return None
        head=private_bytes(git/'HEAD',4096).decode('ascii').strip()
        if re.fullmatch(r'[0-9a-f]{40}',head):return head
        if not head.startswith('ref: '):return None
        ref=head[5:]
        if not re.fullmatch(r'refs/heads/[A-Za-z0-9_/-]+',ref) or '..' in Path(ref).parts:return None
        for ancestor in (git/ref).parents:
            if ancestor==git:break
            if ancestor.is_symlink():return None
        value=private_bytes(git/ref,4096).decode('ascii').strip()
        return value if re.fullmatch(r'[0-9a-f]{40}',value) else None
    except (OSError,ValueError,UnicodeError):return None


def reset_runtime(life, verification):
    """Stopped-runtime reset shared by both arms; retain coupled kernel diagnostics.
    Source is rebuilt from Root's exact admitted tuple, never from candidate refs,
    untracked files, session logs or an untrusted checkout's Git configuration.
    """
    if life.state().get('stopped') is not True:raise ValueError('reset requires confirmed stopped runtime')
    for directory in (life.live,life.init_home,life.init_home/'lineage',life.init_home/'lineage/body.git'):
        if directory.is_symlink() or not directory.is_dir():raise ValueError('invalid trusted reset directory')
    tuple_id,value=current_tuple(life,verification);admitted=value['body']['sha'];body=life.init_home/'body'
    before_body=reset_tree_summary(body,life.live,exclude=('.git',));previous_head=body_head_without_git(body)
    before=dict(work=reset_tree_summary(life.init_home/'work',life.live),ordinary_state=reset_tree_summary(life.init_home/'state',life.live,exclude=('kernel',)))
    retained=[]
    for relative in ('state/kernel','cache','vault'):
        retained.append(dict(path=relative,**reset_tree_summary(life.init_home/relative,life.live)))
    clean=dict(PATH='/usr/bin:/bin',HOME=pwd.getpwuid(os.getuid()).pw_dir,GIT_CONFIG_NOSYSTEM='1',GIT_CONFIG_GLOBAL='/dev/null',
        GIT_ATTR_NOSYSTEM='1',GIT_TERMINAL_PROMPT='0')
    fixed=['/usr/bin/git','-c','core.hooksPath=/dev/null','-c','core.fsmonitor=false','-c','core.pager=cat','-c','gc.auto=0','-c','core.attributesFile=/dev/null']
    fresh=life.live/('.benchmark-body-'+secrets.token_hex(12))
    def git(args,cwd=None):
        result=subprocess.run(fixed+args,cwd=cwd,env=clean,stdin=subprocess.DEVNULL,stdout=subprocess.PIPE,stderr=subprocess.PIPE,timeout=120)
        if result.returncode!=0:raise ValueError('admitted Body reconstruction failed')
        return result.stdout
    try:
        git(['clone','--no-checkout','--no-local','--no-tags','--single-branch','--branch','main','--origin','origin',str(life.init_home/'lineage/body.git'),str(fresh)])
        git(['checkout','--force','-B','main',admitted],cwd=fresh)
        if git(['rev-parse','HEAD'],cwd=fresh).decode('ascii').strip()!=admitted:raise ValueError('reconstructed Body commit mismatch')
        if git(['status','--porcelain','--untracked-files=all'],cwd=fresh):raise ValueError('reconstructed Body is dirty')
        # The installed upstream is a trusted read-only bare repo as well.
        if (life.init_home/'lineage/seed.git').is_dir():git(['remote','add','upstream',str(life.init_home/'lineage/seed.git')],cwd=fresh)
        git(['config','--add','remote.origin.fetch','+refs/notes/*:refs/notes/*'],cwd=fresh)
        after_body=reset_tree_summary(fresh,life.live,exclude=('.git',))
        if body.is_symlink() or not body.is_dir():raise ValueError('invalid old Body checkout')
        shutil.rmtree(body);fresh.replace(body)
        for relative,keep in (('work',set()),('state',{'kernel'})):
            directory=life.init_home/relative
            if directory.is_symlink() or not directory.is_dir():raise ValueError('invalid ordinary runtime directory')
            for item in directory.iterdir():
                if item.name in keep:continue
                if item.is_symlink() or not item.is_dir():item.unlink()
                else:shutil.rmtree(item)
        for relative in ('work/body','work/kernel','state/run','state/sessions'):(life.init_home/relative).mkdir(parents=True,exist_ok=True)
        after=dict(work=reset_tree_summary(life.init_home/'work',life.live),ordinary_state=reset_tree_summary(life.init_home/'state',life.live,exclude=('kernel',)))
        if any(reset_tree_summary(life.init_home/row['path'],life.live)!= {key:value for key,value in row.items() if key!='path'} for row in retained):raise ValueError('retained operational state changed during reset')
        if digest(life.root_home/'lineage/lineage.jsonl')!=verification['lineage_sha256']:raise ValueError('Root lineage changed during reset')
        return dict(v=1,status='completed',source_tuple_id=tuple_id,admitted_body_sha=admitted,admitted_body_binary_sha256=value['body']['binary'],
            lineage_sha256=verification['lineage_sha256'],previous_body_head_sha=previous_head,
            body_before=before_body,body_after=after_body,discarded_unadmitted_body_changes=before_body!=after_body or previous_head!=admitted,
            before=before,after=after,ordinary_task_state_cleared=True,fresh_conversation=True,
            retained_runtime_state=retained,residual_memory_confound=True,information_free_reset=False,
            scope='exact admitted Body; ordinary work and non-kernel state reset; coupled operational state retained')
    finally:
        if fresh.exists():shutil.rmtree(fresh)


def preflight(config):
    evidence=json.loads(Path(config['evidence']).read_text()); report=check(evidence)
    manifest=evidence.get('manifest',{})
    fixtures=json.loads(Path(config['fixtures']).read_text())
    if hashes(fixtures)!=manifest.get('fixtures'): report['blockers'].append('Fixture/prompt/scorer differs from frozen manifest.')
    life=Lifecycle(config.get('base',str(BASE))); life.validate_runtime_paths()
    for key in ('ledger','fixtures','evidence','output'):
        path=Path(config[key]).resolve()
        if life.live.resolve() in path.parents or life.snapshots.resolve() in path.parents:
            report['blockers'].append(key+': owner artifact must be outside live/snapshot trees.')
    if type(config.get('gateway_port')) is not int or not 1024<=config['gateway_port']<=65535:
        report['blockers'].append('Explicit unprivileged gateway port required.')
    if not config.get('frozen_artifacts'): report['blockers'].append('Qualified frozen artifact paths are missing.')
    for binary, expected in config.get('binaries',{}).items():
        if digest(binary)!=expected: report['blockers'].append('Runtime binary changed: '+binary)
    root=str(life.root_home/'bin/init-root')
    if config.get('binaries',{}).get(root)!=manifest.get('binary_sha'):
        report['blockers'].append('Installed Root binary is not bound to manifest.')
    # Binding dirty source requires an archive digest in the qualification receipt.
    provenance=config.get('source_archive')
    if not isinstance(provenance,dict) or digest(provenance['path'])!=provenance.get('sha256'):
        report['blockers'].append('Exact source archive provenance missing or changed.')
    for name in ('platform','evolution_freeze','gateway_route'):
        pointer=evidence.get(name,{})
        if pointer.get('path'):
            receipt=json.loads(Path(pointer['path']).read_text())
            if receipt.get('runtime_binaries')!=config.get('binaries') or receipt.get('source_archive_sha256')!=(provenance or {}).get('sha256'):
                report['blockers'].append(name+': exact source/binary set not qualified.')
    # All hashes that define freeze must be covered by actual write-denial qualification.
    pointer=evidence.get('evolution_freeze',{})
    if pointer.get('path'):
        receipt=json.loads(Path(pointer['path']).read_text())
        if receipt.get('workspace_mode','external')!=config.get('workspace_mode','external'):
            report['blockers'].append('Workspace mode differs from actual sandbox qualification.')
        if receipt.get('frozen_artifacts')!=config.get('frozen_artifacts'):
            report['blockers'].append('Frozen paths differ from actual denial qualification.')
    if config.get('ephemeral_worker') is True and not config.get('checkpoint_adapter'):
        report['blockers'].append('Ephemeral worker requires a trusted external durable ledger checkpoint adapter.')
    if config.get('checkpoint_adapter'):
        if digest(config['checkpoint_adapter'])!=config.get('checkpoint_adapter_sha256'):
            report['blockers'].append('Checkpoint adapter hash mismatch.')
    report['ready']=not report['blockers']
    return report,evidence,fixtures,life


def execute(config, evidence, fixtures, life, limit):
    manifest=evidence['manifest']; ledger=Ledger(config['ledger'])
    if ledger.config('protocol')!=sha(manifest): raise ValueError('ledger protocol mismatch')
    if not os.environ.get('DEEPSEEK_API_KEY'): raise ValueError('DeepSeek key required in owner environment')
    if not life.meta.exists():
        life.adopt_installed(); life.stopped(True); life.snapshot('genesis')
    output=Path(config['output']); output.mkdir(parents=True,exist_ok=True)
    ledger.db.execute('CREATE TABLE IF NOT EXISTS executions(id TEXT PRIMARY KEY,status TEXT NOT NULL,receipt_sha TEXT)')
    for task in manifest['tasks'][:limit]:
        # Resume only a verified completed prefix. Interrupted tasks are never replayed.
        prior=ledger.db.execute('SELECT status,receipt_sha FROM executions WHERE id=?',(task['id'],)).fetchone()
        if prior and prior[0]=='completed':
            previous=json.loads((output/task['id']/'receipt.json').read_text())
            if sha(previous)!=prior[1]: raise ValueError('completed task receipt changed')
            continue
        if shutil.disk_usage(life.base).free < 27*1024**3: raise ValueError('requires 20 GiB reserve plus 7 GiB runtime headroom')
        owner_checkpoint(config,ledger,task['id']+':started',lambda: claim(ledger,task))
        source='genesis' if task['phase']=='learn' else f"{task['block']}-{task['arm']}-learned"
        life.restore(source)
        pre_task_verification=verify_lineage(life.root_home,life.init_home,config['binaries'][str(life.root_home/'bin/init-root')])
        pre_task_reset=reset_runtime(life,pre_task_verification)
        arm=life.arm_config(task)
        folder=output/task['id']; folder.mkdir()
        workspace=(life.init_home/'work/tasks'/task['id']) if config.get('workspace_mode')=='internal_tasks' else folder/'workspace'
        fixture=fixtures[task['family']][task['phase']]
        effective_prompt=fixture['prompt']+'\nTask workspace (use this directory for all task files): '+str(workspace)
        materialize(fixture,workspace)
        usage_baseline=usage_snapshot(life.root_home)
        phase='learn' if arm['evolution_allowed'] else ('genesis' if task['phase']=='learn' else 'transfer')
        port=config['gateway_port']; token=secrets.token_hex(32)
        policy=dict(v=1,run_id=task['id'],phase=phase,gateway_host=f'127.0.0.1:{port}',provider='deepseek',model='deepseek-flash',request_max_tokens=4096)
        policy_raw=json.dumps(policy,sort_keys=True).encode()
        policy_path=life.root_home/'benchmark.json'
        policy_path.write_bytes(policy_raw); policy_path.chmod(0o600)
        keypath=life.root_home/'keys/providers/deepseek'; keypath.parent.mkdir(parents=True,exist_ok=True)
        keypath.write_text(token); keypath.chmod(0o600)
        baseline=fingerprints(life.init_home,config['frozen_artifacts'])
        lineage_path=life.root_home/'lineage/lineage.jsonl'
        lineage_start=digest(lineage_path) if lineage_path.is_file() else None
        lineage_start_bytes=lineage_path.stat().st_size if lineage_path.is_file() else 0
        root=None; gateway=None; trace=None; stats=None; stopped=False;usage_proof=None
        runtime_started=time.monotonic()
        # Root receives no upstream credential. HOME must match passwd for broker rules;
        # explicit disposable INIT_* homes and no service installation remain mandatory.
        clean=dict(PATH='/usr/bin:/bin',HOME=pwd.getpwuid(os.getuid()).pw_dir,INIT_ROOT_HOME=str(life.root_home),INIT_HOME=str(life.init_home))
        gateway_env=dict(clean,DEEPSEEK_API_KEY=os.environ['DEEPSEEK_API_KEY'],INIT_AB_GATEWAY_TOKEN=token)
        if config.get('checkpoint_adapter'):
            for name in ('INIT_AB_CHECKPOINT_SOCKET','INIT_AB_CHECKPOINT_TOKEN'):
                if not os.environ.get(name): raise ValueError('trusted checkpoint IPC environment missing')
                gateway_env[name]=os.environ[name] # only this capability; never Modal credentials
        try:
            gateway_command=[sys.executable,str(Path(__file__).with_name('gateway.py')),'--ledger',config['ledger'],'--task-id',task['id'],'--port',str(port),'--pricing-checked',PRICING_CHECKED]
            if config.get('checkpoint_adapter'): gateway_command+=['--checkpoint-adapter',config['checkpoint_adapter']]
            gateway=subprocess.Popen(gateway_command,env=gateway_env,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
            def gateway_ready():
                if gateway.poll() is not None: raise RuntimeError('gateway failed to start')
                with socket.create_connection(('127.0.0.1',port),timeout=.2): return True
            wait_for(gateway_ready,10)
            with (folder/'root.log').open('xb') as log:
                root=subprocess.Popen([str(life.root_home/'bin/init-root'),'start','--foreground'],env=clean,stdout=log,stderr=log)
            wait_for(lambda: rpc(life.root_home,'status'))
            stats=rpc(life.root_home,'egress.stats'); assert_policy(stats,policy,policy_raw)
            if config.get('workspace_mode')=='internal_tasks':
                def workspace_ready():
                    status=rpc(life.root_home,'status'); access=status.get('access',{})
                    if access.get('workspace') or access.get('full') or access.get('read_home'):
                        raise RuntimeError('internal tasks require no broad workspace/home grant')
                    return status.get('slot')=='running' and (status.get('kernel') or {}).get('ready') is True
            else:
                rpc(life.root_home,'workspace.set',{'cwd':str(workspace.resolve())})
                def workspace_ready():
                    status=rpc(life.root_home,'status'); access=status.get('access',{})
                    return status.get('slot')=='running' and (status.get('kernel') or {}).get('ready') is True and access.get('workspace')==str(workspace.resolve()) and not access.get('restart_pending',True)
            wait_for(workspace_ready)
            assert_policy(rpc(life.root_home,'egress.stats'),policy,policy_raw)
            prompt_started_at=time.time()
            trace=run_prompt(life.root_home,effective_prompt,task['wall_s'])
            prompt_ended_at=time.time()
            usage_proof=wait_for_usage(life.root_home,usage_baseline,prompt_started_at,prompt_ended_at)
            stats=rpc(life.root_home,'egress.stats')
        finally:
            safe_stop=root is None
            try:
                if root is not None:
                    rpc(life.root_home,'stop')
                    def root_stopped():
                        status=rpc(life.root_home,'status')
                        return status.get('slot')=='stopped' and not status.get('instances') and status.get('kernel') is None
                    wait_for(root_stopped,30)
                    safe_stop=True
            finally:
                # Always close paid transport, even if Root shutdown failed. A request
                # interrupted in transport remains durably held and prevents replay.
                try:
                    if gateway is not None and gateway.poll() is None:
                        gateway.terminate()
                        try: gateway.wait(timeout=10)
                        except subprocess.TimeoutExpired: gateway.kill(); gateway.wait(timeout=5)
                finally:
                    try:
                        if root is not None and root.poll() is None:
                            root.terminate(); root.wait(timeout=30)
                    finally: keypath.unlink(missing_ok=True)
            if not safe_stop:
                raise RuntimeError('Root did not attest all runtime instances stopped; snapshot forbidden')
            life.stopped(True); stopped=True
        if not stopped or trace is None: raise ValueError('incomplete runtime; task remains non-replayable')
        verification=verify_lineage(life.root_home,life.init_home,config['binaries'][str(life.root_home/'bin/init-root')])
        runtime_wall_s=time.monotonic()-runtime_started
        after=fingerprints(life.init_home,config['frozen_artifacts'])
        if phase!='learn' and baseline!=after: raise ValueError('frozen runtime artifacts changed')
        score_result=score(fixture,workspace)
        if config.get('workspace_mode')=='internal_tasks':
            shutil.copytree(workspace,folder/'workspace',symlinks=True)
        write_once(folder/'owner-trace.json',trace)
        write_once(folder/'ceilings.json',stats)
        lineage=life.root_home/'lineage/lineage.jsonl'
        (folder/'lineage.jsonl').write_bytes(private_bytes(lineage))
        lineage_sha256=digest(folder/'lineage.jsonl')
        runtime_reset=reset_runtime(life,verification)
        if task['phase']=='learn': life.snapshot(f"{task['block']}-{task['arm']}-learned")
        owner_artifacts={'workspace':tree_hash(folder/'workspace',allowed_root=folder)}
        owner_artifacts.update({name:digest(folder/name) for name in ('owner-trace.json','ceilings.json','lineage.jsonl')})
        charged=ledger.db.execute('SELECT COALESCE(SUM(global_held),0) FROM requests WHERE run=?',(task['id'],)).fetchone()[0]
        receipt=dict(id=task['id'],arm=task['arm'],phase=task['phase'],status=trace['status'],passed=trace['status']=='completed' and score_result['passed'],score=score_result,charged_uusd=charged,protocol_sha=manifest['protocol_sha'],repo_sha=manifest['repo_sha'],binary_sha=manifest['binary_sha'],runtime_binaries=config['binaries'],source_archive_sha256=config['source_archive']['sha256'],attribution_status='requires_independent_lineage_verification',lineage_start_sha256=lineage_start,lineage_start_bytes=lineage_start_bytes,source_snapshot=source,source_snapshot_sha=life.state()['snapshots'][source],before_artifacts=baseline,after_artifacts=after,policy=policy,workspace_mode=config.get('workspace_mode','external'),effective_prompt_sha256=hashlib.sha256(effective_prompt.encode()).hexdigest(),output_path=str(folder/'workspace'),wall_s=trace['wall_s'])
        receipt.update(root_usage=usage_proof,lineage_verification=verification,runtime_wall_s=runtime_wall_s,
            pre_task_lineage_verification=pre_task_verification,pre_task_reset=pre_task_reset,runtime_reset=runtime_reset,
            owner_artifacts=owner_artifacts,lineage_sha256=lineage_sha256,residual_memory_confound=True,
            root_evidence_status='captured_and_verified' if verification['verified'] and usage_proof is not None and usage_proof['status']=='observed' else 'missing_or_unverified')
        write_once(folder/'receipt.json',receipt)
        complete_execution(config,ledger,life,output,task['id'],receipt)
    ledger.db.close()


def main():
    p=argparse.ArgumentParser(description=__doc__); p.add_argument('--config',required=True)
    p.add_argument('--execute',action='store_true'); p.add_argument('--limit',type=int,default=48)
    args=p.parse_args()
    if not 1<=args.limit<=48: p.error('limit must be 1..48; prefix only, never selected tasks')
    config=json.loads(Path(args.config).read_text())
    try: report,evidence,fixtures,life=preflight(config)
    except (OSError,ValueError,KeyError,TypeError) as exc:
        report=dict(ready=False,blockers=['invalid/missing preflight input: '+type(exc).__name__])
    print(json.dumps(report,indent=2))
    if not report['ready']: raise SystemExit(2)
    if args.execute: execute(config,evidence,fixtures,life,args.limit)

if __name__=='__main__': main()
