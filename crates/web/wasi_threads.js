// What kuroobi_ai.wasm built for wasm32-wasip1-threads asks of its host: the few WASI calls std makes, and
// `wasi.thread-spawn`, answered from a pool of workers started before the engine runs. A worker cannot start
// while the thread that created it is busy searching, so every thread the engine will want is waiting already.

/** Slots of the pool's control block: [next thread id, then per worker: state, thread id, start argument, spare]. */
const SLOT = 4;
const IDLE = 0;
const CLAIMED = 1;
const READY = 2;

/** The memory the module imports (pages), read from its import section. */
export function memoryLimits(bytes) {
  const d = new Uint8Array(bytes instanceof ArrayBuffer ? bytes : bytes.buffer, bytes.byteOffset ?? 0);
  let i = 8;
  const leb = () => {
    let r = 0;
    let s = 0;
    for (;;) {
      const b = d[i++];
      r += (b & 0x7f) * 2 ** s;
      s += 7;
      if (b < 0x80) return r;
    }
  };
  const name = () => {
    const n = leb();
    const t = new TextDecoder().decode(d.subarray(i, i + n));
    i += n;
    return t;
  };
  while (i < d.length) {
    const id = d[i++];
    const size = leb();
    const end = i + size;
    if (id === 2) {
      for (let n = leb(); n > 0; n--) {
        const mod = name();
        const field = name();
        const kind = d[i++];
        if (kind === 0) leb();
        else if (kind === 2) {
          const flags = d[i++];
          const initial = leb();
          const maximum = flags & 1 ? leb() : undefined;
          if (mod === 'env' && field === 'memory') return { initial, maximum, shared: true };
        } else throw new Error('kuroobi_ai: unexpected import kind ' + kind);
      }
    }
    i = end;
  }
  throw new Error('kuroobi_ai: the module imports no memory');
}

/** A control block for `workers` pool workers. */
export function poolControl(workers) {
  return new Int32Array(new SharedArrayBuffer((1 + workers * SLOT) * 4));
}

const ERRNO_SUCCESS = 0;
const ERRNO_AGAIN = 6;
const ERRNO_BADF = 8;
const ERRNO_NOSYS = 52;

/**
 * The imports for one instance on `memory`, any thread's. `ctl` is the pool's control block, through which
 * every thread can hand a new one to an idle worker.
 */
export function imports(memory, ctl) {
  const view = () => new DataView(memory.buffer);
  const bytes = (ptr, len) => new Uint8Array(memory.buffer, ptr, len);
  const sleeper = new Int32Array(new SharedArrayBuffer(4));
  const lines = { 1: '', 2: '' };
  // The time origin differs per worker, so threads read the wall clock at performance.now()'s resolution.
  const nanos = () => BigInt(Math.round((performance.timeOrigin + performance.now()) * 1e6));
  const wasi = {
    clock_time_get(_id, _precision, out) {
      view().setBigUint64(out, nanos(), true);
      return ERRNO_SUCCESS;
    },
    random_get(ptr, len) {
      // getRandomValues will not fill a view of shared memory.
      const r = new Uint8Array(len);
      for (let at = 0; at < len; at += 65536) crypto.getRandomValues(r.subarray(at, Math.min(len, at + 65536)));
      bytes(ptr, len).set(r);
      return ERRNO_SUCCESS;
    },
    environ_sizes_get(count, size) {
      view().setUint32(count, 0, true);
      view().setUint32(size, 0, true);
      return ERRNO_SUCCESS;
    },
    environ_get: () => ERRNO_SUCCESS,
    fd_write(fd, iovs, n, written) {
      const v = view();
      let total = 0;
      for (let k = 0; k < n; k++) {
        const ptr = v.getUint32(iovs + k * 8, true);
        const len = v.getUint32(iovs + k * 8 + 4, true);
        if (fd === 1 || fd === 2) lines[fd] += new TextDecoder().decode(bytes(ptr, len).slice());
        total += len;
      }
      if (fd === 1 || fd === 2) {
        const parts = lines[fd].split('\n');
        lines[fd] = parts.pop();
        for (const line of parts) (fd === 2 ? console.error : console.log)('kuroobi_ai:', line);
      }
      v.setUint32(written, total, true);
      return ERRNO_SUCCESS;
    },
    // A sleep: one clock subscription, waited out on a private word.
    poll_oneoff(subs, events, n, count) {
      const v = view();
      let ns = 0n;
      for (let k = 0; k < n; k++) {
        if (v.getUint8(subs + k * 48 + 8) === 0) ns = v.getBigUint64(subs + k * 48 + 24, true);
      }
      const ms = Number(ns) / 1e6;
      if (ms > 0) Atomics.wait(sleeper, 0, 0, ms);
      v.setBigUint64(events, v.getBigUint64(subs, true), true);
      v.setUint16(events + 8, 0, true);
      v.setUint8(events + 10, 0);
      v.setUint32(count, 1, true);
      return ERRNO_SUCCESS;
    },
    sched_yield: () => ERRNO_SUCCESS,
    proc_exit(code) {
      throw new Error('kuroobi_ai: exit ' + code);
    },
    fd_close: () => ERRNO_BADF,
    fd_fdstat_get: () => ERRNO_BADF,
    fd_prestat_get: () => ERRNO_BADF,
    fd_prestat_dir_name: () => ERRNO_BADF,
    fd_read: () => ERRNO_BADF,
    path_open: () => ERRNO_NOSYS,
  };
  const spawn = (arg) => {
    for (let at = 1; at < ctl.length; at += SLOT) {
      if (Atomics.compareExchange(ctl, at, IDLE, CLAIMED) !== IDLE) continue;
      const tid = Atomics.add(ctl, 0, 1) + 1;
      Atomics.store(ctl, at + 1, tid);
      Atomics.store(ctl, at + 2, arg);
      Atomics.store(ctl, at + 0, READY);
      Atomics.notify(ctl, at);
      return tid;
    }
    console.error('kuroobi_ai: every pool worker is busy');
    return -ERRNO_AGAIN;
  };
  return { env: { memory }, wasi_snapshot_preview1: wasi, wasi: { 'thread-spawn': spawn } };
}

/** A pool worker's life: wait for a thread, run it to its end, wait again. Never returns. */
export function serve(instance, ctl, slot) {
  const at = 1 + slot * SLOT;
  for (;;) {
    for (let s = Atomics.load(ctl, at); s !== READY; s = Atomics.load(ctl, at)) Atomics.wait(ctl, at, s);
    const tid = Atomics.load(ctl, at + 1);
    const arg = Atomics.load(ctl, at + 2);
    try {
      instance.exports.wasi_thread_start(tid, arg);
    } catch (e) {
      console.error('kuroobi_ai: thread', tid, 'died:', e);
    }
    Atomics.store(ctl, at, IDLE);
  }
}
