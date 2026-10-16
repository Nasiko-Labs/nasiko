import asyncio
import tempfile
import unittest

from fastapi.testclient import TestClient
from agentkv.api import create_app
from agentkv.control import Candidate, Conflict, Controller, Event, JevEstimator, Store, prefix_identity


def event(revision=1, kind=None, **kwargs):
    return Event(event_id=f"e{revision}", flow_id="f", revision=revision,
                 kind=kind or ("started" if revision == 1 else "transition"), agent="coder", **kwargs)


def candidate(**kwargs):
    return Candidate(agent="tester", prefix_id="a" * 64, incremental_bytes=1024,
                     avoided_recompute_ms=100, **kwargs)


class RegistryTests(unittest.TestCase):
    def setUp(self):
        self.now = 100.0
        self.store = Store(clock=lambda: self.now)
        self.addCleanup(self.store.close)

    def test_redelivery_does_not_renew_and_collision_rejected(self):
        self.assertTrue(self.store.ingest("a", event()))
        self.now += 10
        self.assertFalse(self.store.ingest("a", event()))
        self.assertEqual(self.store.snapshot("a", "f")[1], 115)
        with self.assertRaises(Conflict):
            self.store.ingest("a", event(evidence="changed"))

    def test_order_terminal_and_tenant_isolation(self):
        self.store.ingest("a", event())
        self.assertIsNone(self.store.snapshot("b", "f"))
        with self.assertRaises(Conflict):
            self.store.ingest("a", event(3))
        self.store.ingest("a", event(2, "completed"))
        with self.assertRaises(Conflict):
            self.store.ingest("a", event(3))
        self.store.ingest("b", event())

    def test_restart_preserves_deduplication(self):
        with tempfile.TemporaryDirectory() as directory:
            db = Store(directory + "/events.db")
            db.ingest("a", event())
            db.close()
            db = Store(directory + "/events.db")
            self.assertFalse(db.ingest("a", event()))
            db.close()

    def test_prefix_identity_all_compatibility_fields(self):
        base = dict(model="m", tokenizer="t", template="c", repository="r", token_ids=[1, 2])
        original = prefix_identity("a", b"secret", **base)
        for key in ("model", "tokenizer", "template", "repository", "token_ids"):
            changed = {**base, key: [1, 3] if key == "token_ids" else "different"}
            self.assertNotEqual(original, prefix_identity("a", b"secret", **changed))
        self.assertNotEqual(original, prefix_identity("b", b"secret", **base))


class DecisionsTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        self.now = 100.0
        self.store = Store(clock=lambda: self.now)
        self.addCleanup(self.store.close)

    async def test_known_successor_never_calls_provider(self):
        class Forbidden:
            async def estimate(*args):
                raise AssertionError("must not be called")
        self.store.ingest("a", event(candidates=[candidate(known_next=True)]))
        result = await Controller(self.store, Forbidden()).decide("a", "f")
        self.assertEqual(result["estimator"], "deterministic")
        self.assertEqual(result["proposals"][0]["probability"], 1)
        self.assertFalse(result["engine_applied"])

    async def test_duplicate_requests_only_bill_once(self):
        class Estimator:
            calls = 0
            async def estimate(self, event, candidates):
                self.calls += 1
                await asyncio.sleep(.01)
                return {"tester": .7}, {"input_tokens": 10, "output_tokens": 2}
        estimator = Estimator()
        self.store.ingest("a", event(candidates=[candidate()]))
        controller = Controller(self.store, estimator)
        results = await asyncio.gather(*[controller.decide("a", "f") for _ in range(3)])
        self.assertEqual(estimator.calls, 1)
        self.assertEqual(results[0], results[2])
        self.now = 116
        self.assertEqual((await controller.decide("a", "f"))["proposals"], [])

    async def test_timeout_falls_back_without_exposing_error(self):
        class Slow:
            async def estimate(*args):
                await asyncio.sleep(10)
        self.store.ingest("a", event(candidates=[candidate()]))
        result = await Controller(self.store, Slow(), timeout=.01).decide("a", "f")
        self.assertEqual(result["estimator"], "deterministic_fallback")
        self.assertEqual(result["proposals"], [])

    async def test_nonfinite_probability_falls_back(self):
        class Invalid:
            async def estimate(*args):
                return {"tester": float("nan")}, {}
        self.store.ingest("a", event(candidates=[candidate()]))
        result = await Controller(self.store, Invalid()).decide("a", "f")
        self.assertEqual(result["estimator"], "deterministic_fallback")
        self.assertEqual(result["proposals"], [])

    async def test_response_after_flow_change_is_discarded(self):
        store = self.store
        class Late:
            async def estimate(*args):
                store.ingest("a", event(2, "completed"))
                return {"tester": .9}, {}
        self.store.ingest("a", event(candidates=[candidate()]))
        result = await Controller(self.store, Late()).decide("a", "f")
        self.assertEqual(result["status"], "stale")
        self.assertEqual(result["proposals"], [])


class APITests(unittest.TestCase):
    def test_auth_owner_boundary_conflict_and_history(self):
        with TestClient(create_app({"a" * 32: "alice", "b" * 32: "bob"}, Store())) as client:
            a = {"Authorization": "Bearer " + "a" * 32}
            b = {"Authorization": "Bearer " + "b" * 32}
            self.assertEqual(client.post("/events", json=event().model_dump()).status_code, 401)
            self.assertEqual(client.post("/events", headers=a, json=event().model_dump()).status_code, 200)
            self.assertEqual(client.post("/flows/f/decide", headers=b).status_code, 404)
            body = {**event().model_dump(), "owner": "bob"}
            self.assertEqual(client.post("/events", headers=a, json=body).status_code, 422)
            self.assertEqual(client.post("/flows/f/decide", headers=a).status_code, 200)
            self.assertEqual(len(client.get("/flows/f/decisions", headers=a).json()["decisions"]), 1)
            self.assertEqual(client.get("/metrics", headers=b).json()["events_accepted"], 0)
            self.assertEqual(client.get("/metrics", headers=a).json()["events_accepted"], 1)
            self.assertEqual(client.get("/metrics", headers=a).json()["engine_actions_applied"], 0)


class PlanTests(unittest.IsolatedAsyncioTestCase):
    async def test_plan_asks_two_independent_nouls(self):
        from types import SimpleNamespace
        captured = {}
        class Response:
            answers = {"repair": SimpleNamespace(noul=.25), "reuse_prefix": SimpleNamespace(noul=.8)}
            model = "jev-1.13.0"
            usage = SimpleNamespace(input_tokens=11, output_tokens=4)
        estimator = JevEstimator("k")
        async def fake(state, questions):
            captured["questions"] = questions
            return Response()
        estimator._system_one = fake
        probabilities, usage = await estimator.plan(event(evidence="goal excerpt"))
        self.assertEqual(probabilities, {"repair": .25, "reuse_prefix": .8})
        self.assertEqual(set(captured["questions"]), {"repair", "reuse_prefix"})
        self.assertEqual(usage["input_tokens"], 11)

    async def test_plan_rejects_nonfinite_answers(self):
        from types import SimpleNamespace
        class Response:
            answers = {"repair": SimpleNamespace(noul=float("nan")), "reuse_prefix": SimpleNamespace(noul=.8)}
            model = "jev-1.13.0"
            usage = SimpleNamespace(input_tokens=1, output_tokens=1)
        estimator = JevEstimator("k")
        async def fake(state, questions):
            return Response()
        estimator._system_one = fake
        with self.assertRaises(ValueError):
            await estimator.plan(event())

    async def test_system_one_caps_questions(self):
        estimator = JevEstimator("k")
        with self.assertRaises(ValueError):
            await estimator._system_one({}, {str(i): i for i in range(4)})

