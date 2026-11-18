/**
 * The publish pipeline's heavy steps on one package, for
 * scripts/store-package-limit.mjs to measure in a real workerd: read the
 * body, verify it as a device would (store/package.ts), read its listing,
 * classify every file, run the checks, and store the package and the
 * version's files object in R2. (D1 and the user directory are small reads
 * and writes; they're left out.)
 */
import { fileKind, isText, runChecks } from "../../src/store/checks";
import { consentItem } from "../../src/store/consent";
import { checkListing, listingOf } from "../../src/store/listing";
import { verifyPackage } from "../../src/store/package";
import { readCapped } from "../../src/read-capped";

interface Env {
  readonly PLUGIN_STORE: R2Bucket;
}

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const started = Date.now();
    const body = await readCapped(request, 80 * 1024 * 1024);
    if (!body) return new Response("too large", { status: 413 });
    const pkg = await verifyPackage(body, 80 * 1024 * 1024);
    const { problems, warnings } = checkListing(pkg.manifest, pkg.files, isText);
    if (problems.length) return new Response(problems[0]!.words, { status: 400 });
    const listing = listingOf(pkg.manifest, pkg.files);
    if ("rule" in listing) return new Response(listing.words, { status: 400 });
    const kinds = new Map([...pkg.files].map(([path, bytes]) => [path, fileKind(path, bytes)]));
    const checks = runChecks({ pkg, listing, warnings, via: "session", consent: consentItem(pkg.manifest), base: null, previous: null });
    // `?r2=0`: leave R2 out, to tell the pipeline's memory from local R2's
    // (Miniflare runs its R2 inside the same workerd process).
    if (new URL(request.url).searchParams.get("r2") !== "0") await Promise.all([
      env.PLUGIN_STORE.put(`pkg/${pkg.sha256}`, body, { sha256: pkg.sha256 }),
      env.PLUGIN_STORE.put(`files/${pkg.sha256}`, pkg.blob)
    ]);
    return Response.json({ bytes: body.length, files: pkg.files.size, kinds: kinds.size, checks: checks.length, wallMs: Date.now() - started });
  }
};
