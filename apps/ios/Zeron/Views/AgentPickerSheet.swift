import SwiftUI

struct AgentPickerSheet: View {
    @Environment(\.dismiss) private var dismiss
    @Binding var selected: String?
    let agents: [AgentInfo]
    let error: String?
    let onRefresh: () async -> Void
    @State private var hasLoaded = false

    var body: some View {
        NavigationStack {
            List {
                Button {
                    selected = nil
                    dismiss()
                } label: {
                    Label("Server default / current agent", systemImage: selected == nil ? "checkmark" : "circle")
                }
                ForEach(agents) { agent in
                    Button {
                        selected = agent.id
                        dismiss()
                    } label: {
                        HStack {
                            VStack(alignment: .leading, spacing: 3) {
                                Text(agent.label)
                                if let description = agent.description {
                                    Text(description).font(Theme.sans(12)).foregroundStyle(Theme.textMuted)
                                }
                            }
                            Spacer()
                            if selected == agent.id { Image(systemName: "checkmark") }
                        }
                    }
                    .accessibilityIdentifier("opencode-agent-\(agent.id)")
                }
                if let error { Text(error).foregroundStyle(Theme.danger) }
                if agents.isEmpty, !hasLoaded, error == nil {
                    ProgressView("Loading agents…")
                } else if agents.isEmpty, error == nil {
                    Text("No selectable agents found.").foregroundStyle(Theme.textMuted)
                }
                Button("Refresh agents") { Task { await refresh() } }
            }
            .navigationTitle("Select agent")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .cancellationAction) { Button("Done") { dismiss() } } }
        }
        .task {
            await refresh()
            for delay in [2.0, 3.0] {
                try? await Task.sleep(for: .seconds(delay))
                guard !Task.isCancelled else { return }
                await refresh()
            }
        }
        .presentationDetents([.medium, .large])
        .presentationDragIndicator(.visible)
    }

    @MainActor private func refresh() async {
        hasLoaded = false
        await onRefresh()
        if !Task.isCancelled { hasLoaded = true }
    }
}
