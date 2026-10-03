# Classifier development data

This small hand-labelled development set is intentionally separate from the public evaluator fixture. It supports quick local regression checks for the built-in local backend; it is not used at runtime and does not claim to predict private-evaluation performance.

Labels follow the routing categories: implementation requests are `code_generation`; explanations of supplied code are `code_understanding`; architecture and trade-off work is `technical_design`; multi-step calculations and comparisons are `analytical_reasoning`; composing or editing prose is `writing`; requests for a verifiable external fact are `factual_lookup`; and open-ended non-specialist requests are `general`.

Complexity is labelled on a five-point rubric: 1 is a single direct task, 2 has a small constraint or edit, 3 has multiple steps or supplied context, 4 requires substantial trade-offs or analysis, and 5 is broad high-risk system work. Train and validation prompts use distinct tasks and wording to avoid near-duplicate leakage.
