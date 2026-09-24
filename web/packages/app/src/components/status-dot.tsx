import type { ChatIndicator } from "../lib/view";

const LABELS: Record<ChatIndicator, string> = {
  working: "Working",
  awaitingInput: "Input",
  errored: "Failed",
  completed: "Done",
  idle: "Idle",
};

/** The status dot: same colors and meaning as the desktop's sidebar dots. */
export function StatusDot({ status }: { status: ChatIndicator }) {
  return <span className={`dot dot-${status}`} role="img" aria-label={LABELS[status]} />;
}
