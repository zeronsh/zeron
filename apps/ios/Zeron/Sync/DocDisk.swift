// On-device Loro doc persistence — the old mobile app's snapshot cache
// (kv.ts/loro-room.ts) and the engine's DocsStore, in file form: one snapshot
// per doc under Application Support. Docs load BEFORE the room join, so the
// UI renders instantly from local state (offline included) and the join's
// version vector turns the backfill incremental instead of a full snapshot.

import Foundation
import Loro

@MainActor
enum SnapshotLease {
    private static var leases: [String: Int] = [:]
    private static var epoch = 0

    static func claim(_ chatId: String) -> Int {
        epoch &+= 1
        leases[chatId] = epoch
        return epoch
    }

    static func isCurrent(_ chatId: String, _ token: Int) -> Bool {
        leases[chatId] == token
    }

    static func revokeAll() {
        epoch &+= 1
        leases.removeAll()
    }
}

enum DocDisk {
    static var directoryOverride: URL?

    static var directory: URL {
        if let directoryOverride {
            try? FileManager.default.createDirectory(at: directoryOverride,
                                                     withIntermediateDirectories: true)
            return directoryOverride
        }
        let base = FileManager.default.urls(for: .applicationSupportDirectory,
                                            in: .userDomainMask)[0]
            .appendingPathComponent("ZeronDocs", isDirectory: true)
        try? FileManager.default.createDirectory(at: base, withIntermediateDirectories: true)
        return base
    }

    static func url(for id: String) -> URL {
        let safe = id.replacingOccurrences(of: "/", with: "_")
        return directory.appendingPathComponent("\(safe).loro")
    }

    /// Import the saved snapshot, if any. Returns whether anything loaded.
    @discardableResult
    static func load(into doc: LoroDoc, id: String) -> Bool {
        guard let data = try? Data(contentsOf: url(for: id)), !data.isEmpty else { return false }
        return (try? doc.importWith(bytes: data, origin: "disk")) != nil
    }

    /// Atomically persist the doc's snapshot.
    @discardableResult
    static func save(doc: LoroDoc, id: String) -> Bool {
        guard let data = try? doc.export(mode: .snapshot) else { return false }
        return saveRegistry(data: data, to: url(for: id))
    }

    /// The workspace registry's persisted blob ({rows, cursor, gcFloor,
    /// clock, pending} JSON — RegistryDoc.toData). Replaces the old `ws3_`
    /// Loro workspace snapshot; session docs stay Loro snapshots unchanged.
    static func registryURL(orgId: String, userId: String) -> URL {
        directory.appendingPathComponent("registry1_\(orgId)_\(userId).json")
    }

    // MARK: chat2 lineage snapshots (docs/chat2-sync.md C2)

    /// `c2_<id>.loro` = 8-byte magic + UInt64 LE room cursor + verified flag +
    /// snapshot,
    /// written atomically in ONE file so doc content and cursor can never
    /// diverge (a restored/copied doc that disagreed with its own cursor was
    /// the root of the s2 redownload-forever class). The un-prefixed
    /// `<id>.loro` files are the retired s2 lineage — never loaded into a
    /// chat2 doc (unrelated Loro histories would duplicate every message),
    /// kept on disk for rollback until LRU pruning ages them out.
    private static let chat2Magic = Data("C2SNAP02".utf8)
    private static let chat2OutboxMagic = Data("C2SNAP03".utf8)
    private static let legacyChat2Magic = Data("C2SNAP01".utf8)

    static func chat2URL(for id: String) -> URL {
        let safe = id.replacingOccurrences(of: "/", with: "_")
        return directory.appendingPathComponent("c2_\(safe).loro")
    }

    static func legacySnapshotExists(id: String) -> Bool {
        FileManager.default.fileExists(atPath: url(for: id).path)
    }

    /// Import the chat2 snapshot; returns its cursor and whether a completed
    /// catch-up verified it, or nil when absent/unreadable.
    static func loadChat2(into doc: LoroDoc, id: String)
        -> (cursor: UInt64, verified: Bool, firstContactQueued: Bool,
            outbox: [(batchId: String, bytes: Data)], bytes: Int)? {
        guard let data = try? Data(contentsOf: chat2URL(for: id)),
              data.count >= 16 else { return nil }
        let fileBytes = data.count
        let magic = data.prefix(8)
        let isLegacy = magic == legacyChat2Magic
        let hasOutbox = magic == chat2OutboxMagic
        guard isLegacy || magic == chat2Magic || hasOutbox else { return nil }
        var cursor: UInt64 = 0
        for (ix, byte) in data.subdata(in: 8..<16).enumerated() {
            cursor |= UInt64(byte) << (8 * ix)
        }
        var snapshotOffset: Int
        let verified: Bool
        let firstContactQueued: Bool
        var outbox: [(batchId: String, bytes: Data)] = []
        if isLegacy {
            snapshotOffset = 16
            verified = false
            firstContactQueued = false
        } else if hasOutbox {
            guard data.count >= 21 else { return nil }
            verified = data[16] & 1 != 0
            firstContactQueued = data[16] & 2 != 0
            let count = Int(readUInt32LE(data, at: 17))
            var offset = 21
            for _ in 0..<count {
                guard let idLength = readUInt32LEIfPresent(data, at: offset) else { return nil }
                offset += 4
                guard idLength > 0, idLength <= UInt32(data.count - offset),
                      let id = String(data: data.subdata(in: offset..<(offset + Int(idLength))),
                                      encoding: .utf8) else { return nil }
                offset += Int(idLength)
                guard let byteLength = readUInt32LEIfPresent(data, at: offset) else { return nil }
                offset += 4
                guard byteLength <= UInt32(data.count - offset) else { return nil }
                outbox.append((id, data.subdata(in: offset..<(offset + Int(byteLength)))))
                offset += Int(byteLength)
            }
            snapshotOffset = offset
        } else {
            guard data.count >= 17 else { return nil }
            snapshotOffset = 17
            verified = data[16] & 1 != 0
            firstContactQueued = false
        }
        guard data.count > snapshotOffset else {
            return (cursor, verified, firstContactQueued, outbox, fileBytes)
        }
        guard (try? doc.importWith(bytes: data.subdata(in: snapshotOffset..<data.count),
                                   origin: "disk")) != nil else { return nil }
        return (cursor, verified, firstContactQueued, outbox, fileBytes)
    }

    /// Atomically persist the chat2 doc snapshot + its room cursor.
    @discardableResult
    static func saveChat2(doc: LoroDoc, id: String, cursor: UInt64, verified: Bool,
                          firstContactQueued: Bool = false,
                          outbox: [(batchId: String, bytes: Data)] = []) -> Bool {
        guard let snapshot = try? doc.export(mode: .snapshot) else { return false }
        return saveChat2ReturningBytes(snapshot: snapshot, id: id, cursor: cursor,
                                       verified: verified,
                                       firstContactQueued: firstContactQueued,
                                       outbox: outbox) != nil
    }

    @discardableResult
    static func saveChat2(snapshot: Data, id: String, cursor: UInt64, verified: Bool,
                          firstContactQueued: Bool = false,
                          outbox: [(batchId: String, bytes: Data)] = []) -> Bool {
        saveChat2ReturningBytes(snapshot: snapshot, id: id, cursor: cursor,
                                verified: verified,
                                firstContactQueued: firstContactQueued,
                                outbox: outbox) != nil
    }

    static func saveChat2ReturningBytes(
        snapshot: Data,
        id: String,
        cursor: UInt64,
        verified: Bool,
        firstContactQueued: Bool = false,
        outbox: [(batchId: String, bytes: Data)] = []
    ) -> Int? {
        var data = chat2OutboxMagic
        var le = cursor.littleEndian
        withUnsafeBytes(of: &le) { data.append(contentsOf: $0) }
        data.append((verified ? 1 : 0) | (firstContactQueued ? 2 : 0))
        var count = UInt32(outbox.count).littleEndian
        withUnsafeBytes(of: &count) { data.append(contentsOf: $0) }
        for push in outbox {
            guard let id = push.batchId.data(using: .utf8),
                  let idLength = UInt32(exactly: id.count),
                  let byteLength = UInt32(exactly: push.bytes.count) else { return nil }
            var idLE = idLength.littleEndian
            withUnsafeBytes(of: &idLE) { data.append(contentsOf: $0) }
            data.append(id)
            var bytesLE = byteLength.littleEndian
            withUnsafeBytes(of: &bytesLE) { data.append(contentsOf: $0) }
            data.append(push.bytes)
        }
        data.append(snapshot)
        return saveRegistryReturningBytes(data: data, to: chat2URL(for: id))
    }

    static func chat2SnapshotSize(id: String) -> Int {
        guard let attributes = try? FileManager.default.attributesOfItem(
            atPath: chat2URL(for: id).path
        ), let size = attributes[.size] as? NSNumber else {
            return 0
        }
        return size.intValue
    }

    private static func readUInt32LE(_ data: Data, at offset: Int) -> UInt32 {
        data.subdata(in: offset..<(offset + 4)).withUnsafeBytes {
            UInt32(littleEndian: $0.loadUnaligned(as: UInt32.self))
        }
    }

    private static func readUInt32LEIfPresent(_ data: Data, at offset: Int) -> UInt32? {
        guard offset >= 0, offset <= data.count - 4 else { return nil }
        return readUInt32LE(data, at: offset)
    }

    static func chat2PendingOutboxCount(at url: URL) -> Int {
        guard let handle = try? FileHandle(forReadingFrom: url) else { return 0 }
        defer { try? handle.close() }
        guard let header = try? handle.read(upToCount: 21), header.count >= 8,
              header.prefix(8) == chat2OutboxMagic, header.count >= 21 else {
            return 0
        }
        let count = readUInt32LE(header, at: 17)
        return Int(count)
    }

    static func chat2HasPendingOutbox(id: String) -> Bool {
        chat2PendingOutboxCount(at: chat2URL(for: id)) > 0
    }

    @discardableResult
    static func saveRegistry(data: Data, to url: URL) -> Bool {
        saveRegistryReturningBytes(data: data, to: url) != nil
    }

    static func saveRegistryReturningBytes(data: Data, to url: URL) -> Int? {
        do {
            try data.write(to: url, options: .atomic)
            return data.count
        } catch {
            return nil
        }
    }

    private struct CachedModel: Codable {
        var id: String
        var label: String
        var description: String?
        var reasoningLevels: [String]
        var options: [CachedOption]
    }

    private struct CachedOption: Codable {
        var id: String
        var label: String
        var choices: [CachedChoice]
        var defaultChoice: String
    }

    private struct CachedChoice: Codable {
        var id: String
        var label: String
    }

    static func modelsURL(deviceId: String, harness: String) -> URL {
        func safe(_ value: String) -> String {
            value.map { $0.isLetter || $0.isNumber || $0 == "-" || $0 == "_" ? $0 : "_" }
                .reduce(into: "") { $0.append($1) }
        }
        return directory.appendingPathComponent("models_\(safe(deviceId))_\(safe(harness)).json")
    }

    @discardableResult
    static func saveModels(_ models: [ModelInfo], deviceId: String, harness: String) -> Bool {
        let cached = models.map { model in
            CachedModel(id: model.id, label: model.label, description: model.description,
                        reasoningLevels: model.reasoningLevels,
                        options: model.options.map {
                            CachedOption(id: $0.id, label: $0.label,
                                         choices: $0.choices.map { CachedChoice(id: $0.id, label: $0.label) },
                                         defaultChoice: $0.defaultChoice)
                        })
        }
        guard let data = try? JSONEncoder().encode(cached) else { return false }
        return saveRegistry(data: data, to: modelsURL(deviceId: deviceId, harness: harness))
    }

    static func loadModels(deviceId: String, harness: String) -> [ModelInfo]? {
        guard let data = try? Data(contentsOf: modelsURL(deviceId: deviceId, harness: harness)),
              let cached = try? JSONDecoder().decode([CachedModel].self, from: data) else { return nil }
        return cached.map { model in
            ModelInfo(id: model.id, label: model.label, description: model.description,
                      reasoningLevels: model.reasoningLevels,
                      options: model.options.map {
                          ModelOptionInfo(id: $0.id, label: $0.label,
                                          choices: $0.choices.map { ModelOptionChoiceInfo(id: $0.id, label: $0.label) },
                                          defaultChoice: $0.defaultChoice)
                      })
        }
    }

    /// LRU-prune session snapshots (the workspace registry blob is always
    /// kept; a leftover `ws3_` Loro snapshot is retained for rollback).
    static func prune(keep: Int) {
        let fm = FileManager.default
        guard let files = try? fm.contentsOfDirectory(at: directory,
                                                      includingPropertiesForKeys: [.contentModificationDateKey])
        else { return }
        let sessions = files.filter {
            $0.pathExtension == "loro"  // never the registry blob or uploads/
                && !$0.lastPathComponent.hasPrefix("ws3_")
                && !$0.lastPathComponent.hasPrefix("registry1_")
        }
        let deletable = sessions.filter {
            chat2PendingOutboxCount(at: $0) == 0
        }
        guard deletable.count > keep else { return }
        let sorted = deletable.sorted {
            let a = (try? $0.resourceValues(forKeys: [.contentModificationDateKey]).contentModificationDate) ?? .distantPast
            let b = (try? $1.resourceValues(forKeys: [.contentModificationDateKey]).contentModificationDate) ?? .distantPast
            return a > b
        }
        for stale in sorted.dropFirst(keep) {
            try? fm.removeItem(at: stale)
        }
    }

    /// Sign-out hygiene: local doc state belongs to the signed-in identity.
    @MainActor
    static func wipeAll() {
        SnapshotLease.revokeAll()
        try? FileManager.default.removeItem(at: directory)
    }
}

@MainActor
protocol DocSaverScheduling {
    func schedule(after nanoseconds: UInt64, action: @escaping @MainActor () async -> Void)
}

struct TaskDocSaverScheduler: DocSaverScheduling {
    nonisolated init() {}

    func schedule(after nanoseconds: UInt64, action: @escaping @MainActor () async -> Void) {
        Task { @MainActor in
            try? await Task.sleep(nanoseconds: nanoseconds)
            await action()
        }
    }
}

/// Debounced snapshot persistence shared by the doc stores: poke on every
/// change; `save` runs after a quiet debounce, and `flush` forces it
/// (backgrounding, store teardown). The closure captures whatever must be
/// written together (e.g. a chat2 doc AND its cursor — one atomic file).
@MainActor
final class DocSaver {
    private let save: () -> Bool
    private let quietDebounceNs: UInt64
    private let maxDeferralNs: UInt64
    private let staleRetryNs: UInt64
    private let scheduler: any DocSaverScheduling
    private var generation = 0
    private var deadlineGeneration = 0
    private var syncCommits = 0
    private var dirty = false
    var onSaved: (() -> Void)?
    var background: (() async -> Bool)?
    var isDirty: Bool { dirty }

    init(save: @escaping () -> Bool,
         quietDebounceNs: UInt64 = 5_000_000_000,
         maxDeferralNs: UInt64 = 300_000_000_000,
         staleRetryNs: UInt64 = 30_000_000_000,
         scheduler: any DocSaverScheduling = TaskDocSaverScheduler()) {
        self.save = save
        self.quietDebounceNs = quietDebounceNs
        self.maxDeferralNs = maxDeferralNs
        self.staleRetryNs = staleRetryNs
        self.scheduler = scheduler
    }

    func retireTimers() {
        generation += 1
        deadlineGeneration += 1
    }

    private func flushFromTimer() async {
        guard dirty else { return }
        if let background {
            if await background(), dirty {
                dirty = false
                generation += 1
                deadlineGeneration += 1
                onSaved?()
            } else if dirty {
                armDeadline(after: min(maxDeferralNs, staleRetryNs))
            }
        } else {
            flush()
        }
    }

    private func armDeadline(after delay: UInt64) {
        deadlineGeneration += 1
        let expectedDeadline = deadlineGeneration
        scheduler.schedule(after: delay) { [weak self] in
            guard let self, self.deadlineGeneration == expectedDeadline,
                  self.dirty else { return }
            await self.flushFromTimer()
        }
    }

    func poke() {
        let wasDirty = dirty
        dirty = true
        generation += 1
        let expected = generation
        if !wasDirty {
            armDeadline(after: maxDeferralNs)
        }
        scheduler.schedule(after: quietDebounceNs) { [weak self] in
            guard let self, self.generation == expected else { return }
            await self.flushFromTimer()
        }
    }

    func flush() {
        guard dirty else { return }
        _ = commitNow()
    }

    @discardableResult
    func commitNow() -> Bool {
        syncCommits &+= 1
        guard save() else {
            dirty = true
            scheduleRetry()
            return false
        }
        dirty = false
        generation += 1
        deadlineGeneration += 1
        onSaved?()
        return true
    }

    /// Exports off-main and writes only if no newer generation superseded it.
    func commitAsync(export: @escaping @Sendable () -> Data?,
                     write: @escaping (Data) -> Bool) async -> Bool {
        guard dirty else { return true }
        let syncCommitsAtStart = syncCommits
        generation += 1
        let expected = generation
        let snapshot = await SnapshotExporter.shared.export(export)
        if generation != expected {
            if syncCommits == syncCommitsAtStart, let snapshot {
                _ = write(snapshot)
            }
            return false
        }
        guard let snapshot, write(snapshot) else {
            dirty = true
            scheduleRetry()
            return false
        }
        dirty = false
        generation += 1
        deadlineGeneration += 1
        onSaved?()
        return true
    }

    private func scheduleRetry() {
        generation += 1
        let expected = generation
        scheduler.schedule(after: 2_000_000_000) { [weak self] in
            guard let self, self.generation == expected else { return }
            await self.flushFromTimer()
        }
    }
}

actor SnapshotExporter {
    static let shared = SnapshotExporter()
    private var tail: Task<Void, Never>?

    func export(_ work: @escaping @Sendable () -> Data?) async -> Data? {
        let predecessor = tail
        let current = Task.detached(priority: .utility) {
            if let predecessor {
                await predecessor.value
            }
            return work()
        }
        tail = Task.detached(priority: .utility) {
            _ = await current.value
        }
        return await current.value
    }
}

/// DocSaver's registry twin: debounced persistence for the registry blob.
/// Poke on every mutation; the blob writes ~1.5s after the last poke, and
/// `flush` forces it (backgrounding, store teardown).
@MainActor
final class RegistrySaver {
    private let url: URL
    private let save: () -> Bool
    private var generation = 0
    private var dirty = false

    init(url: URL, data: @escaping () -> Data?) {
        self.url = url
        self.save = {
            guard let data = data() else { return false }
            return DocDisk.saveRegistry(data: data, to: url)
        }
    }

    func poke() {
        dirty = true
        generation += 1
        let expected = generation
        Task { @MainActor [weak self] in
            try? await Task.sleep(nanoseconds: 1_500_000_000)
            guard let self, self.generation == expected else { return }
            self.flush()
        }
    }

    func flush() {
        guard dirty else { return }
        guard save() else {
            dirty = true
            scheduleRetry()
            return
        }
        dirty = false
    }

    private func scheduleRetry() {
        generation += 1
        let expected = generation
        Task { @MainActor [weak self] in
            try? await Task.sleep(nanoseconds: 2_000_000_000)
            guard let self, self.generation == expected else { return }
            self.flush()
        }
    }
}
