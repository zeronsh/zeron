// zgx's Titlebar::Inset window setup (window_chrome.rs mac::content_under_titlebar + inset_buttons),
// entered into full screen; lists and captures this process's own windows there.
// Usage: fsrepro [fix]   (fix: the toolbar hidden while in full screen)
import AppKit
let fix = CommandLine.arguments.contains("fix")
let out = CommandLine.arguments.last!
class Delegate: NSObject, NSWindowDelegate {
  func windowWillEnterFullScreen(_ n: Notification) { if fix { (n.object as! NSWindow).toolbar?.isVisible = false } }
  func windowDidExitFullScreen(_ n: Notification) { if fix { (n.object as! NSWindow).toolbar?.isVisible = true } }
}
let app = NSApplication.shared
app.setActivationPolicy(.regular)
let w = NSWindow(contentRect: NSRect(x: 200, y: 200, width: 900, height: 560), styleMask: [.titled, .closable, .miniaturizable, .resizable], backing: .buffered, defer: false)
w.styleMask.insert(.fullSizeContentView)
w.titlebarAppearsTransparent = true
w.titleVisibility = .hidden
w.titlebarSeparatorStyle = .none
let tb = NSToolbar(identifier: "zgx.titlebar.inset")
tb.showsBaselineSeparator = false
w.toolbar = tb
w.toolbarStyle = .unifiedCompact
let v = NSView(); v.wantsLayer = true; v.layer?.backgroundColor = NSColor(red: 0.85, green: 0.1, blue: 0.1, alpha: 1).cgColor
w.contentView = v
let d = Delegate(); w.delegate = d
w.makeKeyAndOrderFront(nil)
app.activate(ignoringOtherApps: true)
func dump(_ tag: String) {
  let list = CGWindowListCopyWindowInfo([.optionAll], kCGNullWindowID) as! [[String: Any]]
  for info in list where (info[kCGWindowOwnerPID as String] as? Int32) == getpid() {
    let b = info[kCGWindowBounds as String] as! [String: Any]
    let id = info[kCGWindowNumber as String] as! CGWindowID
    let onscreen = (info[kCGWindowIsOnscreen as String] as? Bool) ?? false
    print(tag, id, b["X"]!, b["Y"]!, b["Width"]!, b["Height"]!, "onscreen:", onscreen, "layer:", info[kCGWindowLayer as String]!, info[kCGWindowName as String] ?? "")
    if onscreen {
      let p = Process(); p.executableURL = URL(fileURLWithPath: "/usr/sbin/screencapture")
      p.arguments = ["-x", "-o", "-l", "\(id)", "\(out)_\(tag)_\(id).png"]
      try? p.run(); p.waitUntilExit()
    }
  }
}
DispatchQueue.main.asyncAfter(deadline: .now() + 0.6) { dump("windowed"); w.toggleFullScreen(nil) }
func tree(_ v: NSView, _ depth: Int) {
  let pad = String(repeating: "  ", count: depth)
  var extra = ""
  if let e = v as? NSVisualEffectView { extra = " material=\(e.material.rawValue) state=\(e.state.rawValue) blend=\(e.blendingMode.rawValue)" }
  if let l = v.layer, let bg = l.backgroundColor { extra += " layerbg=\(bg)" }
  print("\(pad)\(type(of: v)) \(v.frame) hidden=\(v.isHidden) alpha=\(v.alphaValue)\(extra)")
  if depth < 6 { for sub in v.subviews { tree(sub, depth + 1) } }
}
func strips(_ tag: String) {
  for win in NSApp.windows where win.frame.height < 100 && win.frame.width > 300 {
    print(tag, "strip window", type(of: win), win.frame, "opaque=\(win.isOpaque) alpha=\(win.alphaValue) bg=\(String(describing: win.backgroundColor)) visible=\(win.isVisible)")
    if let frameView = win.contentView?.superview ?? win.contentView {
      tree(frameView, 1)
      if let rep = frameView.bitmapImageRepForCachingDisplay(in: frameView.bounds) {
        frameView.cacheDisplay(in: frameView.bounds, to: rep)
        try? rep.representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: "\(out)_\(tag)_strip.png"))
      }
    }
  }
}
DispatchQueue.main.asyncAfter(deadline: .now() + 2.4) { dump("full"); strips("full"); print("contentLayoutRect", w.contentLayoutRect, "frame", w.frame) }
DispatchQueue.main.asyncAfter(deadline: .now() + 2.6) { w.toggleFullScreen(nil) }
DispatchQueue.main.asyncAfter(deadline: .now() + 4.2) { app.terminate(nil) }
app.run()
