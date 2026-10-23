#!/usr/bin/env python3
"""Offline, fail-closed A/B preflight. Receipt evidence must be owner-controlled.
This verifies supplied qualification artifacts, not the truth of self-authored claims.
"""
import argparse
import hashlib
import json
from pathlib import Path
from gateway import MAX_OUTPUT, validate_request
from harness import sha


def check(evidence=None):
    evidence=evidence or {}
    blockers=[]
    manifest=evidence.get('manifest')
    if not isinstance(manifest,dict) or 'protocol_sha' not in manifest:
        blockers.append('Frozen protocol manifest is missing.')
    elif sha({k:v for k,v in manifest.items() if k!='protocol_sha'})!=manifest['protocol_sha']:
        blockers.append('Frozen protocol hash mismatch.')
    bindings={key:(manifest if isinstance(manifest,dict) else {}).get(key) for key in ('repo_sha','binary_sha','protocol_sha')}
    def receipt(name,fields):
        pointer=evidence.get(name)
        if not isinstance(pointer,dict):
            blockers.append(f'{name}: qualification receipt missing.'); return None
        try:
            raw=Path(pointer['path']).read_bytes()
            if len(raw)>1048576 or hashlib.sha256(raw).hexdigest()!=pointer['sha256']: raise ValueError()
            data=json.loads(raw)
            if data.get('status')!='passed' or any(data.get(k)!=v or not v for k,v in bindings.items()): raise ValueError()
            if any(data.get(field) is not True for field in fields): raise ValueError()
            return data
        except (OSError,ValueError,KeyError,TypeError):
            blockers.append(f'{name}: invalid, stale or failed qualification receipt.'); return None
    pilot=evidence.get('scope')=='body-pilot-v2'
    runtime=receipt('runtime_output',('configurable','all_request_paths_enforced','same_for_both_arms'))
    if runtime is not None:
        cap=runtime.get('max_tokens')
        if type(cap) is not int or not 1<=cap<=MAX_OUTPUT:
            blockers.append(f'Runtime output cap must be explicitly configurable to <= {MAX_OUTPUT}.')
        samples=runtime.get('requests',{})
        for kind in (('normal',) if pilot else ('normal','length_retry','compaction','probe')):
            try:
                sample=Path(samples[kind]['path']).read_bytes()
                if hashlib.sha256(sample).hexdigest()!=samples[kind]['sha256']: raise ValueError()
                value,*_=validate_request(sample)
                if value['max_tokens']>cap: raise ValueError()
            except (OSError,ValueError,KeyError,TypeError):
                blockers.append(f'Output-cap qualification missing/rejected request sample: {kind}.')
    else:
        blockers.append('Unqualified runtime output configuration: a descriptor can request <=4,096 tokens, but no captured normal/retry/compaction/probe evidence proves that cap is active. Built-in defaults include 65,536 tokens and 16,000-token compaction; no silent normalization is permitted.')
    receipt('evolution_freeze',('genesis_write_denied','transfer_write_denied','learned_capability_retained','restart_reuse_observed'))
    receipt('platform',('native_runtime_passed','sandbox_passed','rollback_passed','disk_ready'))
    receipt('gateway_route',('external_guard_used','direct_provider_bypass_denied','credentials_outside_agent','mock_end_to_end_passed'))
    receipt('billing',('price_snapshot_checked','live_balance_checked','full_context_reservation' if pilot else 'input_bound_assumption_reviewed','ledger_outside_snapshots'))
    if pilot:
        receipt('aggregation',('artifact_integrity_passed',))
        receipt('universal_request_guard',('immutable_root_policy','wrong_model_denied','oversized_output_denied','external_gateway_observed'))
        receipt('durable_ledger',('cloud_single_writer','durable_before_post','recovery_fail_closed','checkpoint_monotonic'))
    return dict(ready=not blockers,blockers=blockers,preflight_api_calls=0,
                scope='Qualified L0 body pilot only' if pilot else 'Offline full-coverage evidence preflight',
                remaining_release_gaps=['compaction and probe request coverage','kernel evolution at legitimately earned L1','other OS and provider native qualifications'] if pilot else [])


def main():
    parser=argparse.ArgumentParser(description=__doc__); parser.add_argument('--evidence')
    args=parser.parse_args()
    report=check(json.loads(Path(args.evidence).read_text()) if args.evidence else None)
    print(json.dumps(report,indent=2)); raise SystemExit(0 if report['ready'] else 2)

if __name__=='__main__': main()
