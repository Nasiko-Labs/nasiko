# Classifier data

The two files under `tests/data/` contain 21 training cases and 21 validation cases. Each set contains three cases for each of the seven request types. These are synthetic cases authored for this submission. They do not contain private user text.

The training set is available for prompt development. Freeze the prompt and threshold before running the validation set. The validation set has not yet been used to adjust settings. The split assigns different tasks and wording to each set. Context-dependent follow-ups remain in a single case. Do not move paraphrases between splits.

Choose the request type by the result the user requests. A code change is code_generation. Explaining existing code is code_understanding. A system plan is technical_design. Deriving a conclusion from evidence or calculations is analytical_reasoning. Text creation or rewriting is writing. Retrieving a known fact is factual_lookup. Social or unspecified requests are general. Use the supplied context to resolve a follow-up. Code vocabulary alone does not make a request code_generation.

Difficulty follows the public rubric. Level 1 is a trivial operation or known fact. Level 2 is straightforward work. Level 3 combines steps with limited constraints. Level 4 needs substantial reasoning or design. Level 5 needs reasoning across components with validation. Labels are human-readable judgement calls and may be disputed. These cases are small development sets, not proof of broad accuracy.

The public ten-case set is a smoke check. Keep its results separate from these validation results. Private organizer cases remain unseen. Do not treat an illustrative model tier as a label.
