import unittest
from readiness import check

class ReadinessTests(unittest.TestCase):
    def test_default_fails_with_exact_runtime_and_freeze_gaps(self):
        report=check()
        self.assertFalse(report['ready'])
        text=' '.join(report['blockers'])
        self.assertIn('65,536',text); self.assertIn('16,000',text)
        self.assertIn('evolution_freeze',text); self.assertIn('platform',text)
        self.assertEqual(report['preflight_api_calls'],0)
    def test_boolean_claims_cannot_replace_qualification_receipts(self):
        report=check(dict(manifest=True,runtime_output=True,evolution_freeze=True,platform=True,
                          gateway_route=True,billing=True))
        self.assertFalse(report['ready'])
    def test_nonexistent_receipts_fail(self):
        report=check(dict(runtime_output=dict(path='/nonexistent/receipt',sha256='a'*64)))
        self.assertFalse(report['ready'])
        self.assertTrue(any('runtime_output: invalid' in line for line in report['blockers']))

if __name__=='__main__': unittest.main()
