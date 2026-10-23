#!/usr/bin/env python3
"""Aggregate owner-controlled actual native qualification artifacts, never model claims.
Produces pilot evidence only; does not call models or fill absent checks with success.
Billing and durable cloud-storage evidence must come from the trusted parent lane.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re
from gateway import validate_request
from harness import sha
from qualify_evolution import CAP
from qualify_native import FROZEN, valid_root_denial
from readiness import check
from runner import digest, write_once


def read(path): return json.loads(Path(path).read_text())

def aggregate(config,destination):
    destination=Path(destination); destination.mkdir()
    manifest=read(config['manifest']); native=Path(config['native']); evolution=Path(config['evolution'])
    common={k:manifest[k] for k in ('repo_sha','binary_sha','protocol_sha')}
    common.update(runtime_binaries=config['runtime_binaries'],source_archive_sha256=config['source_archive']['sha256'])
    inputs={}; failures=[]
    def load(path):
        path=Path(path); value=read(path); inputs[str(path)]=digest(path); return value
    def require(ok,reason):
        if not ok: failures.append(reason)
        return bool(ok)
    evidence={'scope':'body-pilot-v2','manifest':manifest}
    def receipt(name,fields,passed=True,**extra):
        record=dict(common,status='passed' if passed else 'blocked',**fields,**extra)
        path=destination/(name+'.json'); write_once(path,record)
        evidence[name]=dict(path=str(path),sha256=digest(path))
    try:
        require(manifest.get('version')==2 and manifest['protocol_sha']==sha({k:v for k,v in manifest.items() if k!='protocol_sha'}),'frozen v2 protocol invalid')
        require(digest(config['source_archive']['path'])==common['source_archive_sha256'],'source archive mismatch')
        for path,expected in config['runtime_binaries'].items(): require(digest(path)==expected,'runtime binary mismatch: '+path)
        probe=load(native/'native-probe.json'); probe_log=(native/'native-probe.log').read_bytes()
        isolation=require(probe.get('passed') is True and probe.get('returncode')==0 and probe.get('root_binary_sha256')==manifest['binary_sha'] and hashlib.sha256(probe_log).hexdigest()==probe['log_sha256'] and re.search(rb'test result: ok\. 1 passed; 0 failed;',probe_log) is not None,'actual native isolation failed/missing/stale')
        freeze_ok=[]; wrong_model=[]; oversized=[]; normal=None
        for phase in ('genesis','transfer'):
            folder=native/phase; result=load(folder/'session-result.json'); stats=load(folder/'ceilings.json')
            policy=stats.get('benchmark',{}); raw={k:v for k,v in policy.items() if k not in ('frozen','policy_sha256')}
            policy_ok=require(policy.get('phase')==phase and policy.get('frozen') is True and policy.get('provider')=='deepseek' and policy.get('model')=='deepseek-flash' and policy.get('request_max_tokens')==4096 and hashlib.sha256(json.dumps(raw,sort_keys=True).encode()).hexdigest()==policy.get('policy_sha256'),'immutable policy mismatch: '+phase)
            text=[]
            for capture in result.get('captures',[]):
                path=Path(capture['path']); payload=path.read_bytes(); inputs[str(path)]=digest(path)
                require(inputs[str(path)]==capture['sha256'],'capture hash mismatch')
                value,*_=validate_request(payload)
                if normal is None: normal=dict(path=str(path),sha256=inputs[str(path)])
                for message in value['messages']:
                    if message.get('role')=='tool' and message.get('tool_call_id')=='qualification-denials': text.append(message.get('content',''))
            text='\n'.join(text)
            freeze_ok.append(require(policy_ok and result.get('owner_completed') is True and set(result.get('before_artifacts',{}))==set(FROZEN) and result['before_artifacts']==result.get('after_artifacts') and all('AB_WRITE_DENIED_'+str(i) in text for i in range(len(FROZEN))) and 'AB_WRITE_ALLOWED_' not in text and 'root.benchmark_frozen' in text,'actual frozen writes/change admission not denied: '+phase))
            direct=load(folder/'root-egress-denials.json'); pointer=result.get('root_egress_denials',{})
            bound=require(pointer.get('sha256')==digest(folder/'root-egress-denials.json'),'actual Root owner-wire proof hash missing: '+phase)
            wrong_model.append(require(bound and valid_root_denial(direct.get('wrong_model',{}),manifest['binary_sha'],policy.get('policy_sha256'),'benchmark-forbidden-model',1),'Root wrong-model owner-wire denial missing: '+phase))
            oversized.append(require(bound and valid_root_denial(direct.get('oversized_output',{}),manifest['binary_sha'],policy.get('policy_sha256'),'deepseek-flash',4097),'Root output-limit owner-wire denial missing: '+phase))
        evolution_result=load(evolution/'result.json')
        sessions={}; segments={}; usages={}
        for phase in ('learn','restart-reuse','rollback'):
            folder=evolution/phase; session=load(folder/'session.json')
            segments[phase]=load(folder/'lineage-segment.json'); usages[phase]=load(folder/'usage-segment.json')
            log=(folder/'lineage-verify.log').read_text()
            sessions[phase]=require(session.get('lineage_verified') is True and 'lineage INVALID' not in log and re.search(r'lineage ok: [1-9][0-9]* records,.*\(full,',log) is not None and digest(folder/'lineage-segment.json')==session['lineage_sha256'] and digest(folder/'usage-segment.json')==session['usage_sha256'],'lineage verification missing: '+phase)
        opens=[r for r in segments['learn'] if r.get('kind')=='change_open' and any(c.get('id')==CAP and c.get('op')=='add' and c.get('evidence')=='root' for c in r.get('data',{}).get('caps',[]))]
        admitted=require(bool(opens) and sessions['learn'] and evolution_result.get('admitted') is True,'actual body admission missing')
        reuse_session=load(evolution/'restart-reuse/session.json')
        reuse=require(sessions['restart-reuse'] and reuse_session.get('owner_completed') is True and reuse_session['before_artifacts']==reuse_session['after_artifacts'] and any(c.get('id')==CAP and c.get('evidence')=='root' and c.get('uses',0)>0 for row in usages['restart-reuse'] for c in row.get('caps',[])),'actual restart capability reuse missing')
        rollback_session=load(evolution/'rollback/session.json'); action=rollback_session.get('owner_action') or {}
        parent=opens[-1]['data'].get('parent_tuple') if opens else None
        rollback=require(sessions['rollback'] and action.get('skill_absent') is True and action.get('parent_tuple_restored')==parent and bool(parent) and evolution_result.get('rollback') is True,'actual owner rollback missing')
        guard=all(wrong_model+oversized) and normal is not None
        receipt('runtime_output',dict(configurable=guard,all_request_paths_enforced=guard,same_for_both_arms=guard),guard,max_tokens=4096,requests={'normal':normal},coverage='All calls bounded at immutable Root + external gateway; compaction/probe runtime behavior unqualified')
        receipt('universal_request_guard',dict(immutable_root_policy=all(freeze_ok),wrong_model_denied=all(wrong_model),oversized_output_denied=all(oversized),external_gateway_observed=normal is not None),guard and all(freeze_ok))
        receipt('evolution_freeze',dict(genesis_write_denied=freeze_ok[0],transfer_write_denied=freeze_ok[1],learned_capability_retained=admitted,restart_reuse_observed=reuse),all(freeze_ok) and admitted and reuse,frozen_artifacts=list(FROZEN),workspace_mode='internal_tasks')
        # Disk and credential isolation remain parent-observed evidence, not guessed here.
        context=load(config['execution_context'])
        bound=all(context.get(k)==v for k,v in common.items())
        receipt('platform',dict(native_runtime_passed=admitted and reuse,sandbox_passed=isolation,rollback_passed=rollback,disk_ready=context.get('disk_free_bytes',0)>=27*1024**3),bound and isolation and admitted and reuse and rollback and context.get('disk_free_bytes',0)>=27*1024**3)
        receipt('gateway_route',dict(external_guard_used=guard,direct_provider_bypass_denied=isolation and all(wrong_model),credentials_outside_agent=context.get('credentials_outside_agent') is True,mock_end_to_end_passed=guard and admitted),bound and guard and isolation and context.get('credentials_outside_agent') is True)
        for name in ('billing','durable_ledger'):
            path=Path(config[name]); evidence[name]=dict(path=str(path),sha256=digest(path)); inputs[str(path)]=digest(path)
    except (OSError,ValueError,KeyError,TypeError) as error:
        failures.append('missing/invalid actual qualification artifact: '+type(error).__name__)
    receipt('aggregation',dict(artifact_integrity_passed=not failures),not failures,failures=failures,source_artifacts=inputs)
    report=check(evidence); report['blockers']=failures+report['blockers']; report['ready']=not report['blockers']
    report['source_artifacts']=inputs
    write_once(destination/'evidence.json',evidence); write_once(destination/'report.json',report)
    return report


def main():
    p=argparse.ArgumentParser(description=__doc__); p.add_argument('--config',required=True); p.add_argument('--output',required=True)
    a=p.parse_args(); report=aggregate(read(a.config),a.output)
    print(json.dumps(report,indent=2)); raise SystemExit(0 if report['ready'] else 2)

if __name__=='__main__': main()
