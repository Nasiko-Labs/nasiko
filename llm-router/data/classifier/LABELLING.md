# Classifier labelling and split policy

Each record has `query`, optional `context`, one of the seven public request types,
an ordinal complexity label from 1 through 5, and a `family` identifier. The requested
action wins over incidental nouns: changing code is `code_generation`; explaining or
auditing without a requested change is `code_understanding`; choosing a system structure
is `technical_design`; deriving or diagnosing is `analytical_reasoning`; producing prose
is `writing`; retrieving a bounded fact is `factual_lookup`; social or uncategorised text
is `general`. For multi-intent prompts, the principal deliverable determines the label.

Complexity is independent of type: 1 is a trivial/local operation, 2 is bounded and
straightforward, 3 requires several ordinary steps or constraints, 4 involves difficult
trade-offs or multi-component reasoning, and 5 requires expert reasoning under interacting
failure, risk, or ambiguity constraints. Output length alone never raises complexity.

The generator assigns all paraphrases/noisy variants of a scenario to one family. Two
fixed family indices per class form validation; all remaining families form training.
Consequently no scenario template crosses the split. The official public smoke set is not
included and was not used to choose features, thresholds, or calibration temperature.

Regenerate the JSONL split and byte-deterministic model with:

```sh
python llm-router/scripts/classifier_train.py
```
