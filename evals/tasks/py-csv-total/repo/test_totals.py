import unittest

from totals import totals


class Totals(unittest.TestCase):
    def test_sums_per_customer(self):
        text = "customer,amount\nann,1.5\nbob,2\nann,3\n"
        self.assertEqual(totals(text), {"ann": 4.5, "bob": 2.0})

    def test_columns_in_any_order(self):
        text = "amount,id,customer\n2,1,ann\n3,2,ann\n"
        self.assertEqual(totals(text), {"ann": 5.0})

    def test_quoted_fields_with_commas(self):
        text = 'customer,amount\n"Smith, Ann",2\n"Smith, Ann",1\n'
        self.assertEqual(totals(text), {"Smith, Ann": 3.0})

    def test_blank_lines_are_skipped(self):
        text = "customer,amount\nann,1\n\nann,1\n"
        self.assertEqual(totals(text), {"ann": 2.0})


if __name__ == "__main__":
    unittest.main()
