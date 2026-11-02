// zeron-escape-monitor: Zeron's computer-use kill switch, ported from
// Synara's EscapeKillSwitchMonitor. A passive (listen-only) event tap watches
// for the human's physical Escape key while an agent is driving the computer
// and prints `escape` on stdout; the bridge then interrupts the driver's
// native input. It never blocks, alters or reads other keys: only a plain,
// hardware-originated Escape produces output, so the driver's own synthetic
// Escape (or any other process's) stays an ordinary keystroke.
//
// Usage: zeron-escape-monitor
// Status lines: `ready`, `escape`, `error <code>: <message>`. Exits when
// stdin closes (the bridge disarmed it) or the parent dies.

import AppKit
import CoreGraphics
import Darwin
import Foundation

func status(_ line: String) {
    FileHandle.standardOutput.write((line + "\n").data(using: .utf8)!)
}

let escapeKeyCode: Int64 = 0x35
/// Modifiers that make the press a chord rather than the plain key. Caps Lock
/// and Fn do not change which physical key was pressed.
let disqualifying: CGEventFlags = [.maskCommand, .maskAlternate, .maskControl, .maskShift]

/// Synara's EscapePhysicalClassifier: all three source fields must agree that
/// no process posted the event. A process-posted event carries its pid, its
/// own source state table, and possibly a user-data marker.
func isPhysicalEscape(_ type: CGEventType, _ event: CGEvent) -> Bool {
    type == .keyDown
        && event.getIntegerValueField(.keyboardEventKeycode) == escapeKeyCode
        && event.flags.intersection(disqualifying).isEmpty
        && event.getIntegerValueField(.keyboardEventAutorepeat) == 0
        && event.getIntegerValueField(.eventSourceUnixProcessID) == 0
        && event.getIntegerValueField(.eventSourceStateID)
            == Int64(CGEventSourceStateID.hidSystemState.rawValue)
        && event.getIntegerValueField(.eventSourceUserData) == 0
}

var tap: CFMachPort?

let callback: CGEventTapCallBack = { _, type, event, _ in
    if type == .tapDisabledByTimeout || type == .tapDisabledByUserInput {
        // macOS paused the listener; resume it rather than going deaf.
        if let tap { CGEvent.tapEnable(tap: tap, enable: true) }
    } else if isPhysicalEscape(type, event) {
        status("escape")
    }
    return Unmanaged.passUnretained(event)
}

func install() -> Bool {
    let mask = CGEventMask(1) << CGEventType.keyDown.rawValue
    guard let created = CGEvent.tapCreate(
        tap: .cgSessionEventTap,
        place: .headInsertEventTap,
        options: .listenOnly,
        eventsOfInterest: mask,
        callback: callback,
        userInfo: nil
    ), let source = CFMachPortCreateRunLoopSource(kCFAllocatorDefault, created, 0) else {
        return false
    }
    tap = created
    CFRunLoopAddSource(CFRunLoopGetMain(), source, .commonModes)
    CGEvent.tapEnable(tap: created, enable: true)
    return true
}

// The event tap needs a Window Server connection, like the indicator.
NSApplication.shared.setActivationPolicy(.prohibited)

// Kept at top level so the timer lives as long as the process.
let retry = DispatchSource.makeTimerSource(queue: .main)
if install() {
    status("ready")
} else {
    // A listen-only tap needs Input Monitoring (or Accessibility). Keep
    // retrying: a grant made in System Settings takes effect without a
    // restart of the agent.
    status("error input_monitoring_required: Input Monitoring is not granted to Zeron.")
    retry.schedule(deadline: .now() + 5, repeating: 5)
    retry.setEventHandler {
        if tap == nil, install() {
            status("ready")
        }
    }
    retry.resume()
}

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
}
watchdog.resume()
NSApplication.shared.run()
