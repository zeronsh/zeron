import Foundation

/// Hops layout-thread frame notifications to the main queue, coalescing: if
/// several frames land before main runs, only the newest is applied.
final class FrameRelay: LayoutListener, @unchecked Sendable {
    private let lock = NSLock()
    private var scheduled = false
    private let apply: @MainActor () -> Void

    init(apply: @escaping @MainActor () -> Void) {
        self.apply = apply
    }

    func frameReady(revision: UInt64) {
        lock.lock()
        let schedule = !scheduled
        scheduled = true
        lock.unlock()
        guard schedule else { return }
        DispatchQueue.main.async { [self] in
            lock.lock()
            scheduled = false
            lock.unlock()
            MainActor.assumeIsolated { apply() }
        }
    }
}
