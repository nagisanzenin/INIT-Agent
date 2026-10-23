#!/usr/bin/env python3
"""Offline native genesis bootstrap, explicitly invoked by owner/cloud dispatcher.
No downloads or provider calls. Uses existing Root install (which builds the Body).
Packages current extracted sources, not HEAD; keeps full native vendor crates.
"""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import pwd
import shutil
import subprocess
import tomllib
from lifecycle import BASE, Lifecycle, REPO, tree_hash
from runner import digest, write_once


def clean_env(home):
    return dict(PATH='/usr/bin:/bin',HOME=str(home),GIT_CONFIG_NOSYSTEM='1',GIT_CONFIG_GLOBAL='/dev/null',
                GIT_AUTHOR_DATE='2026-10-23T00:00:00Z',GIT_COMMITTER_DATE='2026-10-23T00:00:00Z',
                CARGO_NET_OFFLINE='true',TZ='UTC')


def launch_observation(env, keypath, token, driver):
    """Observe the actual trusted driver's Root launch inputs, without key bytes."""
    names=sorted(env)
    sensitive=lambda name: name in ('DEEPSEEK_API_KEY','INIT_AB_CHECKPOINT_TOKEN') or (name.startswith('MODAL_') and any(part in name for part in ('TOKEN','SECRET','KEY','AUTH','CREDENTIAL')))
    provider_files=sorted(p.name for p in keypath.parent.iterdir())
    ephemeral=(not keypath.is_symlink() and keypath.is_file() and keypath.read_text()==token
               and provider_files==['deepseek'] and keypath.stat().st_mode & 0o077==0)
    return dict(checked_at=datetime.now(timezone.utc).isoformat(),environment_names=names,upstream_and_cloud_credentials_absent=not any(sensitive(name) for name in names),
                provider_key_only_ephemeral=ephemeral,provider_credential_kind='synthetic_gateway_token' if ephemeral else 'unqualified',
                driver_source_sha256=digest(driver))


def copy_source(source, destination, *, full_vendor=False):
    """Only regular source trees; fail on links rather than following owner files."""
    def ignored(path,names): return [] if full_vendor else [n for n in names if n in ('.git','target','__pycache__')]
    for p in source.rglob('*'):
        if not full_vendor and any(part in ('.git','target','__pycache__') for part in p.relative_to(source).parts): continue
        if p.is_symlink(): raise ValueError('source symlink requires explicit packaging review')
    shutil.copytree(source,destination,ignore=ignored)


def verify_vendor(directory):
    """Cargo checksum files cover crate-internal directories named target too."""
    manifest=json.loads((directory/'.cargo-checksum.json').read_text())
    for name,expected in manifest['files'].items():
        relative=Path(name)
        if relative.is_absolute() or any(part in ('..','.') for part in relative.parts):
            raise ValueError('invalid vendored checksum path')
        path=directory/relative
        if any(p.is_symlink() for p in (path,*path.parents)) or not path.is_file() or digest(path)!=expected:
            raise ValueError('offline vendor checksum mismatch')


def vendor_seed(repo, seed):
    wanted=set()
    for name in ('Cargo.lock','ext/Cargo.lock'):
        path=seed/name
        if path.is_file():
            for pkg in tomllib.loads(path.read_text()).get('package',[]):
                if pkg.get('source','').startswith('registry+'):
                    wanted.add((pkg['name'],pkg['version']))
    for name,version in sorted(wanted):
        candidates=[repo/'vendor'/f'{name}-{version}',repo/'vendor'/name]
        found=next((p for p in candidates if (p/'Cargo.toml').is_file() and tomllib.loads((p/'Cargo.toml').read_text()).get('package',{}).get('version')==version),None)
        if found is None: raise ValueError(f'missing offline crate {name} {version}')
        copy_source(found,seed/'vendor'/found.name,full_vendor=True)
        verify_vendor(seed/'vendor'/found.name)
    cfg=seed/'.cargo/config.toml'; cfg.parent.mkdir(exist_ok=True)
    cfg.write_text('[source.crates-io]\nreplace-with = "vendored-sources"\n[source.vendored-sources]\ndirectory = "vendor"\n[net]\noffline = true\n')
    return len(wanted)


def bundle(tree, output, branch, tag=None):
    env=clean_env(tree.parent/'git-home'); Path(env['HOME']).mkdir(exist_ok=True)
    prefix=['/usr/bin/git','-C',str(tree),'-c','core.hooksPath=/dev/null','-c','user.name=init-benchmark-packager','-c','user.email=benchmark@init.invalid','-c','commit.gpgsign=false','-c','gc.auto=0']
    def git(*args): return subprocess.run(prefix+list(args),env=env,check=True,capture_output=True).stdout
    git('init','-q','-b',branch); git('add','-A'); git('commit','-q','-m','Frozen benchmark source package')
    refs=['refs/heads/'+branch]
    if tag: git('tag',tag); refs.append('refs/tags/'+tag)
    git('bundle','create',str(output),*refs)
    return git('rev-parse','HEAD').decode().strip()


def package(repo, binaries, output):
    output.mkdir() # immutable package; no overwriting previous evidence
    version=tomllib.loads((repo/'root/Cargo.toml').read_text())['workspace']['package']['version']
    dist=output/'release'; dist.mkdir(); (dist/'src').mkdir(); (dist/'seed').mkdir()
    for name in ('init-root','init-kernel','initctl','init-mockllm'):
        path=Path(binaries[name]); shutil.copy2(path,dist/name)
    for name,source in [('docs',repo/'docs/kernel'),('wit',repo/'wit'),('assets',repo/'root/crates/init-root/assets')]:
        copy_source(source,dist/name)
    smoke=repo/'kernel/kupgrade-smoke'
    if smoke.is_dir(): copy_source(smoke,dist/'kupgrade-smoke')
    seed=output/'seed-src'; copy_source(repo/'seed',seed); crates=vendor_seed(repo,seed)
    seed_hash=tree_hash(seed)
    seed_ref=bundle(seed,dist/'seed'/f'seed-v{version}.bundle','seed',f'seed-v{version}')
    kernel=output/'kernel-src'; kernel.mkdir()
    for name in ('kernel','wit'): copy_source(repo/name,kernel/name)
    kernel_hash=tree_hash(kernel)
    kernel_ref=bundle(kernel,dist/'src'/f'{version}.bundle','release')
    receipt=dict(kind='offline-source-packaging',version=version,seed_source_tree_sha256=seed_hash,kernel_source_tree_sha256=kernel_hash,seed_ref=seed_ref,kernel_ref=kernel_ref,full_vendor_crates=crates,stubbed_crates=0,binaries={name:digest(dist/name) for name in binaries if (dist/name).is_file()},files={str(p.relative_to(dist)):digest(p) for p in dist.rglob('*') if p.is_file()})
    write_once(output/'packaging.json',receipt)
    return dist,version


def charter(repo, port):
    text=(repo/'root/crates/init-root/assets/charter-template.toml').read_text()
    replacements={
        'text = "REPLACE ME: what INIT is for, in your words."':'text = "Disposable isolated benchmark qualification."',
        'destinations = ["api.deepseek.com:443"]':f'destinations = ["127.0.0.1:{port}"]',
        'paths = ["chat/completions", "beta/chat/completions", "models", "user/balance"]':'paths = ["chat/completions"]',
        'reserve_output_tokens = 65536':'reserve_output_tokens = 4096',
        'read_home = true':'read_home = false',
        'allow_full = true':'allow_full = false',
        'cmd = ["/usr/bin/osascript", "-e", "display notification \\"INIT needs you\\" with title \\"INIT\\""]':'cmd = []',
    }
    for old,new in replacements.items():
        if old not in text: raise ValueError('charter template changed; review '+old.split('=')[0])
        text=text.replace(old,new,1)
    text+='\n[extend]\nmode = "free"\n[actions.providers.providers.deepseek]\nhost = "127.0.0.1:%d"\nlocal = true\napi = "chat"\n[constitution]\ntext = %s\n' % (port,json.dumps((repo/'root/crates/init-root/assets/CONSTITUTION.md').read_text()))
    return text


def install(dist,version,toolchain,life,work,port):
    life.validate_runtime_paths()
    if life.live.exists(): raise ValueError('bootstrap requires absent live homes')
    if shutil.disk_usage(work).free<27*1024**3: raise ValueError('need 20 GiB reserve plus 7 GiB installation headroom')
    path=work/'charter.toml'; path.write_text(charter(REPO,port))
    # Required by Root class rules; --no-service prevents writes under passwd HOME.
    env=clean_env(pwd.getpwuid(os.getuid()).pw_dir)
    env.update(INIT_ROOT_HOME=str(life.root_home),INIT_HOME=str(life.init_home))
    with (work/'charter-check.log').open('xb') as log:
        checked=subprocess.run([str(dist/'init-root'),'charter','check',str(path)],env=env,stdout=log,stderr=log,timeout=30)
    if checked.returncode: raise ValueError('bootstrap charter rejected; inspect charter-check.log')
    command=[str(dist/'init-root'),'install','--release',str(dist),'--charter',str(path),'--toolchain',str(toolchain),'--seed-source',str(dist/'seed'/f'seed-v{version}.bundle'),'--seed-tag',f'seed-v{version}','--no-service']
    with (work/'install.log').open('xb') as log:
        result=subprocess.run(command,env=env,stdout=log,stderr=log,timeout=1800)
    write_once(work/'install-result.json',dict(returncode=result.returncode,command=command,log_sha256=digest(work/'install.log'),provider_calls=0))
    if result.returncode: raise ValueError('native genesis install failed; inspect install.log')
    config=life.init_home/'kernel/config.toml'
    data=tomllib.loads(config.read_text())
    if 'descriptor' in data.get('model',{}): raise ValueError('review existing model descriptor before normalization')
    config.write_text(config.read_text()+'\n[model.descriptor]\napi = "chat_completions"\ncontext_window = 1048576\nmax_output = 65536\ndefault_max_tokens = 4096\nrequest_max_tokens = 4096\n')
    life.adopt_installed(); life.stopped(True); fingerprint=life.snapshot('genesis')
    write_once(work/'genesis.json',dict(snapshot_sha256=fingerprint,root_home=str(life.root_home),init_home=str(life.init_home),source_packaging_sha256=digest(work/'packaging.json'),qualified=False,remaining=['kernel evolution requires legitimately earned L1; L0 body self-evolution A/B does not qualify kernel swap','native sandbox probes','actual frozen mutation denials','mock end-to-end request samples','restart and rollback']))


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--binaries',required=True,help='JSON mapping required binary names to exact built paths')
    p.add_argument('--output',required=True); p.add_argument('--toolchain',required=True)
    p.add_argument('--gateway-port',type=int,default=18763); p.add_argument('--install',action='store_true')
    args=p.parse_args()
    if not 1024<=args.gateway_port<=65535: p.error('unprivileged gateway port required')
    output=Path(args.output).resolve()
    if not (BASE==output or BASE in output.parents): p.error('output must stay under target/ab-homes')
    output.parent.mkdir(parents=True,exist_ok=True)
    if shutil.disk_usage(output.parent).free<27*1024**3: p.error('requires 27 GiB free; do not run on disk-constrained laptop')
    dist,version=package(REPO,json.loads(Path(args.binaries).read_text()),output)
    if args.install: install(dist,version,Path(args.toolchain).resolve(),Lifecycle(),output,args.gateway_port)
    print(json.dumps(dict(package=str(dist),installed=args.install,qualified=False,paid_calls=0)))

if __name__=='__main__': main()
