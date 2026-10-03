import Foundation

struct NativeCodexTool: Codable {
    var name: String
    var argument: String
    var output: String?
    var resolved = false
    var isError = false

    var displayOutput: String? {
        guard name == "mobile_shell", let output, let data = output.data(using: .utf8),
              let result = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return output }
        var pieces = [result["stdout"] as? String, result["stderr"] as? String].compactMap { $0 }.filter { !$0.isEmpty }
        if let code = result["exitCode"] as? Int { pieces.append("Exit code: \(code)") }
        if let paths = result["changedPaths"] as? [String], !paths.isEmpty { pieces.append("Changed files:\n" + paths.joined(separator: "\n")) }
        return pieces.joined(separator: "\n")
    }

    /// Recognize only the exact tool envelope written by the old native adapter.
    /// Keep old storage unchanged; ordinary assistant markdown stays markdown.
    static func legacy(_ text: String) -> Self? {
        for name in ["mobile_shell", "mobile_read_file", "mobile_write_file"] {
            let prefix = "**\(name)**\n\n```\n"
            guard text.hasPrefix(prefix) else { continue }
            let body = String(text.dropFirst(prefix.count))
            guard let end = body.range(of: "\n```\n") else { return nil }
            let argument = String(body[..<end.lowerBound])
            let rest = String(body[end.upperBound...])
            if rest.isEmpty { return Self(name: name, argument: argument) }
            guard rest.hasPrefix("\n```\n"), rest.hasSuffix("\n```\n") else { return nil }
            let output = String(rest.dropFirst(5).dropLast(5))
            // Legacy entries did not persist the success flag; do not invent it.
            return Self(name: name, argument: argument, output: output, resolved: true)
        }
        return nil
    }
}

extension NativeCodexMessage {
    var copiedText: String? {
        if let activity = tool ?? NativeCodexTool.legacy(text) {
            let state = activity.resolved ? (activity.isError ? "Failed" : "Completed") : "In progress"
            return "Codex tool: \(activity.name)\n\(activity.argument)\n\(state)" + (activity.displayOutput.map { "\n" + $0 } ?? "")
        }
        guard !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return nil }
        return (user ? "You: " : "Codex: ") + text
    }

    func transcriptEntry(streaming: Bool, working: Bool) -> LocalTranscriptEntry {
        let activity = user ? nil : tool ?? NativeCodexTool.legacy(text)
        let projected = activity.map { value in
            LocalTranscriptTool(name: value.name, argument: value.argument,
                output: !working && !value.resolved ? "Interrupted before a result was received." : value.displayOutput,
                resolved: value.resolved || !working, isError: value.isError || (!working && !value.resolved))
        }
        return LocalTranscriptEntry(id: id, user: user, text: activity == nil ? text : "", streaming: streaming, tool: projected)
    }
}
