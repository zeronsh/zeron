// The identity lane's fifth probe, of the 58fc12caf rules at edge f0631a108 / engine 07e926df0,
// kept as written but for one case the coordinator's 9207b0362 rules the other way:
// "认证插件" is also "authentication plugin" (双因素认证插件), so bare 认证 is a claim
// only beside "zeron" (standing_other), never by itself.
import { describe, expect, it } from "vitest";
import { markClaim } from "./listing";

// (Its Zeron cases went with claims_zeron: the owner's ruling, fa75e5a40.)
describe("the mark in other languages, as people write it", () => {
  for (const text of [
    "Официальной", // Официальной
    "для официальных", // для официальных
    "Официальную", // Официальную
    "Проверен", // Проверен
    "проверенных", // проверенных
    "Verifiсado", // Cyrillic с
    "0ficial",
    "公認プラグイン", // 公認プラグイン
    "공인 플러그인", // 공인 플러그인
  ])
    it(`refuses ${JSON.stringify(text)}`, () => expect(markClaim(text)).toBeDefined());
});

describe("honest CJK that says 'formula', not 'official' (139146b19)", () => {
  it("keeps \"认证插件\": bare 认证 is also \"authentication\" (9207b0362)", () => expect(markClaim("认证插件")).toBeUndefined());
  for (const text of ["数学公式", "公式编辑器", "수학 공식", "공식 계산기"])
    it(`keeps ${JSON.stringify(text)}`, () => expect(markClaim(text)).toBeUndefined());
});
