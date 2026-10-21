import importlib
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from fastapi.testclient import TestClient
import httpx


class GatewayTests(unittest.TestCase):
    def setUp(self):
        with patch.dict(os.environ,{'AGENTKV_PROVIDER_KEY':'test-only-controller-key'}):
            self.gateway=importlib.import_module('agentkv.gateway')
        self.tmp=tempfile.TemporaryDirectory()
        self.old_directory=self.gateway.DIRECTORY
        self.gateway.DIRECTORY=Path(self.tmp.name)
        self.headers={'Authorization':'Bearer '+self.gateway.TOKEN}
        self.client=TestClient(self.gateway.app)

    def tearDown(self):
        self.gateway.DIRECTORY=self.old_directory
        self.tmp.cleanup()

    def test_private_observability_and_mutations_require_authentication(self):
        for url in ['/engine-state','/engine-metrics','/health']:
            self.assertEqual(self.client.get(url).status_code,401)
        self.assertEqual(self.client.post('/cache-hints',json={'enabled':True,'hints':[]}).status_code,401)
        self.assertFalse(list(Path(self.tmp.name).iterdir()))

    def test_leases_have_a_hard_bound_and_atomic_disable(self):
        hint={'request_id':'r','score':1,'lease_seconds':31}
        self.assertEqual(self.client.post('/cache-hints',headers=self.headers,json={'enabled':True,'hints':[hint]}).status_code,422)
        hint['lease_seconds']=15
        response=self.client.post('/cache-hints',headers=self.headers,json={'enabled':True,'hints':[hint]})
        self.assertEqual(response.json()['status'],'proposed')
        self.assertEqual(self.client.post('/cache-hints',headers=self.headers,json={'enabled':False,'hints':[]}).status_code,200)
        import json
        content=json.loads(Path(self.tmp.name,'hints.json').read_text())
        self.assertEqual(content,{'enabled':False,'hints':[]})

    def test_caller_cannot_select_another_cache_namespace(self):
        captured={}
        class Upstream:
            async def post(self,path,**kwargs):
                captured.update(kwargs['json'])
                return httpx.Response(200,json={'ok':True})
        self.gateway.app.state.upstream=Upstream()
        response=self.client.post('/v1/chat/completions',headers=self.headers,
            json={'messages':[],'cache_salt':'other-owner'})
        self.assertEqual(response.status_code,200)
        self.assertEqual(captured['cache_salt'],self.gateway.NAMESPACE)
        self.assertNotEqual(captured['cache_salt'],'other-owner')

    def test_request_usage_indexes_client_id_and_engine_id(self):
        self.gateway.REQUEST_USAGE.clear()
        class Upstream:
            async def post(self,path,**kwargs):
                return httpx.Response(200,json={
                    'id':'cmpl-engine',
                    'usage':{'prompt_tokens':10,'completion_tokens':2,
                             'prompt_tokens_details':{'cached_tokens':8}},
                    'choices':[{'message':{'content':'ok'}}]})
        self.gateway.app.state.upstream=Upstream()
        response=self.client.post('/v1/chat/completions',headers=self.headers,
            json={'messages':[],'request_id':'agentkv-client'})
        self.assertEqual(response.status_code,200)
        for key in ('cmpl-engine','agentkv-client'):
            found=self.client.get('/request-usage/'+key,headers=self.headers)
            self.assertEqual(found.status_code,200,key)
            self.assertEqual(found.json()['usage']['prompt_tokens'],10)
