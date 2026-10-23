#!/usr/bin/env python3
"""Offline arm lifecycle. Driver must stop all owned processes before snapshots.
Snapshots are copied at one stable live path to preserve installed absolute paths.
No subprocesses, API calls, live-home access, or symlink traversal.
"""
import hashlib
import json
import os
from pathlib import Path
import shutil
import socket
import stat
import sys

REPO = Path(__file__).resolve().parents[3]
BASE = REPO / 'target/ab-homes'


def clone_or_copy(source, destination):
    """Private CoW copy when supported; never share a writable inode."""
    if sys.platform.startswith('linux'):
        import fcntl
        created=False
        try:
            with open(source,'rb') as src:
                fd=os.open(destination,os.O_WRONLY|os.O_CREAT|os.O_EXCL,0o600); created=True
                with os.fdopen(fd,'wb') as dst:
                    fcntl.ioctl(dst.fileno(),0x40049409,src.fileno()) # Linux FICLONE
            shutil.copystat(source,destination)
            return destination
        except OSError:
            if created: Path(destination).unlink()
    return shutil.copy2(source,destination)


def _sha_file(path):
    info=Path(path).lstat()
    if not stat.S_ISREG(info.st_mode) or info.st_nlink!=1:
        raise ValueError('regular private owner artifact required')
    digest=hashlib.sha256()
    with Path(path).open('rb') as stream:
        for chunk in iter(lambda:stream.read(1024*1024),b''): digest.update(chunk)
    return digest.hexdigest()


def _json_sha(value):
    return hashlib.sha256(json.dumps(value,sort_keys=True,separators=(',',':'),allow_nan=False).encode()).hexdigest()


def tree_hash(path, allowed_root=None):
    if path.is_symlink() or not path.is_dir():
        raise ValueError('snapshot root must be a real directory')
    allowed_root=Path(allowed_root or path).resolve()
    if path.resolve()!=allowed_root and allowed_root not in path.resolve().parents:
        raise ValueError("hash subtree escapes allowed root")
    digest = hashlib.sha256(b"INIT-ab-tree-v2\0")
    for item in sorted(path.rglob('*')):
        relative = item.relative_to(path)
        if relative.parts[:2] == ('rh', 'egress'):
            continue  # cumulative Root spend is never part of a restorable snapshot
        mode = item.lstat().st_mode
        if stat.S_ISLNK(mode):
            target = os.readlink(item)
            resolved = (item.parent / target).resolve(strict=False)
            if os.path.isabs(target) or (resolved != allowed_root and allowed_root not in resolved.parents):
                raise ValueError('snapshot symlink escapes its root')
            rel = relative.as_posix().encode(); raw_target = os.fsencode(target)
            digest.update(len(rel).to_bytes(8, 'big')); digest.update(rel)
            digest.update(stat.S_IMODE(mode).to_bytes(4, 'big')); digest.update(b'L')
            digest.update(len(raw_target).to_bytes(8, 'big')); digest.update(raw_target)
            continue
        if not (stat.S_ISREG(mode) or stat.S_ISDIR(mode)):
            raise ValueError('snapshot special files forbidden; remove stopped sockets first')
        rel = item.relative_to(path).as_posix().encode()
        digest.update(len(rel).to_bytes(8, 'big')); digest.update(rel)
        digest.update(stat.S_IMODE(mode).to_bytes(4, 'big'))
        digest.update(b'F' if stat.S_ISREG(mode) else b'D')
        if stat.S_ISREG(mode):
            content = hashlib.sha256()
            total = 0
            with item.open('rb') as stream:
                before = os.fstat(stream.fileno())
                for chunk in iter(lambda: stream.read(1024*1024), b''):
                    total += len(chunk); content.update(chunk)
                after = os.fstat(stream.fileno())
            if total != before.st_size or (before.st_size, before.st_mtime_ns) != (after.st_size, after.st_mtime_ns):
                raise ValueError('snapshot changed during hashing')
            digest.update(total.to_bytes(8, 'big')); digest.update(content.digest())
    return digest.hexdigest()


class Lifecycle:
    def __init__(self, base=BASE):
        base = Path(base).absolute()
        if base != BASE and BASE not in base.parents:
            raise ValueError('homes must remain under repository target/ab-homes')
        for ancestor in (base, *base.parents):
            if ancestor.is_symlink(): raise ValueError('symlink home forbidden')
        self.base = base
        self.live = base / 'r'
        self.snapshots = base / 'snapshots'
        self.meta = base / 'state.json'
        self.root_home = self.live / 'rh'
        self.init_home = self.live / 'ih'

    def validate_runtime_paths(self):
        for p in (self.root_home, self.init_home):
            if len(os.fsencode(p)) > 64:
                raise ValueError('production home path exceeds 64-byte limit')

    def adopt_installed(self):
        self.validate_runtime_paths()
        if self.meta.exists() or self.snapshots.exists():
            raise ValueError('lifecycle already initialized')
        for directory in (self.live, self.root_home, self.init_home):
            if not directory.is_dir() or directory.is_symlink():
                raise ValueError('install genesis at the stable runtime paths first')
        self.snapshots.mkdir()
        self.save(dict(version=1, active=None, snapshots={}, stopped=False))

    def state(self):
        return json.loads(self.meta.read_text())

    def save(self, state):
        tmp = self.meta.with_suffix('.tmp')
        with tmp.open('w') as f:
            json.dump(state, f, sort_keys=True); f.flush(); os.fsync(f.fileno())
        tmp.replace(self.meta)

    def initialize(self):
        if self.base.exists(): raise ValueError('new lifecycle requires absent directory')
        self.live.mkdir(parents=True)
        self.snapshots.mkdir()
        self.root_home.mkdir(); self.init_home.mkdir()
        self.save(dict(version=1, active=None, snapshots={}, stopped=False))
        return self.live

    def stopped(self, owned_processes_exited):
        """Driver supplies confirmed wait() results, not a model-authored assertion.
        Socket and pid checks add fail-closed corroboration, not a process supervisor.
        """
        if owned_processes_exited is not True:
            raise ValueError('driver must confirm all owned processes exited')
        pidfile = self.root_home / 'run/root.pid'
        if pidfile.exists():
            pid = int(pidfile.read_text().strip())
            if pid <= 0: raise ValueError('invalid root PID')
            try: os.kill(pid, 0)
            except ProcessLookupError: pass
            else: raise ValueError('Root PID still exists')
        for home in (self.root_home, self.init_home):
            run = home / 'run'
            if run.exists():
                for item in run.iterdir():
                    if stat.S_ISSOCK(item.lstat().st_mode):
                        with socket.socket(socket.AF_UNIX) as sock:
                            sock.settimeout(0.2)
                            try: sock.connect(str(item))
                            except (ConnectionRefusedError, FileNotFoundError): pass
                            else: raise ValueError('runtime socket still accepts connections')
                        item.unlink() # exact stale socket generated by this test home
        state = self.state(); state['stopped'] = True; self.save(state)

    def snapshot(self, name):
        if not name.replace('-', '').isalnum(): raise ValueError('invalid snapshot name')
        state = self.state()
        if not state['stopped']: raise ValueError('runtime must be stopped')
        dest = self.snapshots / name
        if dest.exists() or name in state['snapshots']: raise ValueError('snapshots are immutable, including evicted names')
        providers = self.root_home / 'keys/providers'
        if providers.exists() and any(p.is_file() for p in providers.rglob('*')):
            raise ValueError('remove injected provider credentials before snapshot')
        fingerprint = tree_hash(self.live)
        shutil.copytree(self.live, dest, symlinks=True, copy_function=clone_or_copy,
                        ignore=lambda src, names: ['egress'] if Path(src) == self.root_home else [])
        if tree_hash(dest) != fingerprint: raise ValueError('snapshot verification failed')
        state['snapshots'][name] = fingerprint; self.save(state)
        return fingerprint

    def restore(self, name):
        state = self.state()
        if not state['stopped']: raise ValueError('runtime must be stopped')
        if name in state.get('evicted_snapshots',{}): raise ValueError('snapshot was deliberately evicted after completed transfers')
        fingerprint = state['snapshots'][name]
        source = self.snapshots / name
        if tree_hash(source) != fingerprint: raise ValueError('snapshot hash mismatch')
        # A monotonic counter cannot collide when certified old images are pruned.
        index=state.get('next_archive',0)
        while (self.base/f'previous-{index:04d}').exists(): index+=1
        archive = self.base / f'previous-{index:04d}'
        if archive.exists(): raise ValueError('archive collision')
        archived_hash=tree_hash(self.live)
        self.live.rename(archive)
        state['next_archive']=index+1
        state.setdefault('archives',{})[archive.name]=dict(sha256=archived_hash,
            task_id=(state.get('active') or {}).get('task_id'))
        self.save(state) # keep the exact old outcome registered even if copying fails
        shutil.copytree(source, self.live, symlinks=True,copy_function=clone_or_copy)
        accounting = archive / 'rh/egress'
        if accounting.exists():
            # Preserve all current spend across every restore, never snapshot money.
            # tree_hash validates links/special files before copying this trusted tree.
            tree_hash(accounting)
            shutil.copytree(accounting, self.root_home / 'egress', symlinks=True,copy_function=clone_or_copy)
        if tree_hash(self.live) != fingerprint: raise ValueError('restore verification failed')
        state['source_snapshot'] = name; state['active'] = None; self.save(state)

    def _completed_evidence(self, ledger, output, task_id):
        """Receipt and copied evidence must survive outside every disposable home."""
        if not isinstance(task_id,str) or not task_id or not task_id.replace('-','').isalnum() or Path(task_id).name!=task_id:
            raise ValueError('invalid completed task id')
        output=Path(output).absolute()
        for ancestor in (output,*output.parents):
            if ancestor.is_symlink(): raise ValueError('evidence directory symlink refused')
        if self.live in output.parents or self.snapshots in output.parents or output in (self.live,self.snapshots):
            raise ValueError('outcome evidence must be outside disposable runtime trees')
        if output.is_relative_to(self.base) and any(part.startswith('previous-') for part in output.relative_to(self.base).parts):
            raise ValueError('outcome evidence must not be a disposable archive')
        folder=output/task_id; receipt_path=folder/'receipt.json'
        for ancestor in (receipt_path,*receipt_path.parents):
            if ancestor.is_symlink(): raise ValueError('evidence symlink refused')
        row=ledger.db.execute('SELECT status,receipt_sha FROM executions WHERE id=?',(task_id,)).fetchone()
        receipt=json.loads(receipt_path.read_text())
        if not row or row[0]!='completed' or row[1]!=_json_sha(receipt) or receipt.get('id')!=task_id:
            raise ValueError('completed task journal does not bind immutable receipt')
        artifacts=receipt.get('owner_artifacts')
        if not isinstance(artifacts,dict) or not {'workspace','owner-trace.json','ceilings.json','lineage.jsonl'}<=set(artifacts):
            raise ValueError('completed outcome artifact bindings missing')
        for relative,expected in artifacts.items():
            path=Path(relative)
            if path.is_absolute() or '..' in path.parts or not path.parts: raise ValueError('invalid owner artifact path')
            target=folder/path
            for ancestor in (target,*target.parents):
                if ancestor==folder: break
                if ancestor.is_symlink(): raise ValueError('owner artifact symlink refused')
            actual=tree_hash(target,allowed_root=folder) if target.is_dir() else _sha_file(target)
            if actual!=expected: raise ValueError('completed owner artifact changed')
        return row[1]

    def prune_previous(self, ledger, output):
        """Call only after the trusted completed checkpoint ACK. Unknown images stay."""
        state=self.state()
        if not state['stopped']: raise ValueError('runtime must be stopped before pruning')
        removed=[]
        for name,record in list(state.get('archives',{}).items()):
            if not name.startswith('previous-') or not name[9:].isdigit() or len(name)>32:
                raise ValueError('invalid managed archive name')
            task_id=record.get('task_id')
            if not task_id: continue # qualification/unreceipted outcomes are not disposable
            self._completed_evidence(ledger,output,task_id)
            path=self.base/name
            if tree_hash(path)!=record['sha256']: raise ValueError('managed archive changed')
            shutil.rmtree(path) # exact registered stopped image, never a user home
            state.setdefault('pruned_archives',{})[name]=dict(record,completed_receipt_sha256=ledger.db.execute(
                'SELECT receipt_sha FROM executions WHERE id=?',(task_id,)).fetchone()[0])
            del state['archives'][name]; self.save(state); removed.append(name)
        return removed

    def evict_completed_snapshots(self, ledger, output):
        """Release learned images only after both independently restored transfers finish."""
        state=self.state()
        if not state['stopped']: raise ValueError('runtime must be stopped before eviction')
        removed=[]
        for name,fingerprint in state['snapshots'].items():
            if not name.endswith('-learned') or name in state.get('evicted_snapshots',{}): continue
            prefix=name[:-len('-learned')]
            tasks=[prefix+'-'+phase for phase in ('learn','transfer-1','transfer-2')]
            rows=[ledger.db.execute('SELECT status FROM executions WHERE id=?',(task,)).fetchone() for task in tasks]
            if not all(row and row[0]=='completed' for row in rows): continue
            bindings={task:self._completed_evidence(ledger,output,task) for task in tasks}
            path=self.snapshots/name
            if tree_hash(path)!=fingerprint: raise ValueError('completed learned snapshot changed')
            shutil.rmtree(path) # exact immutable registered snapshot; genesis never matches
            state.setdefault('evicted_snapshots',{})[name]=dict(sha256=fingerprint,completed_receipts=bindings)
            self.save(state); removed.append(name)
        return removed

    def arm_config(self, task):
        state = self.state()
        if not state['stopped']: raise ValueError('previous runtime must be stopped')
        if task['arm'] not in ('A', 'B'): raise ValueError('invalid arm')
        source = state.get('source_snapshot')
        expected = 'genesis' if task['phase'] == 'learn' else f"{task['block']}-{task['arm']}-learned"
        if source != expected: raise ValueError('wrong source snapshot for task')
        config = dict(task_id=task['id'], arm=task['arm'], phase=task['phase'],
                      evolution_allowed=task['arm']=='B' and task['phase']=='learn',
                      preserve_existing_capabilities=True, model='deepseek-flash', provider='deepseek',
                      cap_uusd=task['cap_uusd'], wall_s=task['wall_s'], source_snapshot=source,
                      root_home=str(self.root_home), init_home=str(self.init_home))
        # A declarative driver contract, not Root enforcement. Driver must verify
        # support before start; extend.mode=gated is NOT equivalent to freeze.
        state['active'] = config; state['stopped'] = False; self.save(state)
        return config

    def admit_task(self, ledger, task):
        """Reserve whole cap once BEFORE a trusted driver launches Root.
        Root must enforce <=cap additional spending from current cumulative ledger,
        including setup/twin/sealed lanes. Returned values do not configure Root.
        """
        state = self.state()
        if not state['stopped']:
            raise ValueError('previous runtime must be stopped before admission')
        expected = 'genesis' if task['phase'] == 'learn' else f"{task['block']}-{task['arm']}-learned"
        if state.get('source_snapshot') != expected:
            raise ValueError('restore correct snapshot before admission')
        request = 'task:' + task['id']
        # Deterministic ID is permanently consumed even if settled at zero.
        ledger.reserve(request, task['id'], task['cap_uusd'])
        config = self.arm_config(task)
        config['reservation_id'] = request
        config['root_additional_budget_uusd'] = task['cap_uusd']
        state = self.state(); state['active'] = config; self.save(state)
        return config

    def finish_task(self, ledger, validated_actual_uusd=None):
        """Call only after confirmed owned-process exit and durable usage capture.
        Default retains full reservation; no need to trust delayed balance reads.
        """
        state = self.state()
        if not state['stopped']:
            raise ValueError('stop and wait all owned processes before settlement')
        config = state['active']
        if not config or 'reservation_id' not in config:
            raise ValueError('task has no admission')
        ledger.settle(config['reservation_id'], validated_actual_uusd)
        state['last_task'] = config; state['active'] = None; self.save(state)
