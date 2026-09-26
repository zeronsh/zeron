import os

/// Signposts for Instruments / XCTOSSignpostMetric: every hot path on the
/// transcript's main-thread budget is an interval.
enum Signposts {
    static let transcript = OSSignposter(subsystem: "sh.zeron.ios", category: "transcript")
}
