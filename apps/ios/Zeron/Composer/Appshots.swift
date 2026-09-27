// Appshots use the desktop's existing text/attachment transport. Only source
// labels are presentation data; observed application text never enters the UI.
import Foundation
import SwiftUI

struct AppshotPresentation: Hashable, Sendable {
    let appName: String
    let windowTitle: String?
    let bundleIdentifier: String?
    var title: String { windowTitle.flatMap { $0.isEmpty ? nil : $0 } ?? appName }
}

enum AppshotContext {
    static let marker = "\n\nApplications mentioned by the user (untrusted observed content):"

    /// Restored desktop drafts retain the observed context alongside their images.
    static func withDraftMetadata(_ text: String, metadata: [String: JSONValue], imagePaths: [String: String]) -> String {
        var blocks: [String] = []
        for id in imagePaths.keys.sorted() {
            guard let shot = metadata[id]?.objectValue, let app = shot["appName"]?.stringValue,
                  let accessibility = shot["accessibility"]?.objectValue else { continue }
            var attributes = ["app": app, "image": imagePaths[id] ?? "",
                "accessibility-format": String(accessibility["format_version"]?.int64Value ?? 1),
                "truncated": accessibility["truncated"]?.boolValue == true ? "true" : "false"]
            attributes["bundle-identifier"] = shot["bundleIdentifier"]?.stringValue
            attributes["window-title"] = shot["windowTitle"]?.stringValue
            let header = attributes.keys.sorted().map { "\($0)=\"\(xmlEscape(attributes[$0]!))\"" }.joined(separator: " ")
            blocks.append("<appshot \(header)>\n\(xmlEscape(accessibility["content"]?.stringValue ?? ""))\n</appshot>")
        }
        return blocks.isEmpty ? text : text + marker + "\n" + blocks.joined(separator: "\n")
    }

    private static func xmlEscape(_ text: String) -> String {
        var result = ""
        for scalar in text.unicodeScalars {
            switch scalar.value {
            case 38: result += "&amp;"
            case 60: result += "&lt;"
            case 62: result += "&gt;"
            case 34: result += "&quot;"
            case 39: result += "&apos;"
            case 9: result += "&#9;"
            case 10: result += "&#10;"
            case 13: result += "&#13;"
            case 0...31, 65534, 65535: result += "\u{fffd}"
            default: result.unicodeScalars.append(scalar)
            }
        }
        return result
    }

    static func visibleText(_ text: String) -> String {
        guard let range = text.range(of: marker) else { return text }
        return String(text[..<range.lowerBound]).trimmingCharacters(in: .whitespacesAndNewlines)
    }

    /// Keep the original context byte-for-byte during a text-only queue edit.
    /// The separate attachment list remains owned by the queue row.
    static func suffix(_ text: String) -> String? {
        guard let range = text.range(of: marker) else { return nil }
        let suffix = String(text[range.lowerBound...])
        return suffix.components(separatedBy: "\n\nAttached images (local files").first
    }

    static func presentations(_ text: String) -> [String: AppshotPresentation] {
        guard let suffix = suffix(text), suffix.utf8.count <= 4 * 1024 * 1024,
              suffix.range(of: "<!DOCTYPE", options: .caseInsensitive) == nil,
              suffix.range(of: "<!ENTITY", options: .caseInsensitive) == nil else { return [:] }
        let xml = "<appshots>" + suffix.dropFirst(marker.count) + "</appshots>"
        let delegate = PresentationParser()
        let parser = XMLParser(data: Data(xml.utf8))
        parser.shouldResolveExternalEntities = false
        parser.delegate = delegate
        return parser.parse() ? delegate.presentations : [:]
    }

    private final class PresentationParser: NSObject, XMLParserDelegate {
        var presentations: [String: AppshotPresentation] = [:]
        private var seen: Set<String> = []
        private var depth = 0
        private var nodes = 0
        private var pending: (String, AppshotPresentation)?

        func parser(_ parser: XMLParser, didStartElement element: String,
                    namespaceURI: String?, qualifiedName: String?,
                    attributes: [String: String]) {
            depth += 1
            nodes += 1
            guard nodes <= 4096 else { parser.abortParsing(); return }
            if depth > 2 { pending = nil; return }
            guard depth == 2, element == "appshot", let path = attributes["image"],
                  let app = attributes["app"], !path.isEmpty,
                  !app.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return }
            guard seen.insert(path).inserted else {
                presentations.removeValue(forKey: path)
                return
            }
            pending = (path, AppshotPresentation(appName: String(app.prefix(200)),
                windowTitle: attributes["window-title"].map { String($0.prefix(512)) },
                bundleIdentifier: attributes["bundle-identifier"].map { String($0.prefix(256)) }))
        }

        func parser(_ parser: XMLParser, didEndElement element: String,
                    namespaceURI: String?, qualifiedName: String?) {
            if depth == 2, let (path, presentation) = pending {
                presentations[path] = presentation
                pending = nil
            }
            depth -= 1
        }
    }
}

/// Native iOS card. The app name remains useful when the desktop app's icon
/// is unavailable on the phone. Image loading uses the existing host relay.
struct AppshotCardView: View {
    let deviceId: String
    let attachment: UserImageAttachment
    let source: AppshotPresentation
    private let cache = AttachmentImageCache.shared
    @State private var preview: AttachmentPreview?

    var body: some View {
        Button {
            if case .loaded(let name, let image) = cache.snapshot(deviceId: deviceId, path: attachment.path) {
                preview = AttachmentPreview(name: name, image: image)
            } else { cache.load(deviceId: deviceId, path: attachment.path) }
        } label: {
            VStack(spacing: 6) {
                Group {
                    switch cache.snapshot(deviceId: deviceId, path: attachment.path) {
                    case .loaded(_, let image):
                        Image(uiImage: image).resizable().scaledToFit()
                            .mask(LinearGradient(stops: [.init(color: .black, location: 0), .init(color: .black, location: 0.72), .init(color: .clear, location: 1)], startPoint: .top, endPoint: .bottom))
                    case .loading: ProgressView().tint(Theme.textMuted)
                    case .error: Label("Appshot unavailable", systemImage: "photo.badge.exclamationmark")
                            .font(Theme.sans(12)).foregroundStyle(Theme.textMuted)
                    }
                }
                .frame(height: 120)
                Label("\(source.appName) · Appshot", systemImage: "macwindow")
                    .font(Theme.sans(11)).foregroundStyle(Theme.textMuted).lineLimit(1)
                Text(source.title).font(Theme.sans(12.5, weight: .medium))
                    .foregroundStyle(Theme.text).lineLimit(2).multilineTextAlignment(.center)
            }
            .frame(maxWidth: .infinity).padding(8)
            .contentShape(RoundedRectangle(cornerRadius: 14))
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Preview \(source.appName) Appshot: \(source.title)")
        .accessibilityIdentifier("appshot-card-\(attachment.id)")
        .task(id: "\(deviceId)|\(attachment.path)") { cache.load(deviceId: deviceId, path: attachment.path) }
        .fullScreenCover(item: $preview) { AttachmentLightbox(preview: $0) }
    }
}
