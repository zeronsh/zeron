/**
 * `preview_load_gate` (queue.rs:262-265): a process-wide mutex so only ONE
 * queue-row thumbnail fetch is ever in flight at a time across the whole
 * app — it throttles the burst of concurrent `ReadAttachmentChunk` calls
 * that would otherwise fire when many queue rows with attachments become
 * visible at once (chat switch, fast scroll, or a big queue arriving from
 * another device). A `Promise` chain is the JS-shaped mutex; the ticket's
 * sketched shape exactly.
 */

let gate: Promise<void> = Promise.resolve();

/** Run `fn` under the gate — at most one queue-thumbnail load in flight. */
export function withQueuePreviewGate<T>(fn: () => Promise<T>): Promise<T> {
  const next = gate.then(fn, fn);
  gate = next.then(
    () => undefined,
    () => undefined,
  );
  return next;
}
