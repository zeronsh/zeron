import XCTest
@testable import Zeron

final class RemoteVoiceLifecycleTests: XCTestCase {
    func testCancelledContinuationIgnoresLateNativeSuccess() async {
        let slot = VoiceContinuation<String>()
        slot.finish(.failure(CancellationError()))
        do {
            let _: String = try await withCheckedThrowingContinuation { continuation in
                slot.install(continuation)
                slot.finish(.success("late offer"))
            }
            XCTFail("Cancelled negotiation must not produce an offer")
        } catch { XCTAssertTrue(error is CancellationError) }
    }

    func testNativeCompletionResumesOnlyOnce() async throws {
        let slot = VoiceContinuation<Int>()
        let value = try await withCheckedThrowingContinuation { continuation in
            slot.install(continuation)
            slot.finish(.success(1))
            slot.finish(.success(2))
            slot.finish(.failure(CancellationError()))
        }
        XCTAssertEqual(value, 1)
    }

    @MainActor
    func testClosedEndpointCannotReactivateOrProduceOffer() async {
        let peer = CodexVoicePeer()
        peer.close()
        peer.close()
        XCTAssertThrowsError(try peer.setMuted(false))
        do { _ = try await peer.offer(); XCTFail("Closed peer cannot negotiate") } catch {}
        do { try await peer.apply(answer: "late answer"); XCTFail("Closed peer cannot apply answer") } catch {}
    }

    @MainActor
    func testOrbFramesMatchTheDesktopArtworkBox() {
        for orb: VoiceOrb in [.idle, .connecting, .listening, .speaking, .working, .awaitingInput, .muted] {
            let renderer = OrbRenderer(preset: .hero, orb: orb)
            renderer.setAudioLevels(microphone: 0.4, speaker: 0.9)
            let frame = renderer.nextFrame(animating: true, reducedMotion: false)
            XCTAssertEqual(frame.size, 128)
            XCTAssertFalse(frame.dots.isEmpty)
            XCTAssertEqual(frame.dots.count % 5, 0)
            XCTAssertEqual(frame.lines.count % 7, 0)
            XCTAssertTrue((frame.dots + frame.lines).allSatisfy(\.isFinite))
        }
    }

    func testCaptionShowsTheTailAndVeilsNewWords() {
        let fader = CaptionFader(maxChars: 160)
        XCTAssertEqual(fader.frame(item: "a", text: "  short  ", reducedMotion: true).text, "short")
        let long = String(repeating: "a", count: 300) + " end"
        let frame = fader.frame(item: "b", text: long, reducedMotion: false)
        XCTAssertTrue(frame.text.hasPrefix("…"))
        XCTAssertTrue(frame.text.hasSuffix(" end"))
        XCTAssertLessThanOrEqual(frame.text.count, 161)
        // A new turn: its words fade in while the previous caption fades out.
        XCTAssertTrue(frame.animating)
        XCTAssertEqual(frame.previous, "short")
        XCTAssertEqual(frame.spans.first?.alpha, 0)
    }

    @MainActor
    func testEveryEndReasonExplainsItself() {
        for reason: VoiceEndReason in [.microphoneDenied, .audioUnavailable, .signInRequired, .usageUnavailable, .busy, .hostUnavailable, .hostIncompatible, .connectionLost] {
            XCTAssertFalse(RemoteVoiceController.message(for: reason, host: "Fedora").isEmpty)
        }
    }
}
