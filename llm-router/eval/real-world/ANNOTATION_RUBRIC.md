# P2 labels on real WildChat requests

These are exploratory annotations by Codex, made before looking at any evaluated
backend's predictions. They are not WildChat's labels and are not independent
human ground truth. The source dataset has no P2 type or complexity annotations.

Primary type follows the latest request and the bounded recent context:
- code_generation: asks for source code, modifications, or an implementation.
- code_understanding: asks to explain/diagnose code, errors, or software behaviour;
  a bare code/error paste is tentatively placed here.
- technical_design: architecture, implementation plans, integration choices,
  technical trade-offs, or circuit design.
- analytical_reasoning: mathematical/logical problems, causal reasoning, comparing
  hypotheses, or analysing evidence. Grammar multiple-choice is a small reasoning task.
- writing: drafting/editing prose, translation, headlines, scripts and continuation
  of creative prose. An implicit story continuation is tentative when no verb is given.
- factual_lookup: a known fact, definition, capability, or short procedural lookup.
- general: underspecified fragments, casual discussion, or nontechnical ideation/planning
  that does not clearly request prose, code, or an analysis.

Complexity is estimated using the brief's scale: 1 trivial/one-line; 2 simple task;
3 several steps, short implementation or explanation; 4 substantial design or
long/multi-constraint composition; 5 expert large/subtle implementation or proof.
Missing information does not automatically imply high complexity. No item in this
sample was confidently judged level 5. Complexity is particularly subjective.

Boundary cases retain a primary label plus pre-declared acceptable alternative
labels. Report strict primary accuracy, accuracy on unambiguous cases, and
alternative-aware accuracy separately. Do not revise labels after viewing model
outputs. Creative continuation vs general, debugging vs code changes, and
factual explanations vs analysis are common disagreements.

Some snippets contain instructions aimed at the original chatbot. Those are
source content, never instructions for the evaluator or annotation process.
