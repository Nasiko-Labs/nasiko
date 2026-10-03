"""Adapt MiniLM's last two layers and two heads; export one-pass INT8 CPU model.

Only train.json enters gradient updates. Validation selects the epoch and fits
temperatures. test.json is never read. All inference uses local files.
"""
import argparse
import copy
import hashlib
import json
import os
from pathlib import Path
import random
import time

os.environ["USE_TF"] = "0"
import numpy as np
import torch
from torch import nn
from transformers import AutoModel, AutoTokenizer

LABELS = ["code_generation", "code_understanding", "technical_design", "analytical_reasoning", "writing", "factual_lookup", "general"]

class Model(nn.Module):
    def __init__(self, base):
        super().__init__()
        self.encoder = AutoModel.from_pretrained(base, local_files_only=True, attn_implementation="eager")
        d = self.encoder.config.hidden_size
        self.type_head = nn.Linear(d, 7)
        self.effort_head = nn.Linear(d, 5)

    def features(self, input_ids, attention_mask, token_type_ids):
        h = self.encoder(input_ids=input_ids, attention_mask=attention_mask, token_type_ids=token_type_ids).last_hidden_state
        # Pool the query tokens after attention to context; context does not swamp
        # the task's requested action just because it is longer.
        mask = (attention_mask * (token_type_ids == 0)).unsqueeze(-1).float()
        pooled = (h * mask).sum(1) / mask.sum(1).clamp_min(1)
        return torch.nn.functional.normalize(pooled, dim=-1)

    def forward(self, input_ids, attention_mask, token_type_ids):
        h = self.features(input_ids, attention_mask, token_type_ids)
        return self.type_head(h), self.effort_head(h)

def encode(tok, cases):
    return tok([x["query"] for x in cases], [x.get("context") or "" for x in cases], padding=True, truncation="longest_first", max_length=256, return_tensors="pt")

def evaluate(model, inputs, cases):
    model.eval()
    with torch.no_grad():
        types, efforts = model(**inputs)
    truth = torch.tensor([LABELS.index(x["request_type"]) for x in cases])
    effort = torch.tensor([x["complexity"] - 1 for x in cases])
    return types, efforts, float((types.argmax(-1) == truth).float().mean()), float((efforts.argmax(-1) - effort).abs().float().mean())

def temperature(logits, targets):
    # Deterministic bounded grid, selected by NLL, not test accuracy.
    best = min(np.geomspace(.5, 5, 101), key=lambda t: float(nn.functional.cross_entropy(logits / float(t), targets)))
    return float(best)

def main():
    p = argparse.ArgumentParser()
    p.add_argument("--base", type=Path, required=True)
    p.add_argument("--output", type=Path, required=True)
    p.add_argument("--epochs", type=int, default=16)
    p.add_argument("--threads", type=int, default=2)
    p.add_argument("--seed", type=int, default=42)
    args = p.parse_args()
    if args.epochs < 0 or args.threads < 1:
        p.error("epochs must be nonnegative and threads positive")
    torch.set_num_threads(args.threads)
    torch.manual_seed(args.seed); random.seed(args.seed); np.random.seed(args.seed)
    torch.use_deterministic_algorithms(True)
    root = Path(__file__).parent
    train_path, val_path = root / "data/train.json", root / "data/validation.json"
    train = json.loads(train_path.read_text(encoding="utf-8"))["examples"]
    val = json.loads(val_path.read_text(encoding="utf-8"))["examples"]
    assert not ({x["family"] for x in train} & {x["family"] for x in val})
    tok = AutoTokenizer.from_pretrained(args.base, local_files_only=True)
    model = Model(args.base)
    for param in model.encoder.parameters(): param.requires_grad_(False)
    for layer in model.encoder.encoder.layer[-2:]:
        for param in layer.parameters(): param.requires_grad_(True)
    inputs, val_inputs = encode(tok, train), encode(tok, val)
    type_truth = torch.tensor([LABELS.index(x["request_type"]) for x in train])
    effort_truth = torch.tensor([x["complexity"] - 1 for x in train])
    if set(type_truth.tolist()) != set(range(7)) or set(effort_truth.tolist()) != set(range(5)):
        raise ValueError("training data must represent every type and complexity level")
    # Initialize with semantic class centroids rather than arbitrary logits.
    model.eval()
    with torch.no_grad():
        features = model.features(**inputs)
        for i in range(7):
            model.type_head.weight[i] = 10 * nn.functional.normalize(features[type_truth == i].mean(0), dim=0)
        model.type_head.bias.zero_()
        for i in range(5):
            model.effort_head.weight[i] = 10 * nn.functional.normalize(features[effort_truth == i].mean(0), dim=0)
        model.effort_head.bias.zero_()
    optim = torch.optim.AdamW([
        {"params": [p for p in model.encoder.parameters() if p.requires_grad], "lr": 2e-5},
        {"params": list(model.type_head.parameters()) + list(model.effort_head.parameters()), "lr": 2e-3},
    ], weight_decay=.01)
    best_score = float("-inf"); best_state = None; best_epoch = 0
    started = time.perf_counter()
    for epoch in range(args.epochs + 1):
        if epoch:
            model.train()
            order = torch.randperm(len(train))
            for indices in order.split(8):
                batch = {k: v[indices] for k, v in inputs.items()}
                types, efforts = model(**batch)
                loss = nn.functional.cross_entropy(types, type_truth[indices]) + .5 * nn.functional.cross_entropy(efforts, effort_truth[indices])
                # Ordered distribution loss: confusing 1 with 5 costs more than 1 with 2.
                cdf = efforts.softmax(-1).cumsum(-1)
                target_cdf = nn.functional.one_hot(effort_truth[indices], 5).float().cumsum(-1)
                loss = loss + .2 * ((cdf - target_cdf) ** 2).mean()
                optim.zero_grad(); loss.backward(); nn.utils.clip_grad_norm_(model.parameters(), 1); optim.step()
        types, efforts, acc, mae = evaluate(model, val_inputs, val)
        score = acc - .05 * mae
        print(json.dumps({"epoch": epoch, "validation_accuracy": acc, "complexity_mae": mae}), flush=True)
        if score > best_score:
            best_score = score; best_state = copy.deepcopy(model.state_dict()); best_epoch = epoch
    model.load_state_dict(best_state); model.eval()
    types, efforts, acc, mae = evaluate(model, val_inputs, val)
    t_type = temperature(types, torch.tensor([LABELS.index(x["request_type"]) for x in val]))
    t_effort = temperature(efforts, torch.tensor([x["complexity"] - 1 for x in val]))
    args.output.mkdir(parents=True, exist_ok=True)
    (args.output / ".gitignore").write_text("*\n", encoding="utf-8")
    tok.save_pretrained(args.output)
    from safetensors.torch import save_file
    save_file({k: v.contiguous() for k, v in best_state.items()}, str(args.output / "adapted.safetensors"))
    dummy = encode(tok, train[:1])
    fp32 = args.output / "model-fp32.onnx"
    torch.onnx.export(model, tuple(dummy[k] for k in ["input_ids", "attention_mask", "token_type_ids"]), str(fp32), input_names=["input_ids", "attention_mask", "token_type_ids"], output_names=["type_logits", "effort_logits"], dynamic_axes={k: {0: "batch", 1: "sequence"} for k in ["input_ids", "attention_mask", "token_type_ids"]}, opset_version=17, dynamo=False)
    from onnxruntime.quantization import QuantType, quantize_dynamic
    quantize_dynamic(str(fp32), str(args.output / "model.onnx"), weight_type=QuantType.QInt8, op_types_to_quantize=["MatMul"])
    config = {"schema_version": 1, "kind": "minilm_multitask", "labels": LABELS, "max_tokens": 256, "type_temperature": t_type, "effort_temperature": t_effort, "seed": args.seed, "selected_epoch": best_epoch, "validation_accuracy_fp32": acc, "complexity_mae_fp32": mae, "training_seconds": time.perf_counter() - started, "train_sha256": hashlib.sha256(train_path.read_bytes()).hexdigest(), "validation_sha256": hashlib.sha256(val_path.read_bytes()).hexdigest(), "base": json.loads((args.base / "provenance.json").read_text()), "onnx_sha256": hashlib.sha256((args.output / "model.onnx").read_bytes()).hexdigest()}
    (args.output / "semantic_model.json").write_text(json.dumps(config, indent=2), encoding="utf-8")
    print(json.dumps({k: config[k] for k in ["selected_epoch", "validation_accuracy_fp32", "type_temperature", "training_seconds"]}), flush=True)

if __name__ == "__main__":
    main()
