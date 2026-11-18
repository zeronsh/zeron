import { writeFileSync } from "node:fs";
let seed = Number(process.argv[3] ?? 1);
const rnd = () => { seed = (seed * 1103515245 + 12345) & 0x7fffffff; return seed / 0x80000000; };
const pick = <T,>(a: T[]): T => a[Math.floor(rnd() * a.length)]!;
const records: Uint8Array[] = [];
const enc = new TextEncoder();
const add = (x: string | Uint8Array) => records.push(typeof x === "string" ? enc.encode(x) : x);
const latin = (s: string) => Uint8Array.from([...s].map((c) => c.charCodeAt(0)));
// --- hand-written
const M = '{"id":"a","signature":{"key":"k","files":{},"sig":"s"}}';
const hand = [
  "", " ", "{}", "[]", "\uFEFF{}", '{"format":"zeron-plugin:v1","manifest":{},"files":{}}',
  '{"files":{"a":"QQ=="},"files":{"b":"Qg=="}}', '{"files":{"a":"QQ=="},"\\u0066iles":{"b":"Qg=="}}',
  '{"\\u0066iles":{"b":"Qg=="},"files":{"a":"QQ=="}}', '{"fil\\u0065s":1,"files":{"a":""}}', '{"files":{"a":""},"fil\\u0065s":1}',
  '{"files":{"a":"QQ==","a":"Qg=="}}', '{"files":{"a":"QQ==","\\u0061":"Qg=="}}', '{"files":{"\\u0061":"QQ==","a":"Qg=="}}',
  '{"files":{"x":{"files":{"a":"QQ=="}}}}', '{"x":{"files":{"a":"QQ=="},"manifest":{}},"files":{}}',
  '{"manifest":' + M + ',"manifest":{"id":"b"}}', '{"manifest":{"a":1,"a":2}}', '{"manifest":{"a":{"b":1},"a":{"c":2}}}',
  '{"format":"zeron-plugin:v1","format":"x"}', '{"format":"x","format":"zeron-plugin:v1"}', '{"format":"zeron\\u002dplugin:v1"}',
  '{"format":"zeron-plugin:v1\\u0000"}', '{"\\uFEFFfiles":{"a":""}}', '{"\uFEFFfiles":{"a":""}}', '{"files\\u0000":{"a":""}}',
  '{"Files":{}}', '{"files ":{}}', '{"files":{"e\\u0301":"","\u00e9":"","é":"QQ=="}}',
  '{"files":{"a":"\\u0051\\u0051\\u003d\\u003d"}}', '{"files":{"a":"QQ\\u003d="}}', '{"files":{"a":"Q\\/=="}}', '{"files":{"a":"\\/\\/\\/\\/"}}',
  '{"files":{"a":"QQ==\\n"}}', '{"files":{"a":"Q Q=="}}', '{"files":{"a":"QR=="}}', '{"files":{"a":"QUE="}}', '{"files":{"a":"QUF="}}', '{"files":{"a":"QQ"}}', '{"files":{"a":"QQ="}}', '{"files":{"a":"QQ==="}}', '{"files":{"a":"=QQ="}}', '{"files":{"a":"QQ==QQ=="}}', '{"files":{"a":"===="}}', '{"files":{"a":"A==="}}', '{"files":{"a":"-_-_"}}', '{"files":{"a":"\\ud83d\\ude00"}}', '{"files":{"a":"\\u00c1AAA"}}',
  '{"a":1} x', '{"a":1}\u0000', '{"a":1}\n', '{"a":1}\f', '{"a":1}\u00a0', '{"a":1,}', '{,"a":1}', '{"a" 1}', '{"a":}', '{"a":1', '{"a":[1,]}', '{"a":[,1]}', '{"a":[1 2]}', '{"a" : 1 , "b" : [ ] }',
  '{"a":"\\ud800"}', '{"a":"\\udc00"}', '{"a":"\\ud800\\ud800"}', '{"a":"\\ud800\\udc00"}', '{"a":"\\ud800x"}', '{"a":"\\ud800\\u0041"}', '{"\\ud800":1}', '{"a":"\\uDBFF\\uDFFF"}', '{"a":"\\u12"}', '{"a":"\\u12G4"}', '{"a":"\\x"}', '{"a":"\\\'"}',
  '{"a":"\x01"}', '{"a":"\x1f"}', '{"a":"\x7f"}', '{"a":"\t"}', '{"\n":1}',
  '{"a":-0}', '{"a":-0.0}', '{"a":01}', '{"a":-01}', '{"a":1.}', '{"a":.1}', '{"a":+1}', '{"a":1e}', '{"a":1e+}', '{"a":1E5}', '{"a":-}', '{"a":0x1}', '{"a":1e308}', '{"a":1e309}', '{"a":-1e309}', '{"a":1.7976931348623157e308}', '{"a":1.7976931348623158e308}', '{"a":1.7976931348623159e308}', '{"a":17976931348623158e292}', '{"a":17976931348623159e292}', '{"a":0.17976931348623158e309}', '{"a":1e-400}', '{"a":0e999999999999999}', '{"a":1e999999999999999}', '{"a":1e-999999999999999}', '{"a":0.0e400}', '{"a":18446744073709551615}', '{"a":18446744073709551616}', '{"a":-9223372036854775809}', '{"a":99999999999999999999}', '{"a":1e2147483647}', '{"a":1e2147483648}', '{"a":1e-2147483648}', '{"a":1e-2147483649}',
  '{"a":1' + "0".repeat(300) + 'e8}', '{"a":1' + "0".repeat(300) + 'e9}', '{"a":1' + "0".repeat(400) + 'e-92}', '{"a":1' + "0".repeat(400) + 'e-91}', '{"a":0.' + "0".repeat(400) + '1e709}', '{"a":0.' + "0".repeat(400) + '1e710}', '{"a":1' + "0".repeat(310) + '}', '{"a":1' + "0".repeat(300) + 'e2147483000}', '{"a":1' + "0".repeat(300) + 'e-2147483000}', '{"a":1e2147483300}', '{"a":0.' + "0".repeat(300) + '1e2147483647}', '{"a":' + "9".repeat(5000) + 'e-4692}', '{"a":' + "9".repeat(5000) + 'e-4691}', '{"a":17976931348623158' + "0".repeat(900) + '1e-600}',
  '{"a":179769313486231580793728971405303415079934132710037826936173778980444968292764750946649017977587207096330286416692887910946555547851940402630657488671505820681908902000708383676273854845817711531764475730270069855571366959622842914819860834936475292719074168444365510704342711559699508093042880177904174497791e-1}',
  '{"a":179769313486231580793728971405303415079934132710037826936173778980444968292764750946649017977587207096330286416692887910946555547851940402630657488671505820681908902000708383676273854845817711531764475730270069855571366959622842914819860834936475292719074168444365510704342711559699508093042880177904174497792e-1}',
  '{"a":tru}', '{"a":nulll}', '{"a":True}', '{"a":true}', '{"a":nul}',
];
for (const h of hand) add(h);
// raw byte cases
add(latin('{"a":"\xc0\x80"}')); add(latin('{"a":"\xed\xa0\x80"}')); add(latin('{"a":"\xf4\x90\x80\x80"}')); add(latin('{"a":"\xe0\x9f\xbf"}')); add(latin('{"a":"\xc3"}')); add(latin('{"a":"\xff"}')); add(latin('{"a":1}\xff')); add(latin('\xef\xbb\xbf{"a":1}')); add(latin('{"a":"\xf0\x9f\x98\x80"}')); add(latin('{"\xc3\xa9":1}')); add(latin('{"a":"\x80"}'));
// depth
for (const d of [125, 126, 127, 128, 129, 130]) {
  add("[".repeat(d) + "]".repeat(d)); add("{\"a\":".repeat(d) + "1" + "}".repeat(d));
  add('{"x":' + "[".repeat(d - 1) + "]".repeat(d - 1) + "}");
  add('{"manifest":{"a":' + "[".repeat(d - 2) + "]".repeat(d - 2) + "}}");
  add('{"files":{"a":' + "[".repeat(d - 2) + "]".repeat(d - 2) + "}}");
}
// --- generated
const nums = ["0","-0","1","-1","1.0","1e2","1E2","1e+2","1e-2","0.1","-0.0","18446744073709551615","18446744073709551616","-9223372036854775808","-9223372036854775809","1e400","-1e400","1e-400","00","01","1.","1.e1",".5","+1","2.5e-324","123456789012345678901234567890","0e0","1e0000000000000000002","1.7976931348623158e308","1.7976931348623159e308","1e308","9.9e307","99.9e307"];
const b64 = ['"QQ=="', '"QUI="', '"QUJD"', '""', '"QR=="', '"QQ"', '"Q\\/=="', '"\\u0051\\u0051=="', '"QQ=\\u003d"', '"QQ==\\n"', '"QUJ\\/"', '"++//"', '"-_"', '1', 'null', '{}', '"QQ==="', '"\\u0000"'];
const strs = ['"a"','"\\u00e9"','"é"','"\\ud83d\\ude00"','"😀"','"\\ud83d"','"\\ude00"','"\\u0000"','"\x7f"','"\\/"','"\\b\\f\\n\\r\\t"','"\\x"','"\\uD83D\\uDE00"','"\\ud800\\u0041"','"\uFEFF"','"a\tb"','"\\u00"'];
const topKeys = ['"format"','"manifest"','"files"','"\\u0066iles"','"fil\\u0065s"','"\\u0066\\u0069\\u006c\\u0065\\u0073"','"m\\u0061nifest"','"\\u006danifest"','"f\\u006frmat"','"Files"','"files "','"\\uFEFFfiles"','"x"','"files\\u0000"','"\\/files"','"fi\\les"','"files', 'files'];
const fileKeys = ['"a"','"b"','"\\u0061"','"a/b"','"é"','"e\\u0301"','"\\u00e9"','"😀"','"\\ud83d\\ude00"','"\uffff"','""','"\ue000"','"manifest.json"'];
const ws = ["", " ", "\n", "\t", "\r", "\u00a0", "\f"];
const val = (d: number): string => {
  const r = rnd();
  if (d > 3 || r < 0.35) return pick(nums);
  if (r < 0.6) return pick(strs);
  if (r < 0.65) return pick(["true","false","null","tru","nul"]);
  if (r < 0.8) { const n = Math.floor(rnd()*4); return "[" + Array.from({length:n}, () => pick(ws)+val(d+1)+pick(ws)).join(",") + (rnd()<0.03?",":"") + "]"; }
  const n = Math.floor(rnd()*4); return "{" + Array.from({length:n}, () => pick(ws)+pick([...fileKeys, '"files"','"manifest"'])+pick(ws)+":"+val(d+1)).join(",") + (rnd()<0.03?",":"") + "}";
};
const filesObj = () => { const n = Math.floor(rnd()*5); return "{" + Array.from({length:n}, () => pick(ws)+pick(fileKeys)+pick(ws)+":"+pick(ws)+(rnd()<0.85?pick(b64):val(2))).join(",") + "}"; };
const member = () => {
  const k = pick(topKeys); const r = rnd();
  const v = r < 0.35 ? filesObj() : r < 0.6 ? (rnd() < 0.5 ? '"zeron-plugin:v1"' : pick(['"zeron-plugin:v2"','"zeron\\u002dplugin:v1"','1'])) : val(1);
  return pick(ws) + k + pick(ws) + ":" + pick(ws) + v;
};
const N = Number(process.argv[4] ?? 100000);
if (process.argv[5] === "valid") {
  const keepNum = ["0","-0","1","-1","1.0","1e2","1E2","1e+2","1e-2","0.1","-0.0","18446744073709551615","18446744073709551616","-9223372036854775808","-9223372036854775809","1e-400","2.5e-324","123456789012345678901234567890","0e0","1e0000000000000000002","1.7976931348623158e308","1e308","9.9e307","99.9e307"];
  nums.splice(0, nums.length, ...keepNum);
  strs.splice(0, strs.length, ...strs.filter((x) => !['"\\ud83d"','"\\ude00"','"\\x"','"\\ud800\\u0041"','"a\tb"','"\\u00"'].includes(x)));
  topKeys.splice(0, topKeys.length, ...topKeys.filter((x) => x.startsWith('"') && x.endsWith('"') && x !== '"fi\\les"'));
  ws.splice(0, ws.length, "", " ", "\n", "\t", "\r");
}
const docs: string[] = [];
for (let i = 0; i < N; i++) {
  const n = 1 + Math.floor(rnd() * 5);
  let s = (rnd() < 0.01 ? "\uFEFF" : "") + pick(ws) + "{" + Array.from({ length: n }, member).join(",") + (rnd() < 0.02 ? "," : "") + "}" + pick(ws) + (rnd() < 0.01 ? pick(["x", "1", "{}"]) : "");
  docs.push(s); add(s);
}
// byte mutations of generated docs
for (let i = 0; i < N; i++) {
  const b = enc.encode(pick(docs));
  const c = Array.from(b);
  const m = 1 + Math.floor(rnd() * 3);
  for (let k = 0; k < m; k++) {
    const p = Math.floor(rnd() * (c.length + 1)); const op = rnd();
    const byte = pick([0x22, 0x5c, 0x7b, 0x7d, 0x5b, 0x5d, 0x2c, 0x3a, 0x30, 0x2d, 0x65, 0x2e, 0x75, 0x00, 0x1f, 0x80, 0xc3, 0xff, 0x20, Math.floor(rnd()*256)]);
    if (op < 0.33) c.splice(p, 0, byte); else if (op < 0.66) c.splice(p, 1); else c[p] = byte;
  }
  add(Uint8Array.from(c));
}
let total = 0; for (const r of records) total += 4 + r.length;
const buf = Buffer.alloc(total); let at = 0;
for (const r of records) { buf.writeUInt32LE(r.length, at); buf.set(r, at + 4); at += 4 + r.length; }
writeFileSync(process.argv[2]!, buf);
console.log("records", records.length, "hand", hand.length);
