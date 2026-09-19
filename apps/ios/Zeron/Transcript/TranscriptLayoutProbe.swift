// Test-only physical-window measurements, including hosted table cells.
import SwiftUI

#if DEBUG
@MainActor
enum TranscriptLayoutProbe {
    static var enabled = false
    static var tails: [String: CGRect] = [:]
    static var viewports: [String: CGRect] = [:]
    private final class WeakView {
        weak var view: UIView?
        let viewport: Bool
        init(_ view: UIView, viewport: Bool) { self.view = view; self.viewport = viewport }
    }
    private static var markers: [String: [WeakView]] = [:]
    static func register(_ view: UIView, key: String, viewport: Bool) {
        if !(markers[key] ?? []).contains(where: { $0.view === view }) {
            view.layer.name = "transcript-probe-" + UUID().uuidString
            markers[key, default: []].append(WeakView(view, viewport: viewport))
        }
    }
    static func presentedFrame(for key: String) -> CGRect? {
        presentedFrames(for: [key])[key]
    }

    /// Compare moving markers from one presentation-tree snapshot. Separate
    /// presentation() calls can straddle a compositor frame; converting each
    /// against another independently sampled root can invent an attachment gap.
    static func presentedFrames(for keys: [String]) -> [String: CGRect] {
        var windows: [ObjectIdentifier: UIWindow] = [:]
        var names: [String: String] = [:]
        for key in keys {
            guard let view = markers[key]?.last(where: { $0.view?.window != nil })?.view,
                  let window = view.window, let name = view.layer.name else { continue }
            windows[ObjectIdentifier(window)] = window
            names[name] = key
        }
        var frames: [String: CGRect] = [:]
        for window in windows.values {
            let root = window.layer.presentation() ?? window.layer
            func visit(_ layer: CALayer) {
                if let name = layer.name, let key = names[name] {
                    frames[key] = layer.convert(layer.bounds, to: root)
                }
                for child in layer.sublayers ?? [] { visit(child) }
            }
            visit(root)
        }
        return frames
    }

    static func sample() {
        for (key, candidates) in markers {
            guard let marker = candidates.last(where: { $0.view?.window != nil }),
                  let view = marker.view, let window = view.window else {
                viewports.removeValue(forKey: key)
                tails.removeValue(forKey: key)
                continue
            }
            let frame = view.convert(view.bounds, to: window)
            if marker.viewport { viewports[key] = frame }
            else { tails[key] = frame }
        }
        markers = markers.mapValues { $0.filter { $0.view != nil } }.filter { !$0.value.isEmpty }
    }
}

private struct TranscriptMeasurement: UIViewRepresentable {
    let key: String
    var viewport = false
    func makeUIView(context: Context) -> UIView {
        let view = UIView()
        view.isUserInteractionEnabled = false
        return view
    }
    func updateUIView(_ view: UIView, context: Context) {
        TranscriptLayoutProbe.register(view, key: key, viewport: viewport)
    }
}
#endif

struct TranscriptTailProbe: ViewModifier {
    let rowID: String
    let isTail: Bool
    let chatId: String
    func body(content: Content) -> some View {
        #if DEBUG
        if TranscriptLayoutProbe.enabled, isTail {
            content.background(TranscriptMeasurement(key: chatId + "|" + rowID))
        } else { content }
        #else
        content
        #endif
    }
}

struct TranscriptViewportProbe: ViewModifier {
    let chatId: String
    func body(content: Content) -> some View {
        #if DEBUG
        if TranscriptLayoutProbe.enabled {
            content.background(TranscriptMeasurement(key: chatId, viewport: true))
        } else { content }
        #else
        content
        #endif
    }
}
