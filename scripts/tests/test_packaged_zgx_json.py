"""The zgx.json a package ships: made from the interface's, and checked to have kept it."""
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SCRIPTS = Path(__file__).parents[1]
OURS = SCRIPTS.parent / 'apps/zgx/ui/zgx.json'


def run(script, *args):
    return subprocess.run([sys.executable, str(SCRIPTS / script), *map(str, args)], capture_output=True, text=True)


class PackagedZgxJson(unittest.TestCase):
    def setUp(self):
        held = tempfile.TemporaryDirectory()
        self.addCleanup(held.cleanup)
        self.dir = Path(held.name)
        self.out = self.dir / 'zgx.json'

    def test_the_interfaces_own_is_kept_whole(self):
        made = run('zgx-app-json.py', OURS, '1.2.3', 'reply=/modules/reply', self.out)
        self.assertEqual(made.returncode, 0, made.stderr)
        config = json.loads(self.out.read_text())
        self.assertEqual((config['app_id'], config['version'], config['modules']), ('zeron', '1.2.3', {'reply': '/modules/reply'}))
        # As `zgx build` ships it: without its modules.
        del config['modules']
        self.out.write_text(json.dumps(config))
        checked = run('check-packaged-zgx-json.py', OURS, self.out)
        self.assertEqual(checked.returncode, 0, checked.stderr)

    def test_the_file_the_scripts_wrote_before_is_refused(self):
        self.out.write_text(json.dumps({'app_id': 'zeron', 'version': '1.2.3'}))
        checked = run('check-packaged-zgx-json.py', OURS, self.out)
        self.assertEqual(checked.returncode, 1)
        self.assertIn('named_windows: not there', checked.stderr)

    def test_any_key_lost_or_changed_is_refused(self):
        whole = json.loads(OURS.read_text())

        def paths(value, at=()):
            for key, inner in value.items():
                yield at + (key,)
                if isinstance(inner, dict):
                    yield from paths(inner, at + (key,))

        every = list(paths(whole))
        self.assertGreater(len(every), 30)
        for path in every:
            for change in ('lost', 'changed'):
                config = json.loads(OURS.read_text())
                within = config
                for key in path[:-1]:
                    within = within[key]
                if change == 'lost':
                    del within[path[-1]]
                else:
                    within[path[-1]] = 'something else'
                self.out.write_text(json.dumps(config))
                checked = run('check-packaged-zgx-json.py', OURS, self.out)
                self.assertEqual(checked.returncode, 1, (change, path))
                self.assertIn('.'.join(path), checked.stderr)

    def test_a_missing_file_is_refused(self):
        self.assertEqual(run('check-packaged-zgx-json.py', OURS, self.dir / 'none.json').returncode, 1)

    def test_the_packagings_own_keys_are_its_to_say(self):
        ours = self.dir / 'ours.json'
        ours.write_text(json.dumps({'version': '9'}))
        made = run('zgx-app-json.py', ours, '1.2.3', self.out)
        self.assertNotEqual(made.returncode, 0)
        self.assertIn('`version`', made.stderr)


if __name__ == '__main__':
    unittest.main()
