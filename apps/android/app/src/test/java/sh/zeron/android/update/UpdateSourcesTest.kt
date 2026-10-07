package sh.zeron.android.update

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test
import sh.zeron.android.core.UpdateSources

class UpdateSourcesTest {
    private val gh = "https://github.com/villatothesea/zeron-android-app/releases/download/round5-7/zeron-android-round5-7.apk"

    @Test fun normalizesMirrorPrefixes() {
        assertEquals("https://ghfast.top/", UpdateSources.normalizeMirror(" ghfast.top "))
        assertEquals("https://gh-proxy.com/", UpdateSources.normalizeMirror("https://gh-proxy.com"))
        assertEquals("http://10.0.0.2:8080/", UpdateSources.normalizeMirror("http://10.0.0.2:8080/"))
        assertNull(UpdateSources.normalizeMirror("  "))
        assertNull(UpdateSources.normalizeMirror(null))
    }

    @Test fun githubFirstThenBuiltInMirrors() {
        val s = UpdateSources.downloadSources(gh, userMirror = null, preferred = null)
        assertEquals(listOf("GitHub", "ghfast.top", "gh-proxy.com", "gh.llkk.cc"), s.map { it.label })
        assertEquals(gh, s[0].url)
        assertEquals("https://ghfast.top/$gh", s[1].url)
    }

    @Test fun userMirrorLeadsAndIsNotRepeated() {
        val s = UpdateSources.downloadSources(gh, userMirror = "gh-proxy.com", preferred = null)
        assertEquals(listOf("gh-proxy.com", "GitHub", "ghfast.top", "gh.llkk.cc"), s.map { it.label })
    }

    @Test fun lastWorkingSourceMovesToTheFront() {
        val s = UpdateSources.downloadSources(gh, userMirror = null, preferred = "https://gh.llkk.cc/")
        assertEquals(listOf("gh.llkk.cc", "GitHub", "ghfast.top", "gh-proxy.com"), s.map { it.label })
        val unknown = UpdateSources.downloadSources(gh, userMirror = null, preferred = "https://gone.example/")
        assertEquals("GitHub", unknown[0].label)
    }

    @Test fun tokenUsesTheApiAssetEndpoint() {
        val api = "https://api.github.com/repos/x/y/releases/assets/1"
        val s = UpdateSources.downloadSources(gh, null, null, token = "t", apiAssetUrl = api)
        assertEquals(api, s[0].url)
        assertEquals("t", s[0].token)
        assertNull(s[1].token)
    }

    @Test fun tagFromAbsoluteAndMirrorRelativeLocations() {
        assertEquals("round5-6", UpdateSources.tagFromLocation("https://github.com/villatothesea/zeron-android-app/releases/tag/round5-6"))
        // ghfast.top rewrites the redirect to a path on itself.
        assertEquals("round5-6", UpdateSources.tagFromLocation("/https://github.com/villatothesea/zeron-android-app/releases/tag/round5-6"))
        assertEquals("round6", UpdateSources.tagFromLocation("https://github.com/o/r/releases/tag/round6?x=1"))
        assertNull(UpdateSources.tagFromLocation("https://github.com/o/r/releases"))
        assertNull(UpdateSources.tagFromLocation(null))
    }

    @Test fun parsesGithubDigest() {
        val hex = "c0165c9992565775f476266f5b35121b825fd19f6cab25fdfc478cd5b9dbf3b7"
        assertEquals(hex, UpdateSources.sha256FromDigest("sha256:$hex"))
        assertEquals(hex, UpdateSources.sha256FromDigest("SHA256:${hex.uppercase()}"))
        assertNull(UpdateSources.sha256FromDigest("sha512:abc"))
        assertNull(UpdateSources.sha256FromDigest("sha256:1234"))
        assertNull(UpdateSources.sha256FromDigest(null))
    }

    @Test fun pickerListsGithubThenBuiltInsThenYourMirror() {
        val list = UpdateSources.choices("https://github.com/a.apk", "mirror.example")
        assertEquals(listOf("GitHub", "ghfast.top", "gh-proxy.com", "gh.llkk.cc", "mirror.example"), list.map { it.label })
        assertEquals("https://mirror.example/https://github.com/a.apk", list.last().url)
        // Your mirror being a built-in one doesn't list it twice.
        assertEquals(4, UpdateSources.choices("https://github.com/a.apk", "ghfast.top").size)
    }
}
