import XCTest
import UIKit
import PDFKit
@testable import Zeron

@MainActor
final class NativeWorkspaceArtifactTests: XCTestCase {
    private let html = """
    <style>html,body{margin:0;background:white}canvas{display:block}</style><canvas width="320" height="240"></canvas>
    <script>
    const c=document.querySelector('canvas'),x=c.getContext('2d'),d=x.createImageData(320,240);
    for(let py=0;py<240;py++)for(let px=0;px<320;px++){
      const cr=-2.5+px*3.5/320,ci=-1.3+py*2.6/240;let a=0,b=0,n=0;
      while(a*a+b*b<=4&&n<100){const t=a*a-b*b+cr;b=2*a*b+ci;a=t;n++}
      const i=(py*320+px)*4;d.data[i]=n===100?0:20;d.data[i+1]=n===100?0:70;d.data[i+2]=n===100?0:220;d.data[i+3]=255;
    }x.putImageData(d,0,0);
    </script>
    """

    func testComputedMandelbrotProducesPersistentPDFAndPNG() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let shell = MobileShellRuntime(checkpointURL: root.appendingPathComponent("workspace.json"))
        try await shell.writeFile("/workspace/mandelbrot.html", content: html)
        for format in ["pdf", "png"] {
            let result = try await shell.execute("render /workspace/mandelbrot.html /workspace/mandelbrot.\(format) 320 240; ls /workspace/mandelbrot.\(format)")
            XCTAssertEqual(result.exitCode, 0, result.stderr)
            XCTAssertTrue(result.stdout.contains("Saved /workspace/mandelbrot.\(format)"), result.stdout)
            XCTAssertTrue(result.changedPaths.contains("/workspace/mandelbrot.\(format)"))
        }
        await shell.cancel()
        let restored = MobileShellRuntime(checkpointURL: root.appendingPathComponent("workspace.json"))
        let entries = try await restored.snapshot()
        let pdfData = try XCTUnwrap(Data(base64Encoded: XCTUnwrap(entries.first { $0.path.hasSuffix(".pdf") }?.content)))
        let pdf = try XCTUnwrap(PDFDocument(data: pdfData))
        XCTAssertEqual(pdf.pageCount, 1)
        XCTAssertEqual(pdf.page(at: 0)?.bounds(for: .mediaBox).size, CGSize(width: 320, height: 240))
        let png = try XCTUnwrap(Data(base64Encoded: XCTUnwrap(entries.first { $0.path.hasSuffix(".png") }?.content)))
        let cgImage = try XCTUnwrap(UIImage(data: png)?.cgImage)
        XCTAssertEqual(cgImage.width, 320); XCTAssertEqual(cgImage.height, 240)
        var pixels = [UInt8](repeating: 0, count: 320 * 240 * 4)
        let context = try XCTUnwrap(CGContext(data: &pixels, width: 320, height: 240, bitsPerComponent: 8, bytesPerRow: 320 * 4, space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue))
        context.draw(cgImage, in: CGRect(x: 0, y: 0, width: 320, height: 240))
        let inside = (120 * 320 + 229) * 4, outside = (10 * 320 + 10) * 4
        XCTAssertLessThan(pixels[inside], 10, "Mandelbrot interior is black, not a blank document")
        XCTAssertGreaterThan(pixels[outside + 2], 150, "Computed exterior is blue")
        await restored.cancel()
    }

    func testImageImportRebasesPrivatePathAndRejectsSymlinks() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let images = root.appendingPathComponent("generated_images/thread")
        try FileManager.default.createDirectory(at: images, withIntermediateDirectories: true)
        let png = UIGraphicsImageRenderer(size: CGSize(width: 2, height: 2)).pngData { context in UIColor.red.setFill(); context.fill(CGRect(x: 0, y: 0, width: 2, height: 2)) }
        try png.write(to: images.appendingPathComponent("call_example.png"))
        let shell = MobileShellRuntime(checkpointURL: root.appendingPathComponent("workspace.json"))
        shell.generatedImagesDirectory = images
        let result = try await shell.execute("import_image /old/container/generated_images/thread/call_example.png /workspace/saved.png; ls /workspace/*.png")
        XCTAssertEqual(result.exitCode, 0, result.stderr)
        XCTAssertTrue(result.stdout.contains("/workspace/saved.png"))
        let saved = try await shell.snapshot().first { $0.path == "/workspace/saved.png" }
        XCTAssertEqual(saved?.content, png.base64EncodedString())
        try FileManager.default.createSymbolicLink(at: images.appendingPathComponent("bad.png"), withDestinationURL: images.appendingPathComponent("call_example.png"))
        XCTAssertThrowsError(try NativeWorkspaceArtifacts.read("bad.png", directory: images))
        let invalid = try await shell.execute("import_image /private/auth.json /workspace/no.png")
        XCTAssertNotEqual(invalid.exitCode, 0)
        await shell.cancel()
    }

    func testRendererHasNoWorkspaceBridgeAndBlocksNetwork() async throws {
        let html = """
        <style>body{margin:0}</style><canvas width="16" height="16"></canvas><script>
        window.zeronReady=(async()=>{
          if(window.webkit?.messageHandlers?.workspace)throw new Error('Privileged bridge exposed');
          let blocked=false;try{await fetch('https://example.invalid/')}catch{blocked=true}
          if(!blocked)throw new Error('Network was allowed');
          const x=document.querySelector('canvas').getContext('2d');x.fillStyle='#00ff00';x.fillRect(0,0,16,16);
        })();
        </script>
        """
        let png = try await NativeWorkspaceRenderer().render(html: html, width: 16, height: 16, format: "png")
        let cg = try XCTUnwrap(UIImage(data: png)?.cgImage)
        var pixel = [UInt8](repeating: 0, count: 4)
        let context = try XCTUnwrap(CGContext(data: &pixel, width: 1, height: 1, bitsPerComponent: 8, bytesPerRow: 4, space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue))
        context.draw(cg, in: CGRect(x: 0, y: 0, width: 1, height: 1))
        XCTAssertLessThan(pixel[0], 10); XCTAssertGreaterThan(pixel[1], 240); XCTAssertLessThan(pixel[2], 10)
    }

    func testRenderTimeoutCannotCommitAndNextCommandWorks() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let shell = MobileShellRuntime(checkpointURL: root.appendingPathComponent("workspace.json"))
        try await shell.writeFile("/workspace/wait.html", content: "<script>window.zeronReady=new Promise(()=>{});</script>")
        let result = try await shell.execute("render /workspace/wait.html /workspace/never.png 32 32")
        XCTAssertNotEqual(result.exitCode, 0)
        let entries = try await shell.snapshot()
        XCTAssertFalse(entries.contains { $0.path == "/workspace/never.png" })
        let next = try await shell.execute("echo ready")
        XCTAssertEqual(next.stdout, "ready\n")
        await shell.cancel()
    }
}
