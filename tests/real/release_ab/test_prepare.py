from datetime import datetime, timezone, timedelta
import unittest
from prepare_pilot import available_budget


class PrepareTests(unittest.TestCase):
    def observations(self):
        now=datetime.now(timezone.utc)
        balance=dict(checked_at=now.isoformat(),provider='deepseek',is_available=True,
                     balance_infos=[dict(currency='USD',total_balance='12.19')])
        pricing=dict(checked_at=now.isoformat(),source='https://api-docs.deepseek.com/quick_start/pricing/',
            model='deepseek-flash',context_reservation_tokens=1048576,request_output_cap=4096,
            server_tools_allowed=False,input_uncached_usd_per_million='0.30',
            input_cached_usd_per_million='0.006',output_usd_per_million='1.20',global_reservation_usd='0.319488')
        return balance,pricing,now
    def test_cap_preserves_authorized_ten_dollars_or_lower_balance(self):
        balance,pricing,now=self.observations()
        self.assertEqual(available_budget(balance,pricing,now),10000000)
        balance['balance_infos'][0]['total_balance']='1.230001'
        self.assertEqual(available_budget(balance,pricing,now),1230001)
    def test_stale_actual_balance_and_changed_provider_rates_refuse_setup(self):
        balance,pricing,now=self.observations()
        balance['checked_at']=(now-timedelta(days=2)).isoformat()
        self.assertRaises(ValueError,available_budget,balance,pricing,now)
        balance['checked_at']=now.isoformat(); pricing['output_usd_per_million']='1.21'
        self.assertRaises(ValueError,available_budget,balance,pricing,now)
    def test_insufficient_or_nonfinite_balance_cannot_initialize_ledger(self):
        balance,pricing,now=self.observations()
        for amount in ('0.01','NaN','Infinity','-1'):
            balance['balance_infos'][0]['total_balance']=amount
            self.assertRaises(ValueError,available_budget,balance,pricing,now)


if __name__=='__main__': unittest.main()
