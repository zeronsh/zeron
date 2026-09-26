@testable import Zeron

/// Registers deadlines synchronously, so advancing time never races a task
/// that has not started sleeping yet. Equal deadlines retain insertion order.
@MainActor
final class ManualDocSaverScheduler: DocSaverScheduling {
    private struct Job {
        let deadline: UInt64
        let action: @MainActor () async -> Void
    }

    private var now: UInt64 = 0
    private var jobs: [Job] = []
    private var advancing = false

    func schedule(after nanoseconds: UInt64, action: @escaping @MainActor () async -> Void) {
        jobs.append(Job(deadline: now + nanoseconds, action: action))
    }

    /// Moves to the next deadline and starts its callback without waiting for it
    /// to finish. Wait for an explicit signal from the callback before advancing
    /// again, and await the returned task to observe its completed effects.
    func startNext() -> Task<Void, Never>? {
        precondition(!advancing, "Cannot start a callback during a serial advance")
        guard let index = jobs.indices.min(by: { jobs[$0].deadline < jobs[$1].deadline }) else { return nil }
        let job = jobs.remove(at: index)
        now = job.deadline
        return Task { await job.action() }
    }

    /// Completes every due action, including work scheduled by those actions,
    /// before returning. Await this before asserting persistence effects.
    func advance(by nanoseconds: UInt64) async {
        precondition(!advancing, "Concurrent clock advances are not supported")
        advancing = true
        defer { advancing = false }
        let target = now + nanoseconds
        while let index = jobs.indices.min(by: { jobs[$0].deadline < jobs[$1].deadline }),
              jobs[index].deadline <= target {
            let job = jobs.remove(at: index)
            now = job.deadline
            await job.action()
        }
        now = target
    }
}
