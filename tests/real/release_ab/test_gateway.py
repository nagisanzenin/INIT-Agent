import contextlib
import io
import json
from pathlib import Path
import tempfile
import sqlite3
import unittest
from gateway import Gateway, Rejected, UpstreamFailure, validate_request, usage_cost, MAX_OUTPUT, GLOBAL_REQUEST_BOUND
from harness import Ledger, plan

BASE=Path(__file__).resolve().parents[3]/'target/test-homes/release-ab'

def request(stream=False,**extra):
    return json.dumps(dict(model='deepseek-flash',messages=[dict(role='user',content='hello')],max_tokens=64,stream=stream,**extra)).encode()

def chunk(usage=None):
    x=dict(model='deepseek-flash',choices=[dict(index=0,delta=dict(content='ok'),finish_reason='stop')])
    if usage is not None: x['usage']=usage
    return x

def usage(prompt=10,output=3):
    return dict(prompt_tokens=prompt,completion_tokens=output,total_tokens=prompt+output,
                prompt_cache_hit_tokens=0,prompt_cache_miss_tokens=prompt,
                completion_tokens_details=dict(reasoning_tokens=1))

class Response(io.BytesIO):
    def __init__(self,data,stream=False,status=200):
        super().__init__(data); self.status=status; self.stream=stream
    def getheader(self,name,default=None):
        return {'Content-Type':'text/event-stream' if self.stream else 'application/json'}.get(name,default)

class GatewayTests(unittest.TestCase):
    def setUp(self):
        BASE.mkdir(parents=True,exist_ok=True)
        self.tmp=tempfile.TemporaryDirectory(dir=BASE); self.addCleanup(self.tmp.cleanup)
        self.ledger=Ledger(Path(self.tmp.name)/'ledger.sqlite'); self.addCleanup(self.ledger.db.close)
        self.ledger.initialize(plan(),10000000)
        self.task=plan()['tasks'][0]['id']; self.calls=0
    def gateway(self,response,check=None):
        @contextlib.contextmanager
        def transport(body,key):
            self.calls+=1
            self.assertEqual(self.ledger.summary()['unresolved'],1,'reservation committed before POST')
            self.assertEqual(key,'synthetic-key')
            if check: check(body)
            yield response
        return Gateway(self.ledger,self.task,'synthetic-key',transport)
    def forward(self,gateway,raw=None,emit=None):
        out=[]
        report=gateway.forward(raw or request(),lambda ct:None,emit or out.append)
        return report,b''.join(out)
    def test_full_context_global_reservation_fits_despite_smaller_task_cap(self):
        gateway=self.gateway(Response(json.dumps(chunk(usage())).encode()))
        seen=[]
        gateway.checkpoint=lambda event,request_id,raw: seen.append((event,self.ledger.summary()['accounted_uusd']))
        report,_=self.forward(gateway)
        self.assertEqual(seen[0],('reserved',GLOBAL_REQUEST_BOUND))
        self.assertGreater(GLOBAL_REQUEST_BOUND,plan()['tasks'][0]['cap_uusd'])
        self.assertEqual(report['reserved_uusd'],319488)
        self.assertEqual(self.ledger.summary()['accounted_uusd'],7)
    def test_unknown_usage_cannot_release_full_context_global_holds(self):
        for _ in range(31):
            self.forward(self.gateway(Response(json.dumps(chunk()).encode())))
        self.assertEqual(self.ledger.summary()['accounted_uusd'],31*GLOBAL_REQUEST_BOUND)
        gateway=self.gateway(Response(json.dumps(chunk()).encode()))
        self.assertRaises(ValueError,self.forward,gateway)
        self.assertEqual(self.calls,31,'32nd request must be refused before transport')
    def test_remote_checkpoint_ack_precedes_upstream_and_records_settlement(self):
        events=[]
        gateway=self.gateway(Response(json.dumps(chunk(usage())).encode()))
        def checkpoint(event,request_id,serialized):
            clone=sqlite3.connect(':memory:'); clone.deserialize(serialized)
            row=clone.execute('SELECT status,held FROM requests WHERE id=?',(request_id,)).fetchone(); clone.close()
            events.append((event,row,self.calls))
        gateway.checkpoint=checkpoint
        self.forward(gateway)
        self.assertEqual(events[0][0],'reserved'); self.assertEqual(events[0][1][0],'pending'); self.assertEqual(events[0][2],0)
        self.assertEqual(events[1][0],'settled'); self.assertEqual(events[1][1],('settled',7))
    def test_actual_cache_tokens_are_durable_in_same_settlement_checkpoint(self):
        reported=usage(prompt=1000); reported.update(prompt_cache_hit_tokens=999,prompt_cache_miss_tokens=1)
        gateway=self.gateway(Response(json.dumps(chunk(reported)).encode())); observed=[]
        def checkpoint(event,request_id,serialized):
            db=sqlite3.connect(':memory:'); db.deserialize(serialized)
            observed.append((event,db.execute('SELECT prompt_tokens,completion_tokens,prompt_cache_hit_tokens,prompt_cache_miss_tokens FROM requests WHERE id=?',(request_id,)).fetchone(),self.calls)); db.close()
        gateway.checkpoint=checkpoint; result,_=self.forward(gateway)
        self.assertEqual(observed[0],('reserved',(None,None,None,None),0))
        self.assertEqual(observed[1],('settled',(1000,3,999,1),1))
        self.assertEqual(result['request_cache_hit_percent'],99.9)
    def test_missing_or_partial_cache_usage_is_not_reported_as_zero_hit(self):
        for field in ('prompt_cache_hit_tokens','prompt_cache_miss_tokens'):
            reported=usage(prompt=100); reported.pop(field)
            result,_=self.forward(self.gateway(Response(json.dumps(chunk(reported)).encode())))
            self.assertIsNone(result['request_cache_hit_percent'])
            self.assertIsNone(result['usage_tokens'][field])
            self.assertEqual(result['accounted_uusd'],34,'unknown cache pricing conservatively uses all uncached prompt tokens')
    def test_live_telemetry_uses_weighted_totals_and_never_logs_prompt_or_key(self):
        logs=[]
        for prompt,hit,miss in [(100,99,1),(900,0,900)]:
            reported=usage(prompt=prompt); reported.update(prompt_cache_hit_tokens=hit,prompt_cache_miss_tokens=miss)
            gateway=self.gateway(Response(json.dumps(chunk(reported)).encode())); gateway.telemetry=logs.append
            self.forward(gateway)
        self.assertEqual(logs[0]['request_cache_hit_percent'],99)
        self.assertAlmostEqual(logs[-1]['cumulative_cache']['cache_hit_percent'],9.9)
        encoded=json.dumps(logs); self.assertNotIn('synthetic-key',encoded); self.assertNotIn('hello',encoded)
    def test_failed_reservation_checkpoint_never_posts_and_keeps_pending(self):
        gateway=self.gateway(Response(json.dumps(chunk(usage())).encode()))
        def fail(*args): raise OSError('synthetic persistence failure')
        gateway.checkpoint=fail
        self.assertRaises(OSError,self.forward,gateway)
        self.assertEqual(self.calls,0); self.assertEqual(self.ledger.summary()['unresolved'],1)
        self.assertTrue(self.ledger.summary()['halted'])
    def test_failed_settlement_checkpoint_retains_full_and_halts(self):
        gateway=self.gateway(Response(json.dumps(chunk(usage())).encode()))
        def checkpoint(event,*args):
            if event=='settled': raise OSError('synthetic persistence failure')
        gateway.checkpoint=checkpoint
        self.assertRaises(OSError,self.forward,gateway)
        self.assertEqual(self.calls,1); self.assertTrue(self.ledger.summary()['halted'])
        self.assertEqual(self.ledger.summary()['accounted_uusd'],GLOBAL_REQUEST_BOUND)
    def test_valid_nonstream_and_reasoning_counted_once(self):
        result=self.gateway(Response(json.dumps(chunk(usage())).encode()))
        report,out=self.forward(result)
        self.assertEqual(self.calls,1); self.assertTrue(report['usage_known'])
        self.assertEqual(report['accounted_uusd'],7)
        self.assertEqual(json.loads(out)['model'],'deepseek-flash')
    def test_streaming_and_usage_receipt(self):
        stream=b'data: '+json.dumps(chunk()).encode()+b'\n\n'+b'data: '+json.dumps(chunk(usage())).encode()+b'\n\ndata: [DONE]\n\n'
        gateway=self.gateway(Response(stream,True),lambda b:self.assertTrue(json.loads(b)['stream_options']['include_usage']))
        report,out=self.forward(gateway,request(True))
        self.assertTrue(report['usage_known']); self.assertTrue(out.endswith(b'data: [DONE]\n\n'))
    def test_missing_usage_retains_full_never_zero(self):
        report,_=self.forward(self.gateway(Response(json.dumps(chunk()).encode())))
        self.assertFalse(report['usage_known']); self.assertGreater(report['accounted_uusd'],0)
        self.assertEqual(report['reserved_uusd'],report['accounted_uusd'])
        self.assertEqual(self.ledger.summary()['missing_usage'],1)
    def test_abort_after_usage_retains_full(self):
        gateway=self.gateway(Response(json.dumps(chunk(usage())).encode()))
        def abort(_): raise BrokenPipeError('client left')
        self.assertRaises(BrokenPipeError,self.forward,gateway,None,abort)
        self.assertEqual(self.ledger.summary()['missing_usage'],1)
        self.assertTrue(self.ledger.summary()['halted'])
        self.assertEqual(self.ledger.summary()['accounted_uusd'],GLOBAL_REQUEST_BOUND)
        self.assertEqual(self.ledger.db.execute('SELECT status,prompt_tokens,completion_tokens,prompt_cache_hit_tokens,prompt_cache_miss_tokens FROM requests').fetchone(),('unknown',10,3,0,10),
                         'observed provider tokens remain distinct from uncertain final billing')
    def test_redirect_never_followed(self):
        gateway=self.gateway(Response(b'',status=302))
        self.assertRaises(UpstreamFailure,self.forward,gateway)
        self.assertEqual(self.calls,1); self.assertEqual(self.ledger.summary()['missing_usage'],1)
    def test_pending_crash_blocks_restart_and_no_network(self):
        self.ledger.reserve('crashed',self.task,1000)
        gateway=self.gateway(Response(json.dumps(chunk(usage())).encode()))
        self.assertRaises(ValueError,self.forward,gateway)
        self.assertEqual(self.calls,0)
    def test_provider_bound_violation_halts(self):
        gateway=self.gateway(Response(json.dumps(chunk(usage(output=65))).encode()))
        acknowledged=[]
        def checkpoint(event,request_id,serialized):
            db=sqlite3.connect(':memory:');db.deserialize(serialized)
            acknowledged.append((event,db.execute('SELECT status,global_held,prompt_tokens,completion_tokens,prompt_cache_hit_tokens,prompt_cache_miss_tokens FROM requests WHERE id=?',(request_id,)).fetchone()))
            db.close()
        gateway.checkpoint=checkpoint
        self.assertRaises(Rejected,self.forward,gateway)
        self.assertTrue(self.ledger.summary()['halted'])
        expected=('unknown',GLOBAL_REQUEST_BOUND,10,65,0,10)
        self.assertEqual(acknowledged[-1],('settled',expected),
                         'valid observed tokens survive a policy rejection in the durable checkpoint; full hold remains')
        self.assertEqual(self.ledger.db.execute('SELECT status,global_held,prompt_tokens,completion_tokens,prompt_cache_hit_tokens,prompt_cache_miss_tokens FROM requests').fetchone(),expected)
    def test_accounting_overrun_records_real_larger_value(self):
        gateway=self.gateway(Response(json.dumps(chunk(usage(output=200000))).encode()))
        self.assertRaises(ValueError,self.forward,gateway)
        self.assertTrue(self.ledger.summary()['halted'])
        self.assertGreater(self.ledger.summary()['accounted_uusd'],200000)
    def test_truncated_stream_retains_full(self):
        stream=b'data: '+json.dumps(chunk(usage())).encode()+b'\n\n'
        gateway=self.gateway(Response(stream,True))
        self.assertRaises(UpstreamFailure,self.forward,gateway,request(True))
        self.assertEqual(self.ledger.summary()['missing_usage'],1)
    def test_unknown_parameters_images_multioutput_and_bad_caps_refused_before_network(self):
        base=json.loads(request())
        modifications=[{'n':2},{'model':'other'},{'max_tokens':MAX_OUTPUT+1},{'max_tokens':None},{'max_tokens':True},
                       {'web_search_options':{}},{'messages':[{'role':'user','content':[{'type':'image_url','image_url':{'url':'https://bad'}}]}]},
                       {'tools':[{'type':'web_search'}]}, {'messages':[{'role':'system','content':'x','prefix':True}]}]
        for change in modifications:
            with self.subTest(change=change): self.assertRaises(Rejected,validate_request,json.dumps(dict(base,**change)).encode())
        self.assertRaises(Rejected,validate_request,b'{"model":"deepseek-flash","model":"other"}')
        self.assertEqual(self.ledger.summary()['accounted_uusd'],0)
    def test_function_tools_and_text_history(self):
        raw=request(tools=[{'type':'function','function':{'name':'read','parameters':{'type':'object','properties':{'path':{'type':'string'}}}}}],tool_choice='auto')
        validate_request(raw)
    def test_inconsistent_usage_rejected(self):
        bad=usage(); bad['total_tokens']=0
        self.assertRaises(Rejected,usage_cost,bad)
        bad=usage(); bad['completion_tokens_details']['reasoning_tokens']=4
        self.assertRaises(Rejected,usage_cost,bad)

if __name__=='__main__': unittest.main()
