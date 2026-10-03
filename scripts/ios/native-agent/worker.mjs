import { createRuntime } from "./runtime.mjs";

let runtime;
let queue = Promise.resolve();
let sequence = 0;
const callbacks = new Map();
function nativeRequest(request) {
  return new Promise((resolve, reject) => {
    const fsId = ++sequence;
    callbacks.set(fsId, { resolve, reject });
    self.postMessage({ fsId, request });
  });
}
self.onmessage = ({ data }) => {
  // Filesystem replies must bypass the execution queue: the shell is awaiting them.
  if (data.fsId) {
    const callback = callbacks.get(data.fsId);
    if (!callback) return;
    callbacks.delete(data.fsId);
    if (data.error) callback.reject(new Error(data.error));
    else callback.resolve(data.result);
    return;
  }
  queue = queue.then(async () => {
    try {
      if (data.method === "initialize") {
        runtime = await createRuntime([], nativeRequest);
        self.postMessage({ id: data.id, result: true });
      } else {
        if (!runtime) throw new Error("Mobile runtime has not been initialized");
        self.postMessage({ id: data.id, result: await runtime.dispatch(data) });
      }
    } catch (error) {
      self.postMessage({ id: data.id, error: String(error) });
    }
  });
};
