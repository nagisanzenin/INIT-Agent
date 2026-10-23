# Visible subset (15 of 43) of tests auto-generated with test data from:
# https://github.com/exercism/problem-specifications/tree/main/exercises/rational-numbers/canonical-data.json
# File last updated on 2023-07-19

import unittest

from rational_numbers import (
    Rational,
)


class RationalNumbersTest(unittest.TestCase):

    # Tests of type: Arithmetic

    # Addition

    def test_add_two_positive_rational_numbers(self):
        self.assertEqual(Rational(1, 2) + Rational(2, 3), Rational(7, 6))

    def test_add_a_positive_rational_number_and_a_negative_rational_number(self):
        self.assertEqual(Rational(1, 2) + Rational(-2, 3), Rational(-1, 6))

    def test_add_two_negative_rational_numbers(self):
        self.assertEqual(Rational(-1, 2) + Rational(-2, 3), Rational(-7, 6))

    def test_add_a_rational_number_to_its_additive_inverse(self):
        self.assertEqual(Rational(1, 2) + Rational(-1, 2), Rational(0, 1))

    # Subtraction

    def test_subtract_two_positive_rational_numbers(self):
        self.assertEqual(Rational(1, 2) - Rational(2, 3), Rational(-1, 6))

    def test_subtract_a_positive_rational_number_and_a_negative_rational_number(self):
        self.assertEqual(Rational(1, 2) - Rational(-2, 3), Rational(7, 6))

    def test_subtract_two_negative_rational_numbers(self):
        self.assertEqual(Rational(-1, 2) - Rational(-2, 3), Rational(1, 6))

    def test_subtract_a_rational_number_from_itself(self):
        self.assertEqual(Rational(1, 2) - Rational(1, 2), Rational(0, 1))

    # Multiplication

    def test_multiply_two_positive_rational_numbers(self):
        self.assertEqual(Rational(1, 2) * Rational(2, 3), Rational(1, 3))

    def test_multiply_a_negative_rational_number_by_a_positive_rational_number(self):
        self.assertEqual(Rational(-1, 2) * Rational(2, 3), Rational(-1, 3))

    def test_multiply_two_negative_rational_numbers(self):
        self.assertEqual(Rational(-1, 2) * Rational(-2, 3), Rational(1, 3))

    def test_multiply_a_rational_number_by_its_reciprocal(self):
        self.assertEqual(Rational(1, 2) * Rational(2, 1), Rational(1, 1))

    def test_multiply_a_rational_number_by_1(self):
        self.assertEqual(Rational(1, 2) * Rational(1, 1), Rational(1, 2))

    def test_multiply_a_rational_number_by_0(self):
        self.assertEqual(Rational(1, 2) * Rational(0, 1), Rational(0, 1))

    # Division

    def test_divide_two_positive_rational_numbers(self):
        self.assertEqual(Rational(1, 2) / Rational(2, 3), Rational(3, 4))

    # Tests of type: Absolute value

    # Tests of type: Exponentiation of a rational number

    # Tests of type: Exponentiation of a real number to a rational number

    # Tests of type: Reduction to lowest terms
