// Regenerate only deliberately: these persisted bytes must stay from 1.13.7.
// Run from the repository root: node edge/scripts/loro-legacy-fixtures.cjs
const { LoroDoc } = require('loro-crdt-legacy');
const { version } = require('loro-crdt-legacy/package.json');
const { writeFileSync } = require('node:fs');
const { resolve } = require('node:path');
if (version !== '1.13.7') throw new Error(`Unexpected fixture writer ${version}`);
const dir = resolve(__dirname, '../../crates/doc/tests/fixtures/loro-1.13.7');
const doc = new LoroDoc();
doc.setPeerId(7);
doc.getText('text').insert(0, 'Hello 🌍');
doc.getMap('metadata').set('author', 'legacy');
doc.getList('messages').push('first');
doc.getMovableList('queue').push('one');
doc.getMovableList('queue').push('two');
doc.commit();
writeFileSync(resolve(dir, 'snapshot.bin'), doc.export({ mode: 'snapshot' }));
writeFileSync(resolve(dir, 'shallow.bin'), doc.export({ mode: 'shallow-snapshot', frontiers: doc.oplogFrontiers() }));
writeFileSync(resolve(dir, 'initial.json'), JSON.stringify(doc.toJSON(), null, 2) + '\n');
const versionBefore = doc.version();
doc.getText('text').insert(doc.getText('text').length, ' from legacy');
doc.getMovableList('queue').move(0, 1);
doc.getList('messages').push('second');
doc.commit();
writeFileSync(resolve(dir, 'update.bin'), doc.export({ mode: 'update', from: versionBefore }));
writeFileSync(resolve(dir, 'updated.json'), JSON.stringify(doc.toJSON(), null, 2) + '\n');
doc.free();
