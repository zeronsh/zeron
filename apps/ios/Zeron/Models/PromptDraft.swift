import Foundation

struct PromptDraftTarget: Codable, Hashable {
    var deviceId: String
    var spaceId: String?
    var projectName: String?
    var config: ChatConfig?
    var branch: String?
    var newWorktree = false
}
struct PromptDraftAttachment: Codable, Hashable {
    var id: String
    var name: String
    var blob: String
    var appshot: JSONValue?
}
struct PromptDraftContent: Codable, Hashable {
    var prompt: String
    var target: PromptDraftTarget
    var attachments: [PromptDraftAttachment] = []
    var hasContent: Bool { !prompt.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || !attachments.isEmpty }
    var preview: String {
        prompt.split(separator: "\n").first.map { String($0.prefix(180)) } ?? "\(attachments.count) attachments"
    }
}
struct PromptDraftSave: Codable {
    var deferred: Bool? = nil
    var publicationKey: String { deferred == true ? "editing-\(revision)" : revision }
    var id: String
    var revision: String
    var baseRevision: String?
    var createdAt: Int64
    var content: PromptDraftContent
}
struct PromptDraftRow: Identifiable, Hashable {
    var id: String
    var revision: String
    var baseRevision: String?
    var createdAt: Int64
    var preview: String
    var target: PromptDraftTarget
    var orderKey: String
    var conflict: Bool
}

extension RegistryDoc {
    func promptDraftClosed(_ id: String) -> Bool { overlayRow(kind: "promptDrafts", id: id)?.fields["closed"]?.boolValue == true }
    func promptDraftConsumed(_ id: String) -> Bool { overlayRow(kind: "promptDrafts", id: id)?.fields["sentRevision"]?.stringValue != nil }
    func publishPromptDraft(_ save: PromptDraftSave) {
        guard !promptDraftClosed(save.id) else { return }
        observePromptDraftClock(kind: "promptDrafts", id: save.id)
        if let version = overlayRow(kind: "draftRevisions", id: save.revision) {
            if save.deferred != true && version.fields["draftId"]?.stringValue == save.id && overlayRow(kind: "promptDrafts", id: save.id)?.fields["listed"]?.boolValue != true {
                write(kind: "promptDrafts", id: save.id, op: .upsert, set: ["listed": .bool(true)])
            }
            return
        }
        if let base = save.baseRevision { observePromptDraftClock(kind: "draftRevisions", id: base) }
        let stamp = nextHlc()
        var index: [String: JSONValue] = ["revision": .string(save.revision)]
        index[save.deferred == true ? "deferred" : "listed"] = .bool(true)
        if overlayRows(kind: "draftRevisions").contains(where: { $0.fields["baseRevision"]?.stringValue == save.revision }) {
            index.removeValue(forKey: "revision")
        }
        if overlayRow(kind: "promptDrafts", id: save.id)?.fields["orderKey"]?.stringValue.map(PinOrder.valid) != true {
            guard let key = PinOrder.between(nil, promptDraftRows.first?.orderKey, nonce: stamp) else { return }
            index["orderKey"] = .string(key)
        }
        let fields: [String: JSONValue] = ["draftId": .string(save.id), "baseRevision": save.baseRevision.map(JSONValue.string) ?? .null,
            "createdAt": .int(save.createdAt), "preview": .string(save.content.preview), "target": JSONValue(encodable: save.content.target) ?? .null]
        enqueue(ops: [RegistryOp(kind: "draftRevisions", id: save.revision, op: .upsert, set: fields, hlc: stamp, clocks: nil),
                      RegistryOp(kind: "promptDrafts", id: save.id, op: .upsert, set: index, hlc: stamp, clocks: nil)])
    }
    var promptDraftRows: [PromptDraftRow] {
        let versions = overlayRows(kind: "draftRevisions")
        let parents = Set(versions.compactMap { $0.fields["baseRevision"]?.stringValue })
        var heads: [String: String] = [:]
        for version in versions where !parents.contains(version.id) {
            if let root = version.fields["draftId"]?.stringValue { heads[root] = max(heads[root] ?? version.id, version.id) }
        }
        var rows = versions.compactMap { version -> PromptDraftRow? in
            guard !parents.contains(version.id), let root = version.fields["draftId"]?.stringValue, !promptDraftClosed(root),
                  let data = try? JSONEncoder().encode(version.fields["target"]),
                  let target = try? JSONDecoder().decode(PromptDraftTarget.self, from: data) else { return nil }
            let index = overlayRow(kind: "promptDrafts", id: root)
            if index?.fields["deferred"]?.boolValue == true && index?.fields["listed"]?.boolValue != true { return nil }
            let sent = index?.fields["sentRevision"]?.stringValue
            if sent == version.id { return nil }
            let selected = index?.fields["revision"]?.stringValue
            let current = selected.flatMap { parents.contains($0) ? nil : $0 } ?? heads[root]
            let conflict = sent != nil || current != version.id
            let id = conflict ? version.id : root
            guard !promptDraftClosed(id), !promptDraftConsumed(id), let key = (overlayRow(kind: "promptDrafts", id: id) ?? index)?.fields["orderKey"]?.stringValue, PinOrder.valid(key) else { return nil }
            return PromptDraftRow(id: id, revision: version.id, baseRevision: version.fields["baseRevision"]?.stringValue, createdAt: version.fields["createdAt"]?.int64Value ?? 0,
                preview: version.fields["preview"]?.stringValue ?? "", target: target, orderKey: key, conflict: conflict)
        }
        let keys = rows.map(\.orderKey)
        for i in rows.indices where rows[i].conflict && !rowExists(kind: "promptDrafts", id: rows[i].id) {
            let upper = keys.filter { $0 > rows[i].orderKey }.min()
            if let key = PinOrder.between(rows[i].orderKey, upper, nonce: rows[i].revision) { rows[i].orderKey = key }
        }
        return rows.sorted { $0.orderKey == $1.orderKey ? $0.id < $1.id : $0.orderKey < $1.orderKey }
    }
    func setPromptDraftOrder(_ id: String, key: String) {
        guard PinOrder.valid(key) else { return }
        observePromptDraftClock(kind: "promptDrafts", id: id)
        write(kind: "promptDrafts", id: id, op: .upsert, set: ["orderKey": .string(key)])
    }
    func movePromptDraft(_ id: String, before: String?, after: String?) {
        guard !promptDraftClosed(id) else { return }
        observePromptDraftClock(kind: "promptDrafts", id: id)
        let rows = promptDraftRows.filter { $0.id != id }
        let index = before.flatMap { b in rows.firstIndex { $0.id == b } }
            ?? after.flatMap { a in rows.firstIndex { $0.id == a }.map { $0 + 1 } } ?? rows.count
        guard let key = PinOrder.between(index > 0 ? rows[index - 1].orderKey : nil, index < rows.count ? rows[index].orderKey : nil, nonce: nextHlc()) else { return }
        write(kind: "promptDrafts", id: id, op: .upsert, set: ["orderKey": .string(key)])
    }
}
