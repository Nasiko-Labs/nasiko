import unittest
import httpx
from agentkv.control import Store
from agentkv.nasiko import NasikoBridge


class NasikoTests(unittest.IsolatedAsyncioTestCase):
    async def test_lifecycle_is_owner_scoped_and_idempotent(self):
        payload = {"flow": {"flow_id": "flow", "user_id": "owner", "status": "running"}, "steps": []}
        def handle(request):
            self.assertEqual(request.headers["Authorization"], "Bearer token")
            return httpx.Response(200, json=payload)
        store = Store()
        self.addCleanup(store.close)
        bridge = NasikoBridge(store, owner="owner", base_url="https://nasiko.invalid",
                              token="token", transport=httpx.MockTransport(handle))
        self.addAsyncCleanup(bridge.close)
        self.assertTrue(await bridge.sync("flow"))
        before = store.snapshot("owner", "flow")
        self.assertFalse(await bridge.sync("flow"))
        self.assertEqual(before, store.snapshot("owner", "flow"))
        payload["flow"]["status"] = "completed"
        self.assertTrue(await bridge.sync("flow"))
        self.assertEqual(store.snapshot("owner", "flow")[0].kind, "completed")
        self.assertFalse(await bridge.sync("flow"))
        self.assertIsNone(store.snapshot("someone-else", "flow"))

    async def test_wrong_owner_and_redirect_cannot_ingest_events(self):
        for status, payload in [(200, {"flow": {"flow_id": "f", "user_id": "other"}}), (302, {})]:
            store = Store()
            self.addCleanup(store.close)
            bridge = NasikoBridge(store, owner="owner", base_url="https://nasiko.invalid", token="token",
                transport=httpx.MockTransport(lambda request: httpx.Response(status, json=payload,
                    headers={"Location": "https://elsewhere.invalid"})))
            self.addAsyncCleanup(bridge.close)
            with self.assertRaises((ValueError, httpx.HTTPStatusError)):
                await bridge.sync("f")
            self.assertIsNone(store.snapshot("owner", "f"))
