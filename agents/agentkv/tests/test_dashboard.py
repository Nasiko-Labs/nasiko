import json
import tempfile
import unittest
from pathlib import Path
from fastapi.testclient import TestClient
from agentkv.dashboard import create_dashboard, summarize


class DashboardTests(unittest.TestCase):
    def test_empty_dashboard_never_invents_results(self):
        with tempfile.TemporaryDirectory() as directory:
            with TestClient(create_dashboard(directory+'/missing.json')) as client:
                self.assertEqual(client.get('/').status_code,200)
                data=client.get('/api/report').json()
                self.assertEqual(data['status'],'awaiting_benchmark')
                self.assertEqual(data['rows'],[])

    def test_failures_are_counted_and_unknown_cost_is_preserved(self):
        report={'records':[{'task_id':'x','method':'A','repetition':0,'success':False,'seconds':2,'secret':'must-not-leak'}],
                'trials':[{'method':'A','repetition':0,'concurrency':1,'seconds':3}]}
        data=summarize(report)
        self.assertEqual(data['rows'][0]['failed'],1)
        self.assertIsNone(data['rows'][0]['cost_per_thousand'])
        self.assertNotIn('secret',data['records'][0])

    def test_cache_budgets_remain_separate_cohorts(self):
        data=summarize({'cohorts':[
            {'label':'Natural','report':{'manifest':{'kv_cache_bytes_override':0}}},
            {'label':'Pressure','report':{'manifest':{'kv_cache_bytes_override':1073741824}}}]})
        self.assertEqual([c['report']['cache_budget_bytes'] for c in data['cohorts']],[0,1073741824])
        self.assertEqual(data['rows'],[])
