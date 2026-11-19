import unittest

from roman import from_roman, to_roman


class ToRoman(unittest.TestCase):
    def test_additive(self):
        self.assertEqual(to_roman(3), "III")
        self.assertEqual(to_roman(2023), "MMXXIII")

    def test_subtractive(self):
        self.assertEqual(to_roman(4), "IV")
        self.assertEqual(to_roman(9), "IX")
        self.assertEqual(to_roman(1994), "MCMXCIV")
        self.assertEqual(to_roman(3999), "MMMCMXCIX")

    def test_out_of_range(self):
        with self.assertRaises(ValueError):
            to_roman(0)


class FromRoman(unittest.TestCase):
    def test_round_trip(self):
        for number in range(1, 4000):
            self.assertEqual(from_roman(to_roman(number)), number)


if __name__ == "__main__":
    unittest.main()
