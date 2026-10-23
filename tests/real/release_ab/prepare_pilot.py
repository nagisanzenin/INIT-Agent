#!/usr/bin/env python3
"""Owner-only, $0 pilot setup and actual evidence aggregation.

`prepare` freezes fixtures/protocol, initializes one ledger, and writes configs.
The trusted parent then qualifies its durable checkpoint broker against that exact
ledger/protocol and supplies the resulting receipt. `finalize` aggregates actual
native/evolution evidence. Neither command calls a model or marks missing proof passed.
All paths must be supplied from the disposable cloud worker, never owner live homes.
"""
import argparse
from datetime import datetime, timezone, timedelta
from decimal import Decimal
import json
from pathlib import Path
import shutil
from aggregate_pilot import aggregate
from fixtures import registry, hashes
from gateway import GLOBAL_REQUEST_BOUND, INPUT_RATE, CACHE_RATE, OUTPUT_RATE, MAX_CONTEXT, MAX_OUTPUT
from harness import Ledger, freeze, plan
from lifecycle import BASE, Lifecycle
from qualify_native import FROZEN
from runner import digest, write_once, preflight


def read(path): return json.loads(Path(path).read_text())


def recent(value, now):
    checked=datetime.fromisoformat(value)
    if checked.tzinfo is None or not now-timedelta(hours=24)<=checked<=now+timedelta(minutes=5):
        raise ValueError('fresh timestamped parent observation required')


def available_budget(balance, pricing, now=None):
    """Validate actual owner snapshots; no API client or credential is imported."""
    now=now or datetime.now(timezone.utc)
    recent(balance['checked_at'],now); recent(pricing['checked_at'],now)
    if balance.get('provider')!='deepseek' or balance.get('is_available') is not True:
        raise ValueError('DeepSeek account unavailable')
    amounts=[Decimal(row['total_balance']) for row in balance['balance_infos'] if row.get('currency')=='USD']
    if len(amounts)!=1 or not amounts[0].is_finite() or amounts[0]<=0:
        raise ValueError('one positive USD balance required')
    expected=dict(model='deepseek-flash',context_reservation_tokens=MAX_CONTEXT,request_output_cap=MAX_OUTPUT,server_tools_allowed=False)
    if any(pricing.get(k)!=v for k,v in expected.items()): raise ValueError('pricing model/context/output mismatch')
    for key, rate in [('input_uncached_usd_per_million',INPUT_RATE),('input_cached_usd_per_million',CACHE_RATE),('output_usd_per_million',OUTPUT_RATE)]:
        if Decimal(pricing[key])*1000000!=rate: raise ValueError('pricing rate mismatch')
    if Decimal(pricing['global_reservation_usd'])*1000000!=GLOBAL_REQUEST_BOUND:
        raise ValueError('global reservation mismatch')
    if pricing.get('source')!='https://api-docs.deepseek.com/quick_start/pricing/':
        raise ValueError('official pricing source required')
    cap=min(10000000,int(amounts[0]*1000000))
    if cap<GLOBAL_REQUEST_BOUND: raise ValueError('balance below one fully reserved request')
    return cap


def outside_runtime(life, path):
    path=Path(path).resolve()
    for forbidden in (life.live.resolve(),life.snapshots.resolve()):
        if path==forbidden or forbidden in path.parents: raise ValueError('owner evidence/ledger must be outside runtime snapshots')
    return path


def prepare(config, output):
    output=Path(output).resolve(); life=Lifecycle(config.get('base',str(BASE)))
    life.validate_runtime_paths(); outside_runtime(life,output)
    if not life.state().get('stopped'): raise ValueError('qualification runtime must be stopped')
    # Never overwrite a prior ledger or reset a spent/ambiguous session.
    output.mkdir()
    source=dict(path=str(Path(config['source_archive']['path']).resolve()),sha256=config['source_archive']['sha256'])
    if digest(source['path'])!=source['sha256']: raise ValueError('source archive mismatch')
    binaries={str(Path(p).resolve()):expected for p,expected in config['runtime_binaries'].items()}
    for path,expected in binaries.items():
        if digest(path)!=expected: raise ValueError('runtime binary mismatch')
    root=str(life.root_home/'bin/init-root')
    if root not in binaries: raise ValueError('installed Root must be in exact runtime binary map')
    fixtures=registry(config.get('fixture_seed',2026102301)); write_once(output/'fixtures.json',fixtures)
    manifest=freeze(plan(config.get('order_seed',20261023)),hashes(fixtures),root,config['repo_sha'])
    write_once(output/'manifest.json',manifest)
    common={key:manifest[key] for key in ('repo_sha','binary_sha','protocol_sha')}
    common.update(runtime_binaries=binaries,source_archive_sha256=source['sha256'])
    balance=read(config['balance']); pricing=read(config['pricing'])
    cap=available_budget(balance,pricing)
    ledger_path=output/'spend.sqlite'; outside_runtime(life,ledger_path)
    ledger=Ledger(ledger_path)
    try: ledger.initialize(manifest,cap)
    finally: ledger.db.close()
    context=read(config['execution_context'])
    # This is an observation by the trusted parent, never inferred from model text.
    # The binding fields and disk count below are directly measured in this worker.
    names=context.get('agent_environment_names')
    if context.get('observed_by')!='trusted-parent' or context.get('credentials_outside_agent') is not True or not isinstance(names,list) or not all(isinstance(n,str) for n in names):
        raise ValueError('actual parent credential-isolation observation required')
    if any(n in ('DEEPSEEK_API_KEY','MODAL_TOKEN_ID','MODAL_TOKEN_SECRET','INIT_AB_CHECKPOINT_TOKEN') for n in names):
        raise ValueError('upstream/cloud/checkpoint credential exposed to agent environment')
    recent(context['checked_at'],datetime.now(timezone.utc))
    context.update(common,disk_free_bytes=shutil.disk_usage(life.base).free)
    write_once(output/'execution-context.json',context)
    billing=dict(common,status='passed',price_snapshot_checked=True,live_balance_checked=True,
                 full_context_reservation=True,ledger_outside_snapshots=True,ceiling_uusd=cap,
                 global_request_reservation_uusd=GLOBAL_REQUEST_BOUND,
                 observations={str(Path(p).resolve()):digest(p) for p in (config['balance'],config['pricing'])})
    write_once(output/'billing.json',billing)
    aggregate_config=dict(manifest=str(output/'manifest.json'),native=str(Path(config['native']).resolve()),
        evolution=str(Path(config['evolution']).resolve()),source_archive=source,runtime_binaries=binaries,
        execution_context=str(output/'execution-context.json'),billing=str(output/'billing.json'),
        durable_ledger=str(outside_runtime(life,config['durable_ledger'])))
    write_once(output/'aggregate-config.json',aggregate_config)
    runner=dict(base=str(life.base),ledger=str(ledger_path),fixtures=str(output/'fixtures.json'),
        output=str(output/'tasks'),gateway_port=config.get('gateway_port',18763),frozen_artifacts=list(FROZEN),
        workspace_mode='internal_tasks',binaries=binaries,source_archive=source,
        ephemeral_worker=config.get('ephemeral_worker',True))
    if config.get('checkpoint_adapter'):
        adapter=str(outside_runtime(life,config['checkpoint_adapter']))
        runner.update(checkpoint_adapter=adapter,checkpoint_adapter_sha256=digest(adapter))
    write_once(output/'runner-template.json',runner)
    write_once(output/'preparation.json',dict(paid_calls=0,qualified=False,manifest_sha256=digest(output/'manifest.json'),
        source_archive=source,required_next='Qualify trusted durable checkpoint broker on this ledger/protocol, then finalize.'))
    return dict(paid_calls=0,qualified=False,manifest=str(output/'manifest.json'),ledger=str(ledger_path),
                aggregate_config=str(output/'aggregate-config.json'))


def finalize(prepared, destination):
    prepared=Path(prepared).resolve(); runner=read(prepared/'runner-template.json')
    destination=outside_runtime(Lifecycle(runner['base']),destination)
    report=aggregate(read(prepared/'aggregate-config.json'),destination)
    runner['evidence']=str(destination/'evidence.json')
    write_once(destination/'runner-config.json',runner)
    try: driver_report,*_=preflight(runner)
    except (OSError,ValueError,KeyError,TypeError) as exc:
        driver_report=dict(ready=False,blockers=['actual driver preflight failed: '+type(exc).__name__])
    write_once(destination/'driver-preflight.json',driver_report)
    result=dict(paid_calls=0,qualified=report['ready'] and driver_report['ready'],readiness_path=str(destination/'evidence.json'),
                report_path=str(destination/'report.json'),runner_config=str(destination/'runner-config.json'))
    write_once(destination/'preflight-result.json',result)
    return result


def main():
    parser=argparse.ArgumentParser(description=__doc__); commands=parser.add_subparsers(dest='command',required=True)
    p=commands.add_parser('prepare'); p.add_argument('--config',required=True); p.add_argument('--output',required=True)
    p=commands.add_parser('finalize'); p.add_argument('--prepared',required=True); p.add_argument('--output',required=True)
    args=parser.parse_args()
    result=prepare(read(args.config),args.output) if args.command=='prepare' else finalize(args.prepared,args.output)
    print(json.dumps(result,indent=2))
    if args.command=='finalize':
        print('INIT_AB_READINESS_PATH='+result['readiness_path'])
        raise SystemExit(0 if result['qualified'] else 2)


if __name__=='__main__': main()
