#!/usr/bin/env python3
"""Copy license notices for the resolved RDP dependency graph into a package."""
import json
from pathlib import Path
import shutil
import subprocess
import sys

output = Path(sys.argv[1])
output.mkdir(parents=True, exist_ok=True)
metadata = json.loads(subprocess.check_output(['cargo', 'metadata', '--locked', '--format-version', '1']))
packages = {p['id']: p for p in metadata['packages']}
nodes = {n['id']: n for n in metadata['resolve']['nodes']}
root = next(p['id'] for p in packages.values() if p['name'] == 'zeron-rdp')
seen, pending, inventory = set(), [root], []
while pending:
    package_id = pending.pop()
    if package_id in seen:
        continue
    seen.add(package_id)
    package = packages[package_id]
    for dep in nodes[package_id]['deps']:
        if any(kind['kind'] != 'dev' for kind in dep['dep_kinds']):
            pending.append(dep['pkg'])
    if package_id == root:
        continue
    directory = Path(package['manifest_path']).parent
    files = set()
    if package.get('license_file'):
        files.add(directory / package['license_file'])
    for candidate in [directory, *list(directory.parents)[:2]]:
        matches = [p for p in candidate.iterdir() if p.is_file() and p.name.upper().startswith(('LICENSE', 'LICENCE', 'COPYING', 'NOTICE'))]
        if matches:
            files.update(matches)
            break
    destination = output / f"{package['name']}-{package['version']}"
    destination.mkdir(exist_ok=True)
    copied = []
    for file in sorted(files):
        if file.is_file():
            shutil.copyfile(file, destination / file.name)
            copied.append(file.name)
    inventory.append({'name': package['name'], 'version': package['version'], 'license': package['license'], 'repository': package.get('repository'), 'notices': copied})
(output / 'inventory.json').write_text(json.dumps(sorted(inventory, key=lambda p: (p['name'], p['version'])), indent=2) + '\n')
print(f'Collected notices and license metadata for {len(inventory)} RDP dependencies')
