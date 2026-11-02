// zeron-window-indicator: holds a ScreenCaptureKit stream on the window the
// agent is driving, so macOS shows its capture indicator (the purple pill in
// the window's title bar) while Zeron's computer use works with that window —
// the indicator Synara's live preview raises. Frames are discarded: the
// stream exists only to make the activity visible to the user.
//
// Usage: zeron-window-indicator --window <CGWindowID> --pid <owner pid>
// Exits when stdin closes (the parent bridge stopped or retargeted), when the
// parent dies, or when the window goes away. Status lines go to stdout.

import AppKit
import CoreMedia
import Darwin
import Foundation
import ScreenCaptureKit

func status(_ line: String) {
    FileHandle.standardOutput.write((line + "\n").data(using: .utf8)!)
}

func fail(_ code: String, _ message: String) -> Never {
    status("error \(code): \(message)")
    exit(EXIT_FAILURE)
}

func argument(_ name: String) -> Int? {
    let args = CommandLine.arguments
    guard let index = args.firstIndex(of: name), index + 1 < args.count else { return nil }
    return Int(args[index + 1])
}

guard let windowArg = argument("--window"), windowArg > 0, windowArg <= Int(UInt32.max),
      let pidArg = argument("--pid"), pidArg > 0, pidArg <= Int(Int32.max)
else {
    fail("usage", "zeron-window-indicator --window <id> --pid <pid>")
}
let windowID = CGWindowID(windowArg)
let ownerPID = pid_t(pidArg)

final class Indicator: NSObject, SCStreamOutput, SCStreamDelegate {
    let queue = DispatchQueue(label: "sh.zeron.window-indicator")
    var stream: SCStream?

    func start() {
        guard CGPreflightScreenCaptureAccess() else {
            fail("screen_recording_required", "Screen Recording is not granted to Zeron.")
        }
        SCShareableContent.getExcludingDesktopWindows(true, onScreenWindowsOnly: false) { content, error in
            self.queue.async {
                guard let window = content?.windows.first(where: {
                    $0.windowID == windowID && $0.owningApplication?.processID == ownerPID
                }) else {
                    fail("target_unavailable", error?.localizedDescription ?? "window \(windowID) not found")
                }
                let configuration = SCStreamConfiguration()
                // The smallest stream that still counts as capturing the window.
                configuration.width = 64
                configuration.height = 64
                configuration.minimumFrameInterval = CMTime(value: 1, timescale: 1)
                configuration.queueDepth = 1
                configuration.showsCursor = false
                let stream = SCStream(
                    filter: SCContentFilter(desktopIndependentWindow: window),
                    configuration: configuration,
                    delegate: self
                )
                do {
                    try stream.addStreamOutput(self, type: .screen, sampleHandlerQueue: self.queue)
                } catch {
                    fail("capture_setup_failed", error.localizedDescription)
                }
                self.stream = stream
                stream.startCapture { error in
                    if let error {
                        fail("capture_start_failed", error.localizedDescription)
                    }
                    status("ready")
                }
            }
        }
    }

    func stream(_ stream: SCStream, didOutputSampleBuffer sampleBuffer: CMSampleBuffer, of type: SCStreamOutputType) {}

    func stream(_ stream: SCStream, didStopWithError error: any Error) {
        fail("capture_stopped", error.localizedDescription)
    }
}

// ScreenCaptureKit needs a Window Server connection; a background-only app
// provides it without a Dock icon or menu bar.
NSApplication.shared.setActivationPolicy(.prohibited)
let indicator = Indicator()
indicator.start()

// Lifetime: stdin EOF from the bridge, or the parent going away.
let parent = getppid()
Thread.detachNewThread {
    _ = FileHandle.standardInput.readDataToEndOfFile()
    exit(EXIT_SUCCESS)
}
let watchdog = DispatchSource.makeTimerSource(queue: .main)
watchdog.schedule(deadline: .now() + 1, repeating: 1)
watchdog.setEventHandler {
    if getppid() != parent || (kill(parent, 0) != 0 && errno == ESRCH) {
        exit(EXIT_SUCCESS)
    }
    // The window closed: nothing left to indicate.
    let windows = CGWindowListCopyWindowInfo(.optionIncludingWindow, windowID) as? [[String: Any]]
    if windows?.isEmpty ?? true {
        exit(EXIT_SUCCESS)
    }
}
watchdog.resume()
NSApplication.shared.run()
