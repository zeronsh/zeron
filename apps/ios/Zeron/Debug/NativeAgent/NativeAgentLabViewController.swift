#if DEBUG
import UIKit

/// Launch with -native-agent-lab. This exercises local tools only; it does not
/// pretend to run Codex until the upstream Rust core can actually be embedded.
final class NativeAgentLabViewController: UIViewController {
    private let output = UITextView()
    private let command = UITextField()
    private let run = UIButton(type: .system)
    private lazy var runtime = MobileShellRuntime(checkpointURL: FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0].appendingPathComponent("native-agent-spike/workspace.json"))

    override func viewDidLoad() {
        super.viewDidLoad()
        title = "Mobile tools lab"
        view.backgroundColor = .systemBackground
        output.isEditable = false
        output.font = .monospacedSystemFont(ofSize: 13, weight: .regular)
        output.accessibilityIdentifier = "native-agent-output"
        command.borderStyle = .roundedRect
        command.autocorrectionType = .no
        command.autocapitalizationType = .none
        command.text = "cat hello.txt"
        command.accessibilityIdentifier = "native-agent-command"
        run.setTitle("Run", for: .normal)
        run.addTarget(self, action: #selector(execute), for: .touchUpInside)
        let stop = UIButton(type: .system)
        stop.setTitle("Stop", for: .normal)
        stop.addTarget(self, action: #selector(cancel), for: .touchUpInside)
        let controls = UIStackView(arrangedSubviews: [command, run, stop])
        controls.spacing = 12
        let stack = UIStackView(arrangedSubviews: [output, controls])
        stack.axis = .vertical
        stack.spacing = 12
        stack.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.leadingAnchor, constant: 16),
            stack.trailingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.trailingAnchor, constant: -16),
            stack.topAnchor.constraint(equalTo: view.safeAreaLayoutGuide.topAnchor),
            stack.bottomAnchor.constraint(equalTo: view.keyboardLayoutGuide.topAnchor, constant: -12),
        ])
        output.text = "Local just-bash worker · no model connected\n\n"
        run.isEnabled = false
        Task {
            defer { run.isEnabled = true }
            do {
                try await runtime.writeFile("/workspace/hello.txt", content: "hello\nworld\n")
                let result = try await runtime.execute("cat hello.txt | grep hello; sed -i 's/world/mobile/' hello.txt; cat hello.txt")
                output.text += "$ read → pipe → edit → read\n\(result.stdout)\(result.stderr)\nexit \(result.exitCode)\n"
                let text = try await runtime.readFile("/workspace/hello.txt")
                guard text == "hello\nmobile\n" else { throw MobileShellRuntime.Failure.message("Shared workspace mismatch") }
                await runtime.cancel()
                let restored = try await runtime.readFile("/workspace/hello.txt")
                guard restored == text else { throw MobileShellRuntime.Failure.message("Checkpoint mismatch") }
                output.text += "\nPASS: shell/native file tools share edits; checkpoint survives worker restart.\n"
            } catch { output.text += "\nFAILED: \(error.localizedDescription)\n" }
        }
    }

    @objc private func execute() {
        let text = command.text ?? ""
        run.isEnabled = false
        output.text += "\n$ \(text)\n"
        Task {
            defer { run.isEnabled = true }
            do {
                let result = try await runtime.execute(text)
                output.text += "\(result.stdout)\(result.stderr)\nexit \(result.exitCode)\n"
            } catch { output.text += "\(error.localizedDescription)\n" }
        }
    }

    @objc private func cancel() { Task { await runtime.cancel() } }
}
#endif
