"""Small inference-only dependency surface: ONNX Runtime, tokenizers, NumPy."""
import hashlib
import json
from pathlib import Path
import numpy as np
import onnxruntime as ort
from tokenizers import Tokenizer

class SemanticModel:
    def __init__(self, path, threads=2):
        path = Path(path)
        self.cfg = json.loads((path / "semantic_model.json").read_text(encoding="utf-8"))
        if self.cfg["schema_version"] != 1 or self.cfg["kind"] != "minilm_multitask":
            raise ValueError("unsupported model schema")
        self.labels = self.cfg["labels"]
        if self.labels != ["code_generation", "code_understanding", "technical_design", "analytical_reasoning", "writing", "factual_lookup", "general"]:
            raise ValueError("invalid label map")
        if hashlib.sha256((path / "model.onnx").read_bytes()).hexdigest() != self.cfg["onnx_sha256"]:
            raise ValueError("model checksum mismatch")
        if any(not np.isfinite(self.cfg[k]) or self.cfg[k] <= 0 for k in ["type_temperature", "effort_temperature"]):
            raise ValueError("invalid model temperatures")
        if self.cfg["max_tokens"] != 256:
            raise ValueError("invalid token budget")
        self.tok = Tokenizer.from_file(str(path / "tokenizer.json"))
        # Never silently truncate the current query. Drop oldest context instead.
        self.tok.enable_truncation(max_length=self.cfg["max_tokens"], strategy="only_second", direction="left")
        options = ort.SessionOptions()
        options.intra_op_num_threads = threads
        options.inter_op_num_threads = 1
        options.execution_mode = ort.ExecutionMode.ORT_SEQUENTIAL
        self.session = ort.InferenceSession(str(path / "model.onnx"), options, providers=["CPUExecutionProvider"])

    def predict(self, state, questions=None):
        encoded = self.tok.encode(state["query"], state.get("context") or "")
        inputs = {"input_ids": np.array([encoded.ids], dtype=np.int64), "attention_mask": np.array([encoded.attention_mask], dtype=np.int64), "token_type_ids": np.array([encoded.type_ids], dtype=np.int64)}
        types, efforts = self.session.run(None, inputs)
        def probs(logits, temperature):
            z = np.asarray(logits[0], dtype=np.float64) / temperature
            p = np.exp(z - z.max())
            p /= p.sum()
            return p.tolist()
        return {"answers": {
            "request_type": {"probabilities": dict(zip(self.labels, probs(types, self.cfg["type_temperature"])))},
            "complexity": {"probabilities": {str(i): p for i, p in enumerate(probs(efforts, self.cfg["effort_temperature"]))}}
        }}
