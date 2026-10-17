import XCTest
@testable import Zeron

final class TranscriptImageTests: XCTestCase {
    @MainActor
    func testRustImageLayoutCrossesTheSwiftBridge() async throws {
        let ready = expectation(description: "image layout")
        var transcript: TranscriptView!
        var fulfilled = false
        let listener = FrameRelay {
            if transcript.frame().rowCount() == 1, !fulfilled {
                fulfilled = true
                ready.fulfill()
            }
        }
        transcript = TranscriptView(text: TextEngine.shared, listener: listener)
        defer { transcript.close() }
        transcript.setViewport(width: 390, textScale: 1)
        transcript.setDebugEntries(entries: [DebugEntry(
            id: "image", user: false,
            text: "![Calculation result: 8](/workspace/calculator/5-plus-3.png)",
            streaming: false
        )], working: false)
        await fulfillment(of: [ready], timeout: 5)
        let frame = transcript.frame()
        let display = try XCTUnwrap(frame.display(index: 0))
        let widget = try XCTUnwrap(display.widgets.first)
        XCTAssertEqual(widget.kind, .image(reference: "/workspace/calculator/5-plus-3.png"))
        XCTAssertEqual(widget.payload, "Calculation result: 8")
        XCTAssertLessThanOrEqual(widget.x + widget.w, display.width)
        XCTAssertLessThanOrEqual(widget.y + widget.h, display.height)
        let fonts = StyleFonts()
        fonts.update(frame.styles())
        let row = RowView(frame: .zero)
        let delegate = ImageDelegate()
        row.delegate = delegate
        row.configure(RowModel(display: display, fonts: fonts), kind: .markdown)
        let image = try XCTUnwrap(row.accessibilityElements?.compactMap { $0 as? TranscriptImageView }.first)
        XCTAssertEqual(image.accessibilityLabel, "Calculation result: 8")
        XCTAssertEqual(delegate.reads, ["/workspace/calculator/5-plus-3.png"])
        XCTAssertTrue(image.accessibilityActivate())
        XCTAssertEqual(delegate.reads, ["/workspace/calculator/5-plus-3.png", "/workspace/calculator/5-plus-3.png"])
    }

    @MainActor
    func testLoadingFailureAndRetryRemainAccessible() {
        let view = TranscriptImageView(label: "Calculation result: 8")
        XCTAssertEqual(view.accessibilityValue, "Loading image")
        XCTAssertNil(view.image)
        view.showFailure()
        XCTAssertTrue(view.accessibilityValue?.contains("Image unavailable") == true)
        XCTAssertEqual(view.accessibilityLabel, "Calculation result: 8")
        var retries = 0
        view.onActivate = {
            retries += 1
            view.showLoading()
        }
        XCTAssertTrue(view.accessibilityActivate())
        XCTAssertEqual(retries, 1)
        XCTAssertEqual(view.accessibilityValue, "Loading image")
    }

    @MainActor
    func testLoadedImageKeepsAltTextAndOpens() {
        let view = TranscriptImageView(label: "Calculation result: 8")
        let image = UIGraphicsImageRenderer(size: CGSize(width: 20, height: 10)).image { _ in }
        view.showImage(image)
        XCTAssertTrue(view.image === image)
        XCTAssertEqual(view.contentMode, .scaleAspectFit)
        XCTAssertEqual(view.accessibilityLabel, "Calculation result: 8")
        XCTAssertEqual(view.accessibilityValue, "Image loaded")
        var opened = false
        view.onActivate = { opened = true }
        XCTAssertTrue(view.accessibilityActivate())
        XCTAssertTrue(opened)
    }

    @MainActor
    func testImageCacheSeparatesHostsWithTheSamePath() {
        let path = "/workspace/calculator/5-plus-3.png"
        XCTAssertNotEqual(
            CoreSessionSource.imageCacheKey(device: "host-a", reference: path),
            CoreSessionSource.imageCacheKey(device: "host-b", reference: path)
        )
        XCTAssertNotEqual(
            CoreSessionSource.imageCacheKey(device: "a", reference: "bc"),
            CoreSessionSource.imageCacheKey(device: "ab", reference: "c")
        )
    }
}

private final class ImageDelegate: RowViewDelegate {
    var reads: [String] = []
    func rowView(_ view: RowView, toggle key: UInt64) {}
    func rowView(_ view: RowView, toggleDetail detail: UInt64, open: Bool) {}
    func rowView(_ view: RowView, open url: URL) {}
    func rowView(_ view: RowView, imageFor reference: String, into imageView: UIImageView) {
        reads.append(reference)
        imageView.accessibilityIdentifier = reference
        (imageView as? TranscriptImageView)?.showFailure()
    }
}
