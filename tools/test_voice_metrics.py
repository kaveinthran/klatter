"""Synthetic checks of the voice metrics: python -m unittest discover -s tools."""
import unittest

import numpy as np

import voice_metrics as vm


class PauseTests(unittest.TestCase):
    def test_internal_pause_is_counted_with_or_without_edge_silence(self):
        for leading, trailing in [(0, 0), (20, 0), (0, 20), (20, 20)]:
            speech = np.array([False] * leading + [True] * 20 + [False] * 20
                              + [True] * 20 + [False] * trailing)
            with self.subTest(leading=leading, trailing=trailing):
                stats = vm.pause_stats(speech)
                self.assertAlmostEqual(stats["pauses_per_min"], 150.0)
                self.assertAlmostEqual(stats["pause_mean_ms"], 200.0)
                self.assertAlmostEqual(stats["pause_fraction"], 20 / len(speech))

    def test_short_closures_and_edge_silence_are_not_pauses(self):
        for mask in [[1] * 60, [0] * 20 + [1] * 60 + [0] * 20,
                     [1] * 20 + [0] * 9 + [1] * 20]:
            self.assertEqual(vm.pause_stats(np.array(mask, dtype=bool)),
                             dict(pauses_per_min=0, pause_mean_ms=0, pause_fraction=0))

    def test_multiple_internal_pauses_include_first_and_last(self):
        speech = np.array([1] * 20 + [0] * 10 + [1] * 20 + [0] * 30 + [1] * 20,
                          dtype=bool)
        stats = vm.pause_stats(speech)
        self.assertAlmostEqual(stats["pauses_per_min"], 200.0)
        self.assertAlmostEqual(stats["pause_mean_ms"], 200.0)
        self.assertAlmostEqual(stats["pause_fraction"], 0.4)


if __name__ == "__main__":
    unittest.main()
