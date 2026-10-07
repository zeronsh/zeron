# Android release process (in-app updater contract)

The app's updater (`apps/android/app/src/main/java/sh/zeron/android/core/Updater.kt`)
reads `https://api.github.com/repos/villatothesea/zeron-android-app/releases/latest`
without authentication (the repo is public; an optional token in
Settings → Check for Updates → Advanced only raises the API rate limit).

A release is picked up when:

1. **Tag = versionName**, e.g. `round6`, or `round6-1` for a patch. Bump
   `apps/android/version.properties` first; `versionCode` must strictly increase
   (`roundN` → `N*100`, `roundN-P` → `N*100 + P`).
2. **One APK asset**, named `zeron-android-<tag>.apk` from round5 on. Any `*.apk`
   asset is accepted as a fallback (round4 shipped `zeron-android-debug.apk`).
3. **Release notes contain a line `versionCode: <n>`**. If missing, the updater
   derives it from the tag as above.
4. **Signed with the stable release key.** `build-apk.sh` uses
   `~/zeron-keys/keystore.properties` (or `$ZERON_KEYSTORE_PROPERTIES`). The key
   never goes into the repo; without it the build falls back to the debug key and
   the system installer will refuse to update over a release build.
5. **Rate limits:** the unauthenticated API allows 60 calls/hour per IP. When it
   answers 403/429, the updater falls back to `github.com/<repo>/releases/latest`
   (a redirect to the latest tag) and the predictable asset URL
   `releases/download/<tag>/zeron-android-<tag>.apk`, so rules 1 and 2 matter.
6. **Mirrors (round5-7 on):** when GitHub fails or is slow, the check and the
   download go through public prefix proxies (`UpdateSources.BUILT_IN_MIRRORS`:
   `https://ghfast.top/` etc. + the full GitHub URL). Nothing to publish there,
   but the asset must be the release's only APK so its API `digest` (SHA-256)
   matches what the mirror serves, and it must be signed with the release key:
   the app rejects a download whose signing certificate differs from its own.

```bash
scripts/android/build-apk.sh
# With the release key this is the non-debuggable release variant (round5-3 on).
cp apps/android/app/build/outputs/apk/release/app-release.apk /tmp/zeron-android-round6.apk
gh release create round6 /tmp/zeron-android-round6.apk \
  --repo villatothesea/zeron-android-app --title round6 \
  --notes $'What changed...\n\nversionCode: 600'
```

To test the updater, build a lower version, install it with `adb install -r -d`, then
run Settings → Check for Updates:
`ZERON_VERSION_CODE=1 ZERON_VERSION_NAME=test scripts/android/build-apk.sh`.
