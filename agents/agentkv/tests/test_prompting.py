import unittest
from agentkv.prompting import compile_prompt


TASK = {
    "id": "clamp",
    "goal": "Clamp value.",
    "context": "def clamp(value, low, high):\n    return value",
    "source": "def clamp(value, low, high):\n    return value",
}


class PromptTests(unittest.TestCase):
    def test_b_keeps_the_same_information_in_a_stable_system_prefix(self):
        a = compile_prompt(TASK, "A")
        b = compile_prompt(TASK, "B")
        self.assertEqual(a[0]["role"], "system")
        self.assertIn("Clamp value.", a[1]["content"])
        self.assertIn("def clamp", a[1]["content"])
        self.assertIn("def clamp", b[0]["content"])
        self.assertIn("Clamp value.", b[1]["content"])
        self.assertNotIn("def clamp", b[1]["content"])
        self.assertEqual(compile_prompt(TASK, "C")[0]["content"], b[0]["content"])
        self.assertEqual(compile_prompt(TASK, "E")[1]["content"], b[1]["content"])

    def test_reviewer_does_not_drop_source_or_proposed_code(self):
        review = compile_prompt(TASK, "B", "reviewer", "def clamp(value, low, high):\n    return min(high, max(low, value))")
        self.assertIn("def clamp", review[0]["content"])
        self.assertIn("min(high, max(low, value))", review[1]["content"])

    def test_support_tickets_share_the_knowledge_base_prefix(self):
        ticket = {"id": "late-parcel", "kind": "support", "goal": "Where is my order?",
                  "context": "Ground delivery is 5 business days.", "label": "shipping"}
        a = compile_prompt(ticket, "A", "triage")
        b = compile_prompt(ticket, "B", "triage")
        self.assertIn("5 business days", a[1]["content"])
        self.assertIn("5 business days", b[0]["content"])
        self.assertIn("Where is my order?", b[1]["content"])
