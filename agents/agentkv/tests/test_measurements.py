import unittest
from agentkv.measurements import delta, usage_totals


class MeasurementsTests(unittest.TestCase):
    def test_missing_or_reset_counter_is_unknown(self):
        self.assertIsNone(delta('', 'x 2','x'))
        self.assertIsNone(delta('x 4','x 2','x'))
        self.assertEqual(delta('x{worker="0"} 1\nx{worker="1"} 2','x{worker="0"} 4\nx{worker="1"} 5','x'),6)

    def test_failed_work_still_counts_tokens(self):
        result=usage_totals([{'success':False,'events':[{'usage':{'prompt_tokens':20,'completion_tokens':3,'prompt_tokens_details':{'cached_tokens':10}}}]}])
        self.assertEqual(result['cached_fraction'],.5)
        self.assertEqual(result['generated_tokens'],3)
        self.assertIsNone(usage_totals([{'events':[{'usage':{'prompt_tokens':20}}]}])['cached_fraction'])
