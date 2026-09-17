import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { runInNewContext } from 'node:vm';
const source = readFileSync(new URL('./public/downloads.js', import.meta.url), 'utf8');
const html = readFileSync(new URL('./public/index.html', import.meta.url), 'utf8');
async function load(navigator, version = '0.2.67', fail = false) {
  const links = Object.fromEntries(['nav-download', 'hero-download', 'closing-download', 'ver'].map(id => [id, { href: '#downloads', setAttribute() {} }]));
  const choices = ['macos', 'windows', 'linux', 'linux-arm'].map(platformDownload => ({ dataset: { platformDownload } }));
  runInNewContext(source, { navigator, document: { getElementById: id => links[id], querySelectorAll: () => choices }, fetch: () => fail ? Promise.reject() : Promise.resolve({ ok: true, text: () => Promise.resolve(version) }) });
  await new Promise(resolve => setImmediate(resolve));
  return { links, choices };
}
for (const [platform, userAgent, file] of [
  ['MacIntel', 'Macintosh', 'macos-arm64.dmg'], ['Win32', 'Windows NT', 'windows-x86_64.zip'],
  ['Linux x86_64', 'Linux', 'linux-x86_64.tar.gz'], ['Linux aarch64', 'Linux', 'linux-aarch64.tar.gz'],
]) test(`desktop download: ${platform}`, async () => {
  const { links, choices } = await load({ platform, userAgent });
  for (const id of ['nav-download', 'hero-download', 'closing-download']) assert.equal(links[id].href, `https://zeron.sh/releases/zeron-0.2.67-${file}`);
  assert.equal(choices.length, 4);
  for (const choice of choices) assert.match(choice.href, /^https:\/\/zeron.sh\/releases\/zeron-0.2.67-/);
});
for (const navigator of [{ platform: 'Linux', userAgent: 'Android' }, { platform: 'MacIntel', maxTouchPoints: 5 }, { platform: 'iPhone' }, {}]) test(`mobile/unknown stays at chooser: ${JSON.stringify(navigator)}`, async () => {
  const { links } = await load(navigator);
  assert.equal(links['hero-download'].href, '#downloads');
});
for (const [version, fail] of [['<script>alert(1)</script>', false], ['0.2.65', false], ['', true]]) test(`safe fallback: ${version}`, async () => {
  const { links } = await load({ platform: 'Win32' }, version, fail);
  assert.equal(links['hero-download'].href, 'https://zeron.sh/releases/zeron-0.2.66-windows-x86_64.zip');
});
test('HTML retains all four explicit downloads without JavaScript', () => {
  for (const file of ['macos-arm64.dmg', 'windows-x86_64.zip', 'linux-x86_64.tar.gz', 'linux-aarch64.tar.gz']) assert.ok(html.includes(`https://zeron.sh/releases/zeron-0.2.66-${file}`));
  assert.ok(html.includes('id="downloads"'));
  assert.ok(html.includes('href="#downloads">All downloads'));
});
