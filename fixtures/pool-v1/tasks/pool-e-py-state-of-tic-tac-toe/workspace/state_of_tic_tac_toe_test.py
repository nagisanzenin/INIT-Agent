# Visible subset (10 of 29) of tests auto-generated with test data from:
# https://github.com/exercism/problem-specifications/tree/main/exercises/state-of-tic-tac-toe/canonical-data.json
# File last updated on 2026-09-30

import unittest

from state_of_tic_tac_toe import (
    gamestate,
)


class StateOfTicTacToeTest(unittest.TestCase):
    def test_finished_game_where_x_won_via_left_column_victory(self):
        board = [
            "XOO",
            "X  ",
            "X  ",
        ]
        self.assertEqual(gamestate(board), "win")

    def test_finished_game_where_x_won_via_middle_column_victory(self):
        board = [
            "OXO",
            " X ",
            " X ",
        ]
        self.assertEqual(gamestate(board), "win")

    def test_finished_game_where_x_won_via_right_column_victory(self):
        board = [
            "OOX",
            "  X",
            "  X",
        ]
        self.assertEqual(gamestate(board), "win")

    def test_finished_game_where_o_won_via_left_column_victory(self):
        board = [
            "OXX",
            "OX ",
            "O  ",
        ]
        self.assertEqual(gamestate(board), "win")

    def test_finished_game_where_o_won_via_middle_column_victory(self):
        board = [
            "XOX",
            " OX",
            " O ",
        ]
        self.assertEqual(gamestate(board), "win")

    def test_finished_game_where_o_won_via_right_column_victory(self):
        board = [
            "XXO",
            " XO",
            "  O",
        ]
        self.assertEqual(gamestate(board), "win")

    def test_finished_game_where_x_won_via_top_row_victory(self):
        board = [
            "XXX",
            "XOO",
            "O  ",
        ]
        self.assertEqual(gamestate(board), "win")

    def test_finished_game_where_x_won_via_middle_row_victory(self):
        board = [
            "O  ",
            "XXX",
            " O ",
        ]
        self.assertEqual(gamestate(board), "win")

    def test_finished_game_where_x_won_via_bottom_row_victory(self):
        board = [
            " OO",
            "O X",
            "XXX",
        ]
        self.assertEqual(gamestate(board), "win")

    def test_finished_game_where_o_won_via_top_row_victory(self):
        board = [
            "OOO",
            "XXO",
            "XX ",
        ]
        self.assertEqual(gamestate(board), "win")
