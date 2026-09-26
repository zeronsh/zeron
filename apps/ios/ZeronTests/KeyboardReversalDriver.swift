#if DEBUG
import Foundation

/// Drives the keyboard test from observed geometry, independently of the display clock.
struct KeyboardReversalDriver {
    enum Phase: String {
        case initial, reversing, missedEndpoint, resetting, finishing, complete, failed
    }

    enum Action: Equatable {
        case setShowing(Bool)
        case finished
    }

    let hiddenPosition: CGFloat
    let maximumRecoveries: Int
    private(set) var phase: Phase = .initial
    private(set) var reversals = 0
    private(set) var recoveries = 0
    private(set) var showing = false
    private(set) var failure: String?
    private var legStart: CGFloat = 0
    private var stableFrames = 0

    init(hiddenPosition: CGFloat, maximumRecoveries: Int = 3) {
        self.hiddenPosition = hiddenPosition
        self.maximumRecoveries = maximumRecoveries
    }

    var diagnostic: String {
        "phase=\(phase.rawValue) reversals=\(reversals)/5 recoveries=\(recoveries)/\(maximumRecoveries) showing=\(showing) failure=\(failure ?? "none")"
    }

    mutating func observe(position: CGFloat, target: CGFloat, isFirstResponder: Bool) -> Action? {
        switch phase {
        case .initial:
            phase = .reversing
            legStart = position
            showing = true
            return .setShowing(true)
        case .reversing:
            // Wait for the model to acknowledge the requested direction.
            guard targetIsAhead(target) else { return nil }
            let progress = (position - legStart) / (target - legStart)
            if progress >= 0.45, progress < 1, abs(position - target) > 1 {
                reversals += 1
                legStart = position
                showing.toggle()
                if reversals == 5 { phase = .finishing }
                return .setShowing(showing)
            }
            if progress >= 1 || abs(position - target) <= 1 {
                guard recoveries < maximumRecoveries else {
                    failure = "Missed in-flight observation window; recovery budget exhausted"
                    phase = .failed
                    return .finished
                }
                recoveries += 1
                stableFrames = 0
                phase = .missedEndpoint
            }
        case .missedEndpoint, .resetting:
            // First settle the missed leg, then return to its opposite endpoint.
            // Neither setup transition counts as an observed reversal.
            let settled = targetIsAhead(target) && abs(position - target) <= 1
                && isFirstResponder == showing
            stableFrames = settled ? stableFrames + 1 : 0
            if stableFrames == 3 {
                phase = phase == .missedEndpoint ? .resetting : .reversing
                stableFrames = 0
                legStart = position
                showing.toggle()
                return .setShowing(showing)
            }
        case .finishing:
            let hidden = abs(position - hiddenPosition) <= 1 && abs(target - hiddenPosition) <= 1
            stableFrames = hidden && !isFirstResponder ? stableFrames + 1 : 0
            if stableFrames == 3 {
                phase = .complete
                return .finished
            }
        case .complete, .failed:
            return .finished
        }
        return nil
    }

    private func targetIsAhead(_ target: CGFloat) -> Bool {
        showing ? target < legStart - 1 : target > legStart + 1
    }
}
#endif
