import os

/// Signposts for Instruments / XCTOSSignpostMetric: every hot path on the
/// transcript's main-thread budget is an interval.
enum Signposts {
    static let transcript = OSSignposter(subsystem: "sh.zeron.ios", category: "transcript")
}

/// Main-thread cost counters for the transcript (read by `-bench`).
enum TranscriptPerf {
    nonisolated(unsafe) static var maxLayoutMs = 0.0
    nonisolated(unsafe) static var maxApplyMs = 0.0
    nonisolated(unsafe) static var maxSyncBuildMs = 0.0
    nonisolated(unsafe) static var syncBuilds = 0
    nonisolated(unsafe) static var asyncBuilds = 0
    nonisolated(unsafe) static var viewsCreated = 0

    static func reset() {
        maxLayoutMs = 0; maxApplyMs = 0; maxSyncBuildMs = 0
        syncBuilds = 0; asyncBuilds = 0; viewsCreated = 0
    }

    static var json: String {
        String(format: "{\"maxLayoutMs\":%.2f,\"maxApplyMs\":%.2f,\"maxSyncBuildMs\":%.2f,\"syncBuilds\":%d,\"asyncBuilds\":%d,\"viewsCreated\":%d}",
               maxLayoutMs, maxApplyMs, maxSyncBuildMs, syncBuilds, asyncBuilds, viewsCreated)
    }
}
