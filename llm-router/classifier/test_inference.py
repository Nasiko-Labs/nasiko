"""Offline artifact checks: CLASSIFIER_MODEL_PATH=... python -m unittest discover
-s llm-router/classifier -p test_inference.py
"""
import json
import os
from pathlib import Path
import tempfile
import unittest

from semantic_inference import SemanticModel


@unittest.skipUnless(os.environ.get("CLASSIFIER_MODEL_PATH"), "prepare the model first")
class InferenceTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.path = Path(os.environ["CLASSIFIER_MODEL_PATH"])
        cls.model = SemanticModel(cls.path)

    def test_variable_length_contract_and_context_budget(self):
        for query, context in [
            ("Hello", ""),
            ("Implement the fix", "Earlier we found an incorrect cache key."),
            ("Summarize the report", "irrelevant old context " * 2000),
        ]:
            result = self.model.predict({"query": query, "context": context})
            for name, count in [("request_type", 7), ("complexity", 5)]:
                p = result["answers"][name]["probabilities"]
                self.assertEqual(len(p), count)
                self.assertAlmostEqual(sum(p.values()), 1, places=6)
                self.assertTrue(all(0 <= value <= 1 for value in p.values()))

    def test_repeated_predictions_are_identical(self):
        state = {"query": "Explain this function", "context": "def inc(n): return n + 1"}
        self.assertEqual(self.model.predict(state), self.model.predict(state))

    def test_oversized_query_is_not_silently_truncated(self):
        with self.assertRaises(Exception):
            self.model.predict({"query": "word " * 1000, "context": ""})

    def test_mismatched_weights_fail_at_startup(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "semantic_model.json").write_bytes((self.path / "semantic_model.json").read_bytes())
            (root / "model.onnx").write_bytes(b"corrupted model")
            with self.assertRaisesRegex(ValueError, "checksum"):
                SemanticModel(root)

    def test_label_mapping_is_checked_before_loading(self):
        with tempfile.TemporaryDirectory() as tmp:
            cfg = json.loads((self.path / "semantic_model.json").read_text())
            cfg["labels"] = list(reversed(cfg["labels"]))
            (Path(tmp) / "semantic_model.json").write_text(json.dumps(cfg))
            with self.assertRaisesRegex(ValueError, "label map"):
                SemanticModel(tmp)


if __name__ == "__main__":
    unittest.main()
