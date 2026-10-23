# Visible subset (8 of 24) of tests auto-generated with test data from:
# https://github.com/exercism/problem-specifications/tree/main/exercises/word-search/canonical-data.json
# File last updated on 2023-07-19

import unittest

from word_search import (
    WordSearch,
    Point,
)


class WordSearchTest(unittest.TestCase):
    def test_should_accept_an_initial_game_grid_and_a_target_search_word(self):
        puzzle = WordSearch(["jefblpepre"])
        self.assertIsNone(puzzle.search("clojure"))

    def test_should_locate_one_word_written_left_to_right(self):
        puzzle = WordSearch(["clojurermt"])
        self.assertEqual(puzzle.search("clojure"), (Point(0, 0), Point(6, 0)))

    def test_should_locate_the_same_word_written_left_to_right_in_a_different_position(
        self,
    ):
        puzzle = WordSearch(["mtclojurer"])
        self.assertEqual(puzzle.search("clojure"), (Point(2, 0), Point(8, 0)))

    def test_should_locate_a_different_left_to_right_word(self):
        puzzle = WordSearch(["coffeelplx"])
        self.assertEqual(puzzle.search("coffee"), (Point(0, 0), Point(5, 0)))

    def test_should_locate_that_different_left_to_right_word_in_a_different_position(
        self,
    ):
        puzzle = WordSearch(["xcoffeezlp"])
        self.assertEqual(puzzle.search("coffee"), (Point(1, 0), Point(6, 0)))

    def test_should_locate_a_left_to_right_word_in_two_line_grid(self):
        puzzle = WordSearch(["jefblpepre", "tclojurerm"])
        self.assertEqual(puzzle.search("clojure"), (Point(1, 1), Point(7, 1)))

    def test_should_locate_a_left_to_right_word_in_three_line_grid(self):
        puzzle = WordSearch(["camdcimgtc", "jefblpepre", "clojurermt"])
        self.assertEqual(puzzle.search("clojure"), (Point(0, 2), Point(6, 2)))

    def test_should_locate_a_left_to_right_word_in_ten_line_grid(self):
        puzzle = WordSearch(
            [
                "jefblpepre",
                "camdcimgtc",
                "oivokprjsm",
                "pbwasqroua",
                "rixilelhrs",
                "wolcqlirpc",
                "screeaumgr",
                "alxhpburyi",
                "jalaycalmp",
                "clojurermt",
            ]
        )
        self.assertEqual(puzzle.search("clojure"), (Point(0, 9), Point(6, 9)))
