/**
 * Closing a Cloud runner out of rooms it reached through its session's
 * `devices` reach, once the person isolates the session
 * (registry-runner.ts). Shared by ChatRoom and DeviceRoom.
 *
 * The registry admits a runner into another chat or device only while its
 * reach allows (`/runner-admit`), and says at which revocation epoch. The
 * Worker stamps that epoch on the forward (`RUNNER_EPOCH_HEADER`); a room
 * keeps it on the socket. Isolating bumps the epoch and sends every room the
 * runner was admitted to a drop (`RUNNER_DROP_PATH`): the room closes the
 * runner's sockets from older epochs and remembers the epoch, so a dial
 * admitted just before the change, arriving after the drop, is refused too.
 *
 * Admissions a runner has in every reach (its own chats and device, its
 * delegates) carry no epoch and are never dropped.
 */
import { RUNNER_DEVICE_HEADER, RUNNER_EPOCH_HEADER } from "./env";

/** The internal route a room takes a drop on (never routed by the Worker). */
export const RUNNER_DROP_PATH = "/drop-runner";

/** Close code for a runner socket its reach no longer allows. */
export const REACH_REVOKED = 4403;

/** A drop's body: close `device`'s sockets admitted before `epoch`. */
export interface RunnerDrop {
  readonly device: string;
  readonly epoch: number;
}

const ID_RE = /^[A-Za-z0-9_-]{1,128}$/;

/** The meta key a room keeps a runner's latest dropped epoch under. */
export const runnerDroppedKey = (device: string): string => `runnerDropped:${device}`;

/** The runner and epoch a forward was admitted with, when it is revocable. */
export const admittedRunner = (request: Request): RunnerDrop | undefined => {
  const device = request.headers.get(RUNNER_DEVICE_HEADER);
  const raw = request.headers.get(RUNNER_EPOCH_HEADER);
  if (!device || raw === null || !/^\d{1,15}$/.test(raw)) return undefined;
  return { device, epoch: Number(raw) };
};

/** A drop's body, validated. */
export const parseDrop = (body: unknown): RunnerDrop | undefined => {
  const { device, epoch } = (body ?? {}) as Record<string, unknown>;
  if (typeof device !== "string" || !ID_RE.test(device)) return undefined;
  if (typeof epoch !== "number" || !Number.isSafeInteger(epoch) || epoch < 0) return undefined;
  return { device, epoch };
};

/** Whether an admission was revoked by a drop the room recorded. */
export const revoked = (admitted: RunnerDrop, dropped: string | undefined): boolean =>
  dropped !== undefined && admitted.epoch < Number(dropped);

/** Record `drop` in a room's meta and close the sockets it revokes. Each
 * socket's admission is read by `admissionOf`. */
export const applyDrop = (
  drop: RunnerDrop,
  meta: {
    get(key: string): string | undefined;
    set(key: string, value: string): void;
  },
  sockets: Iterable<WebSocket>,
  admissionOf: (socket: WebSocket) => RunnerDrop | undefined
): number => {
  const key = runnerDroppedKey(drop.device);
  const before = meta.get(key);
  if (before === undefined || Number(before) < drop.epoch) meta.set(key, String(drop.epoch));
  let closed = 0;
  for (const socket of sockets) {
    const admitted = admissionOf(socket);
    if (admitted?.device !== drop.device || admitted.epoch >= drop.epoch) continue;
    try {
      socket.close(REACH_REVOKED, "this Cloud chat no longer reaches here");
    } catch {
      /* already gone */
    }
    closed += 1;
  }
  return closed;
};
