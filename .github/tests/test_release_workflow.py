import pathlib
import unittest


class ReleaseWorkflowTests(unittest.TestCase):
    def test_package_archive_uses_step_output(self):
        repository_root = pathlib.Path(__file__).resolve().parents[2]
        workflow = (repository_root / ".github" / "workflows" / "release.yml").read_text()

        self.assertIn("id: package", workflow)
        self.assertIn('echo "archive=${name}.zip" >> "$GITHUB_OUTPUT"', workflow)
        self.assertIn('echo "archive=${name}.tar.gz" >> "$GITHUB_OUTPUT"', workflow)
        self.assertIn("${{ steps.package.outputs.archive }}", workflow)
        self.assertNotIn("env.ARCHIVE", workflow)
        self.assertNotIn("GITHUB_ENV", workflow)


if __name__ == "__main__":
    unittest.main()
