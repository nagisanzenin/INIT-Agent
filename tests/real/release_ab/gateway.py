#!/usr/bin/env python3
"""Owner-controlled DeepSeek-only benchmark gateway. NEVER a product spend ceiling.
No network occurs on import. Real serving is explicit. Fixed upstream, no redirects/retries.
Body-byte input bound includes a documented conservative framing assumption; see protocol.
"""
import argparse
import contextlib
import hmac
import http.client
import http.server
import importlib.util
import json
import math
import os
import re
import socket
import time
import threading
import uuid
from harness import Ledger, worst_case_uusd, usage_tokens

HOST = 'api.deepseek.com'
PATH = '/chat/completions'
MODEL = 'deepseek-flash'
PRICING_CHECKED = '2026-10-23'
INPUT_RATE = 300000        # micro USD / million tokens, peak uncached
CACHE_RATE = 6000
OUTPUT_RATE = 1200000
MAX_REQUEST = 512 * 1024
MAX_RESPONSE = 8 * 1024 * 1024
MAX_LINE = 1024 * 1024
MAX_OUTPUT = 4096
MAX_CONTEXT = 1048576
GLOBAL_REQUEST_BOUND = worst_case_uusd(MAX_CONTEXT, MAX_OUTPUT, INPUT_RATE, OUTPUT_RATE)
READ_TIMEOUT = 20
WALL_SECONDS = 180
NAME = re.compile(r'^[a-zA-Z0-9_-]{1,128}$')

class Rejected(ValueError): pass
class UpstreamFailure(RuntimeError): pass


def object_keys(value, allowed, required=()):
    if type(value) is not dict or set(value)-set(allowed) or set(required)-set(value):
        raise Rejected('unsupported or missing object field')


def string(value, nullable=False):
    if nullable and value is None: return
    if type(value) is not str: raise Rejected('text-only content required')


def count(value, lo=0, hi=10**9):
    if type(value) is not int or not lo <= value <= hi: raise Rejected('integer outside allowed range')
    return value


def strict_json(raw):
    def pairs(items):
        obj = {}
        for k,v in items:
            if k in obj: raise Rejected('duplicate JSON key')
            obj[k]=v
        return obj
    def constant(_): raise Rejected('nonfinite JSON number')
    try: return json.loads(raw, object_pairs_hook=pairs, parse_constant=constant)
    except (ValueError, RecursionError, UnicodeError) as exc: raise Rejected('invalid JSON') from None


def schema_safe(value, depth=0):
    if depth > 24: raise Rejected('schema nesting too deep')
    if isinstance(value,dict):
        if '$ref' in value or '$dynamicRef' in value: raise Rejected('schema references forbidden')
        for child in value.values(): schema_safe(child,depth+1)
    elif isinstance(value,list):
        for child in value: schema_safe(child,depth+1)


def tool_function(value):
    object_keys(value, {'name','description','parameters'}, {'name'})
    if type(value['name']) is not str or not NAME.fullmatch(value['name']): raise Rejected('invalid tool name')
    if 'description' in value: string(value['description'])
    if 'parameters' in value:
        if type(value['parameters']) is not dict: raise Rejected('tool schema must be object')
        schema_safe(value['parameters'])


def validate_request(raw):
    if not isinstance(raw,bytes) or len(raw)>MAX_REQUEST: raise Rejected('request too large')
    value=strict_json(raw)
    object_keys(value, {'model','messages','tools','tool_choice','max_tokens','stream','stream_options',
                        'temperature','top_p','stop','response_format','reasoning_effort','thinking'},
                       {'model','messages','max_tokens'})
    if value['model']!=MODEL: raise Rejected('only frozen DeepSeek Flash allowed')
    count(value['max_tokens'],1,MAX_OUTPUT)
    messages=value['messages']
    if type(messages) is not list or not 1<=len(messages)<=512: raise Rejected('invalid message count')
    for msg in messages:
        object_keys(msg, {'role','content','name','tool_call_id','tool_calls','reasoning_content'}, {'role'})
        role=msg['role']
        if role not in ('system','user','assistant','tool'): raise Rejected('unsupported message role')
        if 'name' in msg:
            string(msg['name'])
            if not NAME.fullmatch(msg['name']): raise Rejected('invalid message name')
        if 'content' not in msg: raise Rejected('message content required')
        string(msg['content'],nullable=role=='assistant')
        if role=='tool': string(msg.get('tool_call_id'))
        elif 'tool_call_id' in msg: raise Rejected('tool id on nontool message')
        if 'reasoning_content' in msg:
            if role!='assistant': raise Rejected('reasoning only allowed on assistant history')
            string(msg['reasoning_content'],nullable=True)
        if 'tool_calls' in msg:
            calls=msg['tool_calls']
            if role!='assistant' or type(calls) is not list or len(calls)>64: raise Rejected('invalid tool history')
            for call in calls:
                object_keys(call,{'id','type','function'},{'id','type','function'})
                string(call['id'])
                if call['type']!='function': raise Rejected('only local function tools allowed')
                object_keys(call['function'],{'name','arguments'},{'name','arguments'})
                if type(call['function']['name']) is not str or not NAME.fullmatch(call['function']['name']): raise Rejected('invalid tool name')
                string(call['function']['arguments'])
    tools=value.get('tools',[])
    if type(tools) is not list or len(tools)>64: raise Rejected('invalid tools')
    names=set()
    for tool in tools:
        object_keys(tool,{'type','function'},{'type','function'})
        if tool['type']!='function': raise Rejected('server tools forbidden')
        tool_function(tool['function'])
        name=tool['function']['name']
        if name in names: raise Rejected('duplicate tool name')
        names.add(name)
    if 'tool_choice' in value and value['tool_choice'] not in ('auto','none'): raise Rejected('unsupported tool choice')
    if 'stream' in value and type(value['stream']) is not bool: raise Rejected('stream must be boolean')
    if 'stream_options' in value:
        if value.get('stream') is not True: raise Rejected('stream options without streaming')
        object_keys(value['stream_options'],{'include_usage'},{'include_usage'})
        if value['stream_options']['include_usage'] is not True: raise Rejected('usage stream required')
    if value.get('stream'): value['stream_options']={'include_usage':True}
    for key,low,high in [('temperature',0,2),('top_p',0,1)]:
        if key in value:
            num=value[key]
            if type(num) not in (int,float) or not math.isfinite(num) or not low<=num<=high: raise Rejected('invalid sampling parameter')
    if 'reasoning_effort' in value and value['reasoning_effort'] not in ('none','low','high'): raise Rejected('unsupported reasoning effort')
    if 'thinking' in value:
        object_keys(value['thinking'],{'type'},{'type'})
        if value['thinking']['type'] not in ('enabled','disabled'): raise Rejected('unsupported thinking mode')
    if 'stop' in value:
        stops=value['stop'] if isinstance(value['stop'],list) else [value['stop']]
        if len(stops)>16 or any(type(s) is not str for s in stops): raise Rejected('invalid stop sequences')
    if 'response_format' in value:
        object_keys(value['response_format'],{'type'},{'type'})
        if value['response_format']['type'] not in ('text','json_object'): raise Rejected('unsupported format')
    body=json.dumps(value,separators=(',',':'),ensure_ascii=False,allow_nan=False).encode()
    if len(body)>MAX_REQUEST: raise Rejected('normalized request too large')
    input_bound=len(body)+4096+64*(len(messages)+len(tools))
    return value,body,input_bound,worst_case_uusd(input_bound,value['max_tokens'],INPUT_RATE,OUTPUT_RATE)


def usage_cost(value):
    if type(value) is not dict: raise Rejected('missing usage')
    try: tokens=usage_tokens(value)
    except ValueError: raise Rejected('invalid reported token/cache usage') from None
    prompt=tokens['prompt_tokens']; output=tokens['completion_tokens']
    if count(value.get('total_tokens'),1)!=prompt+output: raise Rejected('inconsistent usage totals')
    hit,miss=tokens['prompt_cache_hit_tokens'],tokens['prompt_cache_miss_tokens']
    # Budget conservatively at the full uncached rate if either actual cache
    # field is absent. This pricing fallback is never an observed zero hit count.
    if hit is None or miss is None: hit,miss=0,prompt
    details=value.get('completion_tokens_details')
    if details is not None:
        if type(details) is not dict: raise Rejected('invalid completion breakdown')
        if count(details.get('reasoning_tokens',0))>output: raise Rejected('reasoning outside total output')
    # Reasoning is a subset of completion_tokens, never double-count it.
    cost=(miss*INPUT_RATE+hit*CACHE_RATE+output*OUTPUT_RATE+999999)//1000000
    return prompt,output,cost


@contextlib.contextmanager
def upstream(body,key):
    connection=http.client.HTTPSConnection(HOST,443,timeout=READ_TIMEOUT)
    timer=None
    try:
        # Explicit TLS connect: certificate validation is enabled by default.
        # No environment proxy discovery, redirects or retry loops exist here.
        connection.connect()
        sock=connection.sock
        def deadline():
            try: sock.shutdown(socket.SHUT_RDWR)
            except OSError: pass
        timer=threading.Timer(WALL_SECONDS,deadline); timer.daemon=True; timer.start()
        connection.request('POST',PATH,body=body,headers={'Authorization':'Bearer '+key,
                           'Content-Type':'application/json','Accept-Encoding':'identity','User-Agent':'INIT-release-benchmark/1'})
        yield connection.getresponse()
    finally:
        if timer is not None: timer.cancel()
        connection.close()


class Gateway:
    def __init__(self, ledger, task_id, api_key, transport=upstream, checkpoint=None):
        if not api_key or '\n' in api_key or '\r' in api_key: raise Rejected('invalid runtime credential')
        ledger.config('ceiling')
        if ledger.config('global_request_bound')!=GLOBAL_REQUEST_BOUND:
            raise Rejected('ledger needs frozen v2 full-context reservation protocol')
        self.ledger,self.task,self._key,self.transport=ledger,task_id,api_key,transport
        self.checkpoint=checkpoint
        self.telemetry=None # owner main enables compact logs; synthetic qualification stays quiet

    def halt(self):
        with self.ledger.transaction(): self.ledger.db.execute("UPDATE config SET v='true' WHERE k='halt'")

    def forward(self,raw,begin,emit):
        """begin(content_type), emit(bytes). Caller must close connection on failure.
        Exceptions/errors never release an uncertain request reservation.
        """
        value,body,input_bound,bound=validate_request(raw)
        request='gateway:'+uuid.uuid4().hex
        self.ledger.reserve(request,self.task,bound,global_bound=GLOBAL_REQUEST_BOUND) # durable BEFORE any transport call
        if self.checkpoint is not None:
            try: self.checkpoint('reserved',request,self.ledger.db.serialize())
            except BaseException:
                self.halt() # keep full pending hold; absolutely no upstream POST
                raise
        started=time.monotonic(); actual=None; observed=None; usage=None; done=False
        def examine(obj):
            nonlocal usage,observed
            if type(obj) is not dict or obj.get('model')!=MODEL:
                self.halt(); raise Rejected('upstream model identity changed')
            choices=obj.get('choices')
            if type(choices) is not list or len(choices)>1 or any(type(c) is not dict or c.get('index')!=0 for c in choices):
                self.halt(); raise Rejected('unexpected upstream choices')
            if obj.get('usage') is not None:
                if usage is not None: raise Rejected('duplicate usage receipt')
                observed=usage_cost(obj['usage'])
                # Valid provider observations remain evidence even when they
                # violate the frozen policy. Halting/unknown holds stay intact.
                usage=obj['usage']
                if observed[0]>MAX_CONTEXT or observed[1]>value['max_tokens']:
                    self.halt(); raise Rejected('provider exceeded frozen token bound')
        try:
            with self.transport(body,self._key) as response:
                if response.status!=200: raise UpstreamFailure('upstream refused request')
                if response.getheader('Content-Encoding','identity')!='identity': raise UpstreamFailure('encoded upstream response forbidden')
                content_type=response.getheader('Content-Type','').split(';')[0].strip().lower()
                expected='text/event-stream' if value.get('stream') else 'application/json'
                if content_type!=expected: raise UpstreamFailure('unexpected upstream content type')
                begin(expected)
                if value.get('stream'):
                    total=0; event=[]; event_bytes=0
                    while True:
                        if time.monotonic()-started>WALL_SECONDS: raise UpstreamFailure('upstream wall limit')
                        line=response.readline(MAX_LINE+1)
                        if len(line)>MAX_LINE: raise UpstreamFailure('upstream line too large')
                        total+=len(line)
                        if total>MAX_RESPONSE: raise UpstreamFailure('upstream response too large')
                        if not line: break
                        if line in (b'\n',b'\r\n'):
                            if event:
                                payload=b'\n'.join(event)
                                if payload==b'[DONE]':
                                    done=True; emit(b'data: [DONE]\n\n'); break
                                obj=strict_json(payload); examine(obj)
                                emit(b'data: '+payload+b'\n\n')
                            event=[]; event_bytes=0
                        elif line.startswith(b'data:'):
                            fragment=line[5:].lstrip(b' ').rstrip(b'\r\n')
                            event_bytes+=len(fragment)
                            if event_bytes>MAX_LINE: raise UpstreamFailure('upstream event too large')
                            event.append(fragment)
                        elif line.startswith(b':'): pass # heartbeat, bounded but not forwarded
                        else: raise UpstreamFailure('unsupported SSE field')
                    if not done: raise UpstreamFailure('incomplete SSE stream')
                else:
                    chunks=[]; total=0
                    while True:
                        if time.monotonic()-started>WALL_SECONDS: raise UpstreamFailure('upstream wall limit')
                        chunk=response.read(min(65536,MAX_RESPONSE-total+1))
                        if not chunk: break
                        total+=len(chunk)
                        if total>MAX_RESPONSE: raise UpstreamFailure('upstream response too large')
                        chunks.append(chunk)
                    payload=b''.join(chunks); examine(strict_json(payload)); done=True; emit(payload)
                if done and usage is not None: actual=observed[2]
        except BaseException:
            # A disconnected provider may still be generating. Stop the program;
            # do not create a second potentially in-flight upstream request.
            self.halt()
            raise
        finally:
            # All failures/disconnects retain full reservation, even after usage.
            # Actual overrun must remain visible instead of hiding it in the hold.
            if observed is not None and observed[2]>bound:
                actual=observed[2]
            try:
                self.ledger.settle(request,actual,reported_usage=usage)
            finally:
                if self.checkpoint is not None:
                    try: self.checkpoint('settled',request,self.ledger.db.serialize())
                    except BaseException:
                        # Remote retains last acknowledged full reservation. Retain
                        # it locally too and prevent another request after sync loss.
                        with self.ledger.transaction():
                            self.ledger.db.execute("UPDATE requests SET held=MAX(held,?),global_held=MAX(global_held,?),actual=NULL,status='unknown' WHERE id=?",(bound,GLOBAL_REQUEST_BOUND,request))
                            self.ledger.db.execute("UPDATE config SET v='true' WHERE k='halt'")
                        raise
        tokens=usage_tokens(usage); known=tokens['prompt_cache_hit_tokens'] is not None and tokens['prompt_cache_miss_tokens'] is not None
        report=dict(request_id=request,reserved_uusd=GLOBAL_REQUEST_BOUND,task_estimated_reserved_uusd=bound,accounted_uusd=actual if actual is not None else GLOBAL_REQUEST_BOUND,
                    usage_known=actual is not None,usage_tokens=tokens,
                    request_cache_hit_percent=100*tokens['prompt_cache_hit_tokens']/tokens['prompt_tokens'] if known else None,
                    cumulative_cache=self.ledger.cache_summary(),run_cache=self.ledger.cache_summary(self.task))
        if self.telemetry is not None:
            try:
                self.telemetry(dict(v=1,event='provider_usage',request_id=request,run_id=self.task,provider='deepseek',model=MODEL,
                    **tokens,request_cache_hit_percent=report['request_cache_hit_percent'],cumulative_cache=report['cumulative_cache'],
                    usage_known=actual is not None,estimated_usd=actual/1000000 if actual is not None else None,
                    accounted_upper_bound_uusd=report['accounted_uusd']))
            except OSError: pass # an unavailable log sink never changes durable accounting
        return report


def handler(gateway,token):
    class Handler(http.server.BaseHTTPRequestHandler):
        protocol_version='HTTP/1.0'
        def log_message(self,*args): pass # never record headers/prompts/credentials
        def setup(self):
            super().setup(); self.connection.settimeout(READ_TIMEOUT)
        def do_POST(self):
            started=False
            def begin(content_type):
                nonlocal started
                self.send_response(200); self.send_header('Content-Type',content_type)
                self.send_header('Connection','close'); self.end_headers(); started=True
            def emit(data): self.wfile.write(data); self.wfile.flush()
            try:
                if self.path not in (PATH,'/v1'+PATH): raise Rejected('unsupported route')
                if not hmac.compare_digest(self.headers.get('Authorization',''),'Bearer '+token): raise Rejected('unauthorized gateway client')
                if self.headers.get('Transfer-Encoding') or self.headers.get('Content-Encoding'): raise Rejected('encoded request forbidden')
                lengths=self.headers.get_all('Content-Length',[])
                if len(lengths)!=1 or not lengths[0].isdigit(): raise Rejected('one content length required')
                size=int(lengths[0]); count(size,1,MAX_REQUEST)
                if self.headers.get('Content-Type','').split(';')[0]!='application/json': raise Rejected('JSON required')
                raw=self.rfile.read(size)
                if len(raw)!=size: raise Rejected('short request')
                gateway.forward(raw,begin,emit)
            except Exception:
                if not started:
                    payload=b'{"error":{"message":"benchmark gateway refused request; inspect owner ledger","type":"benchmark_guard"}}'
                    self.send_response(502); self.send_header('Content-Type','application/json')
                    self.send_header('Content-Length',str(len(payload))); self.end_headers()
                    try: self.wfile.write(payload)
                    except OSError: pass
            finally: self.close_connection=True
    return Handler


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--ledger',required=True); p.add_argument('--task-id',required=True)
    p.add_argument('--port',required=True,type=int)
    p.add_argument('--pricing-checked',required=True,choices=[PRICING_CHECKED])
    p.add_argument('--checkpoint-adapter',help='owner-only Python adapter exposing build_checkpoint(ledger_path); synchronous durable acknowledgment')
    args=p.parse_args()
    if not 1024<=args.port<=65535: p.error('unprivileged port required')
    # Credentials only from environment. Never argv, config, stdout, or exceptions.
    key=os.environ.get('DEEPSEEK_API_KEY',''); token=os.environ.get('INIT_AB_GATEWAY_TOKEN','')
    if len(token)<32: p.error('runtime gateway token is required')
    ledger=Ledger(args.ledger)
    checkpoint=None
    if args.checkpoint_adapter:
        spec=importlib.util.spec_from_file_location('owner_ab_checkpoint',args.checkpoint_adapter)
        if spec is None or spec.loader is None: p.error('invalid owner checkpoint adapter')
        module=importlib.util.module_from_spec(spec); spec.loader.exec_module(module)
        checkpoint=module.build_checkpoint(args.ledger)
        if not callable(checkpoint): p.error('checkpoint adapter did not return a callback')
    gateway=Gateway(ledger,args.task_id,key,checkpoint=checkpoint)
    gateway.telemetry=lambda record: print('INIT_AB_USAGE '+json.dumps(record,sort_keys=True,separators=(',',':'),allow_nan=False),flush=True)
    server=http.server.HTTPServer(('127.0.0.1',args.port),handler(gateway,token))
    try: server.serve_forever()
    finally: server.server_close(); ledger.db.close()

if __name__=='__main__': main()
