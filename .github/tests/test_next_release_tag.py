import datetime
import importlib.util
import pathlib
import unittest


SCRIPT = pathlib.Path(__file__).parents[1] / "scripts" / "next_release_tag.py"
SPEC = importlib.util.spec_from_file_location("next_release_tag", SCRIPT)
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class NextReleaseTagTests(unittest.TestCase):
    NOW = datetime.datetime(2026, 11, 21, tzinfo=datetime.timezone.utc)

    def test_starts_at_one_for_new_month(self):
        tags = ["v2026.10.9", "v2025.11.7", "not-a-release"]
        self.assertEqual(MODULE.next_release_tag(tags, self.NOW), "v2026.11.1")

    def test_increments_highest_same_month_tag(self):
        tags = ["v2026.11.1", "v2026.11.3", "v2026.11.2"]
        self.assertEqual(MODULE.next_release_tag(tags, self.NOW), "v2026.11.4")

    def test_rejects_malformed_and_other_month_tags(self):
        tags = ["v2026.11.0", "v2026.11.-1", "v2026.11.4beta", "v2026.12.99"]
        self.assertEqual(MODULE.next_release_tag(tags, self.NOW), "v2026.11.1")


if __name__ == "__main__":
    unittest.main()
