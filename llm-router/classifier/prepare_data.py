"""Materialize authored splits and reject family/exact-query leakage."""
import csv
import hashlib
import json
from pathlib import Path

def main():
    root = Path(__file__).parent / "data"
    source = root / "cases.tsv"
    groups, families, queries = {}, {}, {}
    with source.open(encoding="utf-8", newline="") as f:
        for row in csv.DictReader(f, delimiter="\t"):
            split = row.pop("split")
            family = row["family"]
            normalized = " ".join(row["query"].lower().split())
            assert families.setdefault(family, split) == split, f"family leak: {family}"
            assert queries.setdefault(normalized, split) == split, f"query leak: {normalized}"
            row["id"] = f"{split}-{family}"
            row["complexity"] = int(row["complexity"])
            groups.setdefault(split, []).append(row)
    for split, examples in groups.items():
        value = {"schema_version": "nasiko-classifier-dev-v1", "purpose": "Authored synthetic development data; not evidence of private-set performance.", "source_sha256": hashlib.sha256(source.read_bytes()).hexdigest(), "examples": examples}
        (root / f"{split}.json").write_text(json.dumps(value, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(json.dumps({k: len(v) for k, v in groups.items()}))

if __name__ == "__main__":
    main()
