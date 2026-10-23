import json
from pathlib import Path
import tempfile
import unittest
from aggregate_pilot import aggregate
from harness import plan, sha
from lifecycle import BASE
from readiness import check

class AggregateTests(unittest.TestCase):
    def test_pilot_missing_coverage_is_not_security_permission(self):
        report=check({'scope':'body-pilot-v2'})
        self.assertFalse(report['ready'])
        for gate in ('aggregation','durable_ledger','universal_request_guard','evolution_freeze','platform'):
            self.assertTrue(any(gate in line for line in report['blockers']))
        self.assertTrue(any('compaction' in gap for gap in report['remaining_release_gaps']))
    def test_missing_actual_logs_never_generates_passed_aggregation(self):
        BASE.mkdir(parents=True,exist_ok=True)
        with tempfile.TemporaryDirectory(dir=BASE) as root:
            root=Path(root); manifest=plan(); manifest.update(repo_sha='a'*40,binary_sha='b'*64)
            manifest['protocol_sha']=sha(manifest); (root/'manifest.json').write_text(json.dumps(manifest))
            config=dict(manifest=str(root/'manifest.json'),native=str(root/'absent-native'),evolution=str(root/'absent-evolution'),runtime_binaries={},source_archive={'path':str(root/'absent-source'),'sha256':'c'*64})
            report=aggregate(config,root/'output')
            self.assertFalse(report['ready'])
            evidence=json.loads((root/'output/evidence.json').read_text())
            aggregation=json.loads(Path(evidence['aggregation']['path']).read_text())
            self.assertEqual(aggregation['status'],'blocked')
            self.assertFalse(check(evidence)['ready'])

if __name__=='__main__': unittest.main()
