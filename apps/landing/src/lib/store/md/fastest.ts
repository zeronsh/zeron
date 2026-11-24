// A time held to a limit, whatever else the machine is doing: the least it
// took in up to five tries, done at the first within the limit. What is
// slow by its nature (the square of what it's given) is slow in every one.
export const TRIES = 5;

export function fastest<T>(work: () => T, within: number): { ms: number; out: T } {
  let ms = Infinity;
  let out!: T;
  for (let i = 0; i < TRIES && ms >= within; i++) {
    const started = performance.now();
    out = work();
    ms = Math.min(ms, performance.now() - started);
  }
  return { ms, out };
}
