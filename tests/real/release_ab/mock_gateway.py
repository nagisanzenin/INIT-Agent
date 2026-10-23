#!/usr/bin/env python3
"""Synthetic upstream for native qualification. Contains NO upstream network client.
Exercises the production guard via injected in-memory transport; not model evidence.
Scenario labels are intentions, never proof that a runtime path executed.
"""
import argparse
import contextlib
import http.server
import io
import json
import os
from pathlib import Path
from gateway import Gateway, MODEL, handler
from harness import Ledger, plan
from runner import digest, write_once

class Response(io.BytesIO):
    status=200
    def __init__(self,payload,stream):
        super().__init__(payload); self.stream=stream
    def getheader(self,key,default=None):
        return {'Content-Type':'text/event-stream' if self.stream else 'application/json','Content-Encoding':'identity'}.get(key,default)

class Scripted:
    def __init__(self,scenario,output):
        self.scenario=scenario; self.output=Path(output); self.index=0
    @contextlib.contextmanager
    def __call__(self,body,key):
        if self.index>=len(self.scenario): raise ValueError('synthetic scenario exhausted')
        step=self.scenario[self.index]; index=self.index; self.index+=1
        value=json.loads(body); path=self.output/f'request-{index:03d}.json'
        with path.open('xb') as f: f.write(body)
        delta=step.get('delta',{'role':'assistant','content':'Synthetic qualification response.'})
        usage=step.get('usage',{'prompt_tokens':100,'completion_tokens':1,'total_tokens':101})
        common=dict(id=f'qualification-{index}',model=MODEL,created=0)
        finish=step.get('finish_reason','stop'); stream=value.get('stream',False)
        if stream:
            chunk=dict(common,object='chat.completion.chunk',choices=[dict(index=0,delta=delta,finish_reason=None)])
            terminal=dict(common,object='chat.completion.chunk',choices=[dict(index=0,delta={},finish_reason=finish)])
            tail=dict(common,object='chat.completion.chunk',choices=[],usage=usage)
            payload=b''.join(b'data: '+json.dumps(x).encode()+b'\n\n' for x in (chunk,terminal,tail))+b'data: [DONE]\n\n'
        else:
            payload=json.dumps(dict(common,object='chat.completion',choices=[dict(index=0,message=delta,finish_reason=finish)],usage=usage)).encode()
        write_once(self.output/f'request-{index:03d}.receipt.json',dict(transport='synthetic',intended_path=step.get('label','unspecified'),request_sha256=digest(path),request_max_tokens=value['max_tokens'],paid_calls=0))
        yield Response(payload,stream)


def main():
    p=argparse.ArgumentParser(description=__doc__); p.add_argument('--scenario',required=True)
    p.add_argument('--output',required=True); p.add_argument('--port',type=int,required=True)
    args=p.parse_args(); token=os.environ.get('INIT_AB_GATEWAY_TOKEN','')
    if len(token)<32: p.error('ephemeral test token required')
    out=Path(args.output); out.mkdir()
    ledger=Ledger(out/'synthetic-ledger.sqlite'); manifest=plan(); ledger.initialize(manifest,10000000)
    transport=Scripted(json.loads(Path(args.scenario).read_text()),out)
    gateway=Gateway(ledger,manifest['tasks'][0]['id'],'synthetic-key-never-used',transport)
    server=http.server.HTTPServer(('127.0.0.1',args.port),handler(gateway,token))
    try: server.serve_forever()
    finally: server.server_close(); ledger.db.close()

if __name__=='__main__': main()
