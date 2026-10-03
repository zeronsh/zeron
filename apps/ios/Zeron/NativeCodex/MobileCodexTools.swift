import Foundation

@MainActor
enum MobileCodexTools {
    static func dispatch(_ params: [String: Any], shell: MobileShellRuntime) async -> [String: Any] {
        do {
            guard let name = params["tool"] as? String,
                  let args = params["arguments"] as? [String: Any] else {
                throw EmbeddedCodex.Failure.message("Invalid mobile tool arguments")
            }
            func string(_ key: String) throws -> String {
                guard let value = args[key] as? String else { throw EmbeddedCodex.Failure.message("Missing \(key)") }
                return value
            }
            let text: String
            let success: Bool
            switch name {
            case "mobile_shell":
                let result = try await shell.execute(try string("command"))
                text = String(decoding: try JSONSerialization.data(withJSONObject: ["stdout": result.stdout, "stderr": result.stderr, "exitCode": result.exitCode, "changedPaths": result.changedPaths]), as: UTF8.self)
                success = result.exitCode == 0
            case "mobile_read_file":
                text = try await shell.readFile(try string("path")); success = true
            case "mobile_write_file":
                try await shell.writeFile(try string("path"), content: try string("content"))
                text = "File saved."; success = true
            default: throw EmbeddedCodex.Failure.message("Unsupported mobile tool: \(name)")
            }
            return ["success": success, "contentItems": [["type": "inputText", "text": text]]]
        } catch {
            return ["success": false, "contentItems": [["type": "inputText", "text": error.localizedDescription]]]
        }
    }
}
