// Credential-free contract test against the actual pinned SDK's interceptor.
// Instrument a disposable copy of the SDK bundle to capture its transport
// options. No implementation is reimplemented and no real network is allowed.
import fs from 'node:fs';
import path from 'node:path';
import assert from 'node:assert/strict';
import {pathToFileURL} from 'node:url';
import {createRequire} from 'node:module';
const root = path.resolve(process.argv[2]);
const entry = path.join(root, 'dist/esm/index.js');
let source = fs.readFileSync(entry, 'utf8');
const moduleDefinition = source.match(/"\.\/src\/agent\/executor-common\.ts"\([^)]*\)\{/);
const start = moduleDefinition?.index ?? -1;
const end = source.indexOf('"./src/agent/native/vendored-tree-sitter.ts"', start);
assert(start >= 0 && end > start, 'SDK layout changed: review the auth contract instrumentation');
const original = source.slice(start, end);
const factory = original.match(/KU:\(\)=>(\w+)/)?.[1];
assert(factory, 'SDK auth factory export changed');
const loader = source.match(/var \w+=(\w+)\("\.\/src\/agent\/executor-common\.ts"\)/)?.[1];
assert(loader, 'SDK loader changed');
const patched = original.replace(/\(0,\w+\.cf\)/g, '(0,globalThis.__cursorTestTransport)');
assert.notEqual(patched, original, 'SDK transport constructor changed');
source = source.slice(0,start) + patched + source.slice(end);
source += `\nexport const __testAuthFactory=${loader}("./src/agent/executor-common.ts").KU;\n`;
const copy = path.join(root, 'dist/esm', `.zeron-auth-contract-${process.pid}.mjs`);
fs.writeFileSync(copy, source);
let captured;
globalThis.__cursorTestTransport = options => {
  captured = options;
  return {unary(){throw Error('Unexpected transport call');},stream(){throw Error('Unexpected transport call');}};
};
let now = 1800000000000;
Date.now = () => now;
let exchanges = 0;
let exchangeStatus = 200;
globalThis.fetch = async (url) => {
  assert(String(url).endsWith('/auth/exchange_user_api_key'), 'Unexpected network request');
  exchanges++;
  if (exchangeStatus !== 200) return new Response('injected exchange failure', {status:exchangeStatus});
  const token = `test.${Buffer.from(JSON.stringify({exp:now/1000+3600,nonce:exchanges})).toString('base64url')}.test`;
  return new Response(JSON.stringify({accessToken:token}));
};
try {
  const sdk = await import(pathToFileURL(copy));
  sdk.__testAuthFactory('synthetic-api-key');
  const auth = captured.interceptors.at(-1);
  const request = (stream=false) => ({header:new Headers(),stream});
  const seen=[];
  const next=async req=>{seen.push(req.header.get('authorization'));return {stream:false};};
  await auth(next)(request());
  assert.equal(exchanges,1);
  now += 3590_000;
  await Promise.all(Array.from({length:100},()=>auth(next)(request())));
  assert.equal(exchanges,2,'must coalesce proactive refresh before token expiry');
  assert.notEqual(seen[0],seen[1],'must replace near-expiry token');
  assert.equal(new Set(seen.slice(1)).size,1);
  const require=createRequire(path.join(root,'package.json'));
  const {ConnectError,Code}=require('@connectrpc/connect');
  let streams=0;
  const response=await auth(async()=>({stream:true,message:(async function*(){streams++;throw new ConnectError('expired',Code.Unauthenticated);})()}))(request(true));
  await assert.rejects(async()=>{for await(const _ of response.message){};});
  await auth(next)(request());
  assert.equal(exchanges,3,'stream auth error must invalidate cached token');
  assert.equal(streams,1,'must not replay consumed stream');
  now += 3601_000;
  exchangeStatus=503;
  await assert.rejects(auth(next)(request()));
  exchangeStatus=200;
  await auth(next)(request());
  assert.equal(exchanges,5,'transient exchange failure must not poison cache');
  now += 3601_000;
  exchangeStatus=401;
  await assert.rejects(auth(next)(request()));
  await assert.rejects(auth(next)(request()));
  assert.equal(exchanges,6,'invalid credentials must surface without a retry storm');
  console.log(JSON.stringify({invalidCredentialsReported:true,proactiveRefresh:true,coalescedCallers:100,streamInvalidation:true,streamReplays:0,exchangeFailureRecovery:true}));
} finally {fs.unlinkSync(copy);}
