// Exercises src/pi/images.mjs with a stub Pi package: tool-result re-encoding
// and the per-request image budget.
import assert from 'node:assert/strict';
import {copyFileSync, mkdirSync, mkdtempSync, rmSync, writeFileSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {pathToFileURL} from 'node:url';

const dir = mkdtempSync(join(tmpdir(), 'zeron-pi-images-'));
try {
  const pkg = join(dir, 'node_modules/@earendil-works/pi-coding-agent');
  mkdirSync(pkg, {recursive: true});
  writeFileSync(join(pkg, 'package.json'), JSON.stringify({name: '@earendil-works/pi-coding-agent', type: 'module', main: 'index.js'}));
  // Halves the side and returns a short JPEG.
  writeFileSync(join(pkg, 'index.js'), `export async function resizeImage(buffer, mimeType, options) {
    return {data: 'J'.repeat(1000), mimeType: 'image/jpeg', width: 640, height: 360, originalWidth: 1280, originalHeight: 720};
  }`);
  copyFileSync(process.argv[2], join(dir, 'images.mjs'));
  const images = await import(pathToFileURL(join(dir, 'images.mjs')));
  const handlers = {};
  const entries = [];
  let settings = {};
  images.default({
    on: (event, fn) => handlers[event] = fn,
    appendEntry: (customType, data) => entries.push({type: 'custom', customType, data}),
    getSettings: () => settings,
  });
  const ctx = {model: undefined, sessionManager: {getBranch: () => entries}};

  // tool_result: big images are re-encoded, small ones and structured output kept.
  const big = {type: 'image', data: 'A'.repeat(600 * 1024), mimeType: 'image/png'};
  const small = {type: 'image', data: 'B'.repeat(100), mimeType: 'image/png'};
  const result = await handlers.tool_result({content: [big, small], structuredContent: {ok: 1}}, ctx);
  assert.equal(result.content[0].mimeType, 'image/jpeg');
  assert.match(result.content[1].text, /Multiply coordinates by 2\.00/);
  assert.deepEqual(result.content[2], small);
  assert.deepEqual(result.structuredContent, {ok: 1});
  assert.equal(await handlers.tool_result({content: [small]}, ctx), undefined);
  // Declared model resize limits or autoResize off: Pi's own handling applies.
  const resizing = {model: {inputLimits: {images: {resize: {maxBytes: 1 << 20}}}}};
  assert.equal(await handlers.tool_result({content: [big]}, resizing), undefined);
  settings = {images: {autoResize: false}};
  assert.equal(await handlers.tool_result({content: [big]}, ctx), undefined);
  settings = {};

  // requestLimits: defaults, then the stricter of defaults and declared limits.
  const mib = 1024 * 1024;
  assert.deepEqual(images.requestLimits(undefined), {maxBytes: 20 * mib, maxImages: 80, byteQuantum: 8 * mib, countQuantum: 20});
  const strict = images.requestLimits({inputLimits: {maxRequestBytes: 10 * mib, images: {maxPerRequest: 4}}});
  assert.equal(strict.maxBytes, 6 * mib);
  assert.equal(strict.maxImages, 4);
  assert.equal(strict.countQuantum, 1);
  assert.equal(images.requestLimits(undefined, 0.5).maxBytes, 10 * mib);

  // dropCount: nothing within limits; whole quanta beyond them.
  const limits = {maxBytes: 100, byteQuantum: 40, maxImages: 10, countQuantum: 4};
  assert.equal(images.dropCount([50, 50], limits), 0);
  assert.equal(images.dropCount([30, 30, 30, 30], limits), 2); // excess 20 -> drop >= 40 bytes
  assert.equal(images.dropCount(Array(11).fill(1), limits), 4); // excess 1 image -> drop 4
  assert.equal(images.dropCount([500], limits), 1); // a lone oversized image goes too

  // The cut moves only in quanta: growing history keeps earlier drops stable.
  const counts = [];
  for (let n = 1; n <= 40; n++) counts.push(images.dropCount(Array(n).fill(10), limits));
  for (let i = 1; i < counts.length; i++) assert.ok(counts[i] >= counts[i - 1]);
  assert.ok(new Set(counts).size < counts.length / 2, `cut moves in steps: ${counts}`);

  // offloadImages: oldest images become text naming their source; others untouched.
  const shot = n => ({type: 'image', data: 'C'.repeat(n), mimeType: 'image/jpeg'});
  const messages = [
    {role: 'user', content: [{type: 'text', text: 'look'}, shot(60)]},
    {role: 'assistant', content: [{type: 'toolCall', id: 'r1', name: 'read', arguments: {path: '/tmp/a.png'}}]},
    {role: 'toolResult', toolCallId: 'r1', toolName: 'read', content: [shot(60)], timestamp: 7},
    {role: 'user', content: 'plain string content'},
    {role: 'toolResult', toolCallId: 'r2', toolName: 'screenshot', content: [shot(60)]},
  ];
  assert.equal(images.offloadImages(messages, {...limits, maxBytes: 1000}), null);
  const out = images.offloadImages(messages, limits);
  assert.equal(out[0].content[0].text, 'look');
  assert.match(out[0].content[1].text, /^\[Earlier image omitted/);
  assert.match(out[2].content[0].text, /from \/tmp\/a\.png/);
  assert.equal(out[2].timestamp, 7);
  assert.equal(out[1], messages[1]);
  assert.equal(out[3], messages[3]);
  assert.equal(out[4], messages[4]);
  assert.equal(messages[0].content[1].type, 'image', 'input is not mutated');

  // context hook: under the default budget nothing changes; over it the newest stay.
  assert.equal(handlers.context({messages}, ctx), undefined);
  const many = Array.from({length: 100}, () => ({role: 'toolResult', toolCallId: 'x', content: [shot(10)]}));
  const kept = list => list.filter(m => m.content[0].type === 'image').length;
  const full = kept(handlers.context({messages: many}, ctx).messages);
  assert.ok(full <= 80 && full > 0, `kept ${full}`);
  assert.equal(handlers.context({messages: many}, ctx).messages.at(-1).content[0].type, 'image');

  // A body-size rejection halves the budget, records it and retries once.
  const rejected = {role: 'assistant', stopReason: 'error', errorMessage: '413 Failed to buffer the request body: length limit exceeded'};
  handlers.message_end({message: {role: 'assistant', stopReason: 'error', errorMessage: '502 bad gateway'}}, ctx);
  assert.equal(entries.length, 0, 'other errors leave the budget alone');
  handlers.message_end({message: rejected}, ctx);
  assert.deepEqual(entries.at(-1), {type: 'custom', customType: 'zeron-image-budget', data: {scale: 0.5}});
  assert.deepEqual(handlers.agent_before_settle({outcome: 'error', context: {canContinue: true}}), {continue: true});
  assert.equal(handlers.agent_before_settle({outcome: 'error', context: {canContinue: true}}), undefined, 'one retry per halving');
  const half = kept(handlers.context({messages: many}, ctx).messages);
  assert.ok(half <= 40 && half < full, `kept ${half} after 413`);

  // A new Pi process (the next run) restores the halved budget from the session.
  const next = {};
  images.default({on: (event, fn) => next[event] = fn, appendEntry: () => {}, getSettings: () => ({})});
  assert.equal(kept(next.context({messages: many}, ctx).messages), half);
  // The budget bottoms out, so repeated rejections cannot loop.
  for (let i = 0; i < 10; i++) handlers.message_end({message: rejected}, ctx);
  assert.equal(entries.at(-1).data.scale, 1 / 8);
  assert.equal(entries.length, 3);
} finally {
  rmSync(dir, {recursive: true, force: true});
}
