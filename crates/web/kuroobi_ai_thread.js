// One pool worker for kuroobi_ai.wasm's threads: the same module on the same memory, waiting to run a thread.
import { imports, serve } from './wasi_threads.js';

self.onmessage = async (e) => {
  const { module, memory, ctl, slot } = e.data;
  const instance = await WebAssembly.instantiate(module, imports(memory, ctl));
  self.postMessage({ ready: slot });
  serve(instance, ctl, slot);
};
