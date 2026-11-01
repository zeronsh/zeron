// Separate process/WASM instance so other tests cannot affect the high-water
// mark. Optional package argument allows verifying that the old version fails.
const assert = require('node:assert/strict');
const name = process.argv[2] || 'loro-crdt';
const { LoroDoc } = require(name);
const { __wasm } = require(`${name}/nodejs/loro_wasm.js`);
const before = __wasm.memory.buffer.byteLength;
const doc = new LoroDoc();
const text = doc.getText('text');
const token = 'streaming text: '.repeat(16);
const ticks = 2000;
for (let i = 0; i < ticks; i++) {
  text.insert(text.length, token);
  doc.commit();
  doc.export({ mode: 'snapshot' });
}
const growth = __wasm.memory.buffer.byteLength - before;
console.log(`WASM heap grew ${growth} bytes for ${token.length * ticks} streamed bytes`);
// WASM linear memory does not shrink: this bounds peak capacity, not JS RSS.
// Leave ample allocator headroom while catching the old hundreds-of-MiB leak.
assert.ok(growth < 32 * 1024 * 1024, `WASM heap grew ${growth} bytes`);
const restored = new LoroDoc();
restored.import(doc.export({ mode: 'snapshot' }));
assert.equal(restored.getText('text').toString(), token.repeat(ticks));
restored.free();
doc.free();
