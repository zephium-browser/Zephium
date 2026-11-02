import os
import re
import tempfile
from pathlib import Path
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[2]


class ReleaseTagRefTests(unittest.TestCase):
    def test_tag_binding_and_release_regressions_are_wired_into_workflows(self):
        workflow = (ROOT / '.github/workflows/release.yml').read_text()
        self.assertIn('run: bash scripts/release/check_tag_ref.sh', workflow)
        self.assertIn("python3 -m unittest discover -s scripts/release -p 'test_*.py'",
                      (ROOT / '.github/workflows/ci.yml').read_text())

    def test_builds_and_ci_retain_the_gated_commit_when_the_tag_moves(self):
        workflow = (ROOT / '.github/workflows/release.yml').read_text()
        with tempfile.TemporaryDirectory() as directory:
            work = Path(directory)
            def git(*arguments):
                return subprocess.check_output(['git', *arguments], cwd=work, text=True,
                                               stderr=subprocess.DEVNULL).strip()
            git('init', '-q')
            for message in ['gated candidate', 'later unrelated commit']:
                git('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid',
                    'commit', '--allow-empty', '-qm', message)
                if message == 'gated candidate':
                    candidate = git('rev-parse', 'HEAD')
                    git('tag', 'v1.2.3')
            later = git('rev-parse', 'HEAD')
            git('checkout', '--detach', candidate)
            gate = subprocess.run(['bash', str(ROOT / 'scripts/release/check_tag_ref.sh')], cwd=work,
                env=dict(os.environ, TAG='v1.2.3', GITHUB_REF='refs/tags/v1.2.3', GITHUB_SHA=candidate),
                capture_output=True, text=True)
            self.assertEqual(gate.returncode, 0, gate.stderr)
            git('tag', '-f', 'v1.2.3', later)
            for name in ['ci', 'macos', 'windows']:
                with self.subTest(job=name):
                    job = re.search(rf'^  {name}:\n(.*?)(?=^  [a-z][\w-]*:\n|\Z)',
                                    workflow, re.M | re.S).group(1)
                    ref = re.search(r'^          ref: (.+)$', job, re.M).group(1)
                    selected = candidate if ref == '${{ github.sha }}' else ref.replace('${{ env.TAG }}', 'v1.2.3')
                    git('checkout', '--detach', selected)
                    self.assertEqual(git('rev-parse', 'HEAD'), candidate,
                                     f'{name} followed the moved tag instead of the commit approved by gate')

    def run_ref(self, ref, sha=None):
        candidate = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip()
        env = dict(os.environ, TAG='v1.2.3', GITHUB_REF=ref,
                   GITHUB_SHA=candidate if sha is None else sha)
        return subprocess.run(['bash', str(ROOT / 'scripts/release/check_tag_ref.sh')],
                              cwd=ROOT, env=env, capture_output=True, text=True)

    def test_tag_dispatch_and_matching_checkout_are_accepted(self):
        self.assertEqual(self.run_ref('refs/tags/v1.2.3').returncode, 0)

    def test_default_branch_or_other_tag_dispatch_is_refused(self):
        for ref in ['refs/heads/main', 'refs/tags/v1.2.2']:
            result = self.run_ref(ref)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('requested tag ref', result.stdout)

    def test_moved_tag_checkout_cannot_claim_the_workflow_commit(self):
        result = self.run_ref('refs/tags/v1.2.3', '0' * 40)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('differs from the workflow commit', result.stdout)


if __name__ == '__main__': unittest.main()
