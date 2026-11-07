"""Exercise the release workflow's actual gating expression and note script."""
import itertools
import json
import os
import re
import subprocess
import tempfile
import textwrap
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = ROOT / '.github/workflows/release.yml'


def job(text, name):
    return re.search(rf'^  {name}:\n(.*?)(?=^  \w[\w-]*:\n|\Z)', text, re.M | re.S).group(1)


class ReleaseWorkflowTests(unittest.TestCase):
    def test_components_have_one_retry_and_an_explicit_emergency_opt_out(self):
        workflow = WORKFLOW.read_text()
        self.assertRegex(workflow, r'emergency:\n(?:.*\n){1,4}?        type: boolean')
        self.assertIn("if: ${{ !inputs.emergency }}", job(workflow, 'components'))
        self.assertIn('--retry=1', (ROOT / '.github/workflows/components.yml').read_text())


    def test_draft_gates_all_required_jobs_including_skipped_component_cases(self):
        publish = job(WORKFLOW.read_text(), 'publish')
        dependencies = re.search(r'^    needs: \[([^]]+)\]', publish, re.M).group(1).split(', ')
        self.assertEqual(set(dependencies), {'gate', 'ci', 'components', 'macos', 'windows'})
        match = re.search(r'^    if: \$\{\{ (.+) \}\}', publish, re.M)
        self.assertIsNotNone(match, 'draft needs an explicit condition for emergency skipped components')
        expression = match.group(1)
        for statuses in itertools.product(['success', 'failure', 'skipped', 'cancelled'], repeat=5):
            results = dict(zip(dependencies, statuses))
            for emergency, cancelled in itertools.product([False, True], repeat=2):
                condition = expression.replace('!cancelled()', str(not cancelled))
                condition = condition.replace('inputs.emergency', str(emergency))
                condition = re.sub(r'needs\.(\w+)\.result', lambda m: repr(results[m[1]]), condition)
                condition = condition.replace('&&', ' and ').replace('||', ' or ')
                actual = eval(condition, {'__builtins__': {}}, {})
                expected = not cancelled and all(results[n] == 'success' for n in dependencies if n != 'components') and (
                    results['components'] == 'success' or (emergency and results['components'] == 'skipped'))
                self.assertEqual(actual, expected, (results, emergency, cancelled))


    def test_emergency_component_bypass_is_visible_in_created_release_notes(self):
        workflow = WORKFLOW.read_text()
        publish = job(workflow, 'publish')
        self.assertIn('EMERGENCY_SKIP_COMPONENTS: ${{ inputs.emergency }}', publish)
        step = publish.split('      - name: Create the draft release\n', 1)[1]
        script = textwrap.dedent(step.split('        run: |\n', 1)[1])
        for emergency in ['true', 'false', '']:
            with tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                (root / 'dist').mkdir()
                gh = root / 'gh'
                gh.write_text('#!/bin/sh\ncase "$1 $2" in\n"release view") exit 1;;\n"release list"|"release create") exit 0;;\n*) exit 2;;\nesac\n')
                gh.chmod(0o755)
                result = subprocess.run(['bash', '-euo', 'pipefail', '-c', script], cwd=root,
                    env=dict(os.environ, PATH=f'{root}:{os.environ["PATH"]}', RUNNER_TEMP=directory,
                             TAG='v1.2.3', VERSION='1.2.3', GITHUB_REPOSITORY='example/repo',
                             SIGNPATH_ENABLED='false', EMERGENCY_SKIP_COMPONENTS=emergency), capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stderr)
                notes = (root / 'notes.md').read_text()
                self.assertEqual('EMERGENCY: Component tests were skipped' in notes, emergency == 'true')


    def test_stable_publisher_rejects_unrelated_failed_or_other_tag_runs(self):
        publish = job((ROOT / '.github/workflows/publish-stable.yml').read_text(), 'publish')
        step = publish.split("      - name: Require the exact candidate's main CI and complete release workflow\n", 1)[1]
        script = textwrap.dedent(step.split('        run: |\n', 1)[1].split('\n      - name:', 1)[0])
        candidate = 'a' * 40
        ci = dict(head_sha=candidate, head_branch='main', event='push', conclusion='success')
        release = dict(head_sha=candidate, head_branch='v1.2.3', event='push', conclusion='success')
        cases = [(ci, release, True), (ci, dict(release, event='workflow_dispatch'), True)]
        for field, bad in [('head_sha', 'b' * 40), ('head_branch', 'other'),
                           ('event', 'pull_request'), ('conclusion', 'failure')]:
            cases.extend([(dict(ci, **{field: bad}), release, False),
                          (ci, dict(release, **{field: bad}), False)])
        for main_run, release_run, expected in cases:
            with self.subTest(main=main_run, release=release_run), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                (root / 'ci.json').write_text(json.dumps({'workflow_runs': [main_run]}))
                (root / 'release.json').write_text(json.dumps({'workflow_runs': [release_run]}))
                gh = root / 'gh'
                gh.write_text('#!/bin/sh\ncase "$2" in\n*ci.yml*) cat "$FIXTURES/ci.json";;\n*release.yml*) cat "$FIXTURES/release.json";;\n*) exit 2;;\nesac\n')
                gh.chmod(0o755)
                result = subprocess.run(['bash', '-c', script], cwd=root,
                    env=dict(os.environ, PATH=f'{root}:{os.environ["PATH"]}', FIXTURES=directory,
                             CANDIDATE=candidate, TAG='v1.2.3', GITHUB_REPOSITORY='example/repo'),
                    capture_output=True, text=True)
                self.assertEqual(result.returncode == 0, expected, result.stderr)


    def test_release_and_publisher_share_tag_lock_and_latest_edits_are_serialized(self):
        release = WORKFLOW.read_text()
        stable = (ROOT / '.github/workflows/publish-stable.yml').read_text()
        for workflow in [release, stable]:
            group = re.search(r'^concurrency:\n  group: (.+)$', workflow, re.M).group(1)
            self.assertEqual(group, 'release-${{ inputs.tag || github.ref_name }}')
            for inputs, ref in [('v1.2.3', 'main'), ('', 'v1.2.3')]:
                self.assertEqual('release-' + (inputs or ref), 'release-v1.2.3')
        self.assertRegex(job(stable, 'publish'),
                         r'    concurrency:\n      group: stable-latest-publication\n      cancel-in-progress: false')

    def test_latest_promotion_requires_a_newer_canonical_stable_version(self):
        publish = job((ROOT / '.github/workflows/publish-stable.yml').read_text(), 'publish')
        step = publish.split('      - name: Publish the verified draft\n', 1)[1]
        script = textwrap.dedent(step.split('        run: |\n', 1)[1]) if '        run: |\n' in step else step.split('        run: ', 1)[1].strip()
        cases = [
            ('v1.10.0', 200, 'v1.9.9', 0, True),
            ('v1.9.0', 200, 'v1.10.0', 0, False),
            ('v1.10.0', 200, 'v1.10.0', 0, False),
            ('v1.0.0', 404, None, 1, True),
            ('v1.0.0', 403, None, 1, None),
            ('v1.0.0', 500, None, 1, None),
            ('v1.0.0', 0, None, 1, None),
            ('v1.0.0', 200, 'garbage', 0, None),
            ('v1.0.0', 200, 'v01.0.0', 0, None),
            ('v1.0.0', 200, 'v1.0.0-beta.1', 0, True),
            ('v1.0.1', 200, 'v1.1.0-beta.1', 0, False),
            ('v1.0.1', 200, 'v1.0.0-0', 0, True),
            ('v1.0.0', 200, 'v1.0.0-beta.1+build.01', 0, True),
            ('v1.0.0', 200, 'v1.0.0+build.01', 0, False),
            ('v1.0.0', 200, 'v1.0.0-01', 0, None),
            ('v1.0.0', 200, 'v1.0.0-beta..1', 0, None),
            ('v1.0.0', 200, 'v1.0.0-beta.', 0, None),
            ('v1.0.0', 200, 'v1.0.0-', 0, None),
            ('v1.0.0', 200, 'v1.0.0+build..1', 0, None),
            ('v1.0.0-beta.1', 404, None, 1, None),
            ('v1.0.0+build.1', 404, None, 1, None),
        ]
        for tag, status, latest, api_exit, expected in cases:
            with self.subTest(tag=tag, status=status, latest=latest), tempfile.TemporaryDirectory() as directory:
                work = Path(directory)
                payload = json.dumps({'tag_name': latest, 'draft': False, 'prerelease': False})
                response = f'HTTP/2.0 {status} Fixture\nContent-Type: application/json\n\n{payload}\n' if status else ''
                (work / 'response').write_text(response)
                gh = work / 'gh'
                gh.write_text("#!/bin/sh\nif [ \"$1\" = api ]; then cat \"$FIXTURES/response\"; exit \"$API_EXIT\"; fi\nif [ \"$1 $2\" = 'release edit' ]; then printf '%s\\n' \"$@\" > \"$FIXTURES/edit\"; exit 0; fi\nexit 9\n")
                gh.chmod(0o755)
                result = subprocess.run(['bash', '-euo', 'pipefail', '-c', script], cwd=work,
                    env=dict(os.environ, PATH=f'{work}:{os.environ["PATH"]}', TAG=tag,
                             GITHUB_REPOSITORY='example/repo', RUNNER_TEMP=directory,
                             FIXTURES=directory, API_EXIT=str(api_exit)), capture_output=True, text=True)
                if expected is None:
                    self.assertNotEqual(result.returncode, 0)
                    self.assertFalse((work / 'edit').exists())
                else:
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertIn(f'--latest={str(expected).lower()}', (work / 'edit').read_text().splitlines())

    def test_stable_publication_only_changes_an_existing_verified_draft(self):
        workflow = (ROOT / '.github/workflows/publish-stable.yml').read_text()
        publish = job(workflow, 'publish')
        self.assertIn('actions: read', publish)
        self.assertNotIn('environment:', publish)
        self.assertNotIn('validate_qualification', publish)
        self.assertIn('--source-digest "${CANDIDATE}"', publish)
        self.assertIn('.head_branch == "main" and .event == "push"', publish)
        self.assertIn('release.yml', publish)
        self.assertIn("gh release view", publish)
        self.assertIn("grep -qx true", publish)
        self.assertIn('gh release edit "${TAG}" --draft=false', publish)
        self.assertNotIn('gh release create', publish)
        self.assertNotIn('gh release upload', publish)



if __name__ == '__main__':
    unittest.main()
