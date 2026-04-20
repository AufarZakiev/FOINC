/**
 * Pyodide Web Worker script.
 *
 * Lifecycle: Unloaded -> Loading -> Idle -> Running -> Idle (or Error/Terminated).
 * Receives `init` and `exec` messages from host; posts `ready`, `result`, or `error` back.
 *
 * The sandbox is applied JUST-IN-TIME per `exec`, after Pyodide has resolved
 * the script's imports via `loadPackagesFromImports` (which needs `pathlib`,
 * `ctypes`, `open`) and before the user script runs. See spec §Sandbox
 * Restrictions for the exact surface blocked and rationale.
 */

interface PyodideInterface {
  runPython(code: string): unknown;
  globals: {
    get(name: string): unknown;
    set(name: string, value: unknown): void;
  };
  loadPackagesFromImports(code: string): Promise<void>;
}

type WorkerState = "unloaded" | "loading" | "idle" | "running" | "error" | "terminated";

const PYODIDE_CDN_BASE = "https://cdn.jsdelivr.net/pyodide/v0.27.5/full/";
const PYODIDE_ESM_URL = `${PYODIDE_CDN_BASE}pyodide.mjs`;
const STDOUT_LIMIT_BYTES = 10 * 1024 * 1024; // 10 MB
const EXEC_TIMEOUT_MS = 30_000; // 30 s — scoped to the whole Running phase.

let state: WorkerState = "unloaded";
let pyodide: PyodideInterface | null = null;

/**
 * Produces a useful error string from an unknown thrown value.
 * Pyodide's PythonError has a `message` but it is sometimes empty; fall back
 * to toString / name so callers always get something.
 */
function errorMessage(e: unknown): string {
  if (e instanceof Error) {
    let body = e.message;
    if (!body || body.trim().length === 0) {
      const str = String(e);
      const prefix = `${e.name}: `;
      body = str.startsWith(prefix) ? str.slice(prefix.length) : str;
    }
    if (!body || body.trim().length === 0) body = e.name || "unknown Error";
    return body;
  }
  if (typeof e === "string" && e.length > 0) return e;
  if (e && typeof e === "object") {
    try {
      return JSON.stringify(e);
    } catch {
      return String(e);
    }
  }
  return String(e) || "unknown error";
}

/**
 * Resets the Python global namespace and clears user-imported modules
 * from sys.modules, ensuring each exec call starts from a clean state.
 */
function cleanupPythonNamespace(): void {
  if (!pyodide) return;
  try {
    pyodide.runPython(`
import sys

# Snapshot of initial module names (builtins + stdlib loaded at init)
if not hasattr(sys, '_initial_modules'):
    sys._initial_modules = set(sys.modules.keys())

# Remove any user-imported modules added since init. This also drops any
# sandbox stub ModuleType entries we installed for this exec — the next
# exec installs fresh stubs.
_to_remove = [k for k in sys.modules.keys() if k not in sys._initial_modules]
for _k in _to_remove:
    del sys.modules[_k]

# Reset the __main__ namespace to a clean state
import importlib
_main = sys.modules['__main__']
_keep = {'__name__', '__doc__', '__loader__', '__spec__', '__builtins__'}
_to_del = [k for k in vars(_main) if k not in _keep]
for _k in _to_del:
    try:
        delattr(_main, _k)
    except Exception:
        pass
`);
  } catch {
    // Cleanup is best-effort; do not fail the exec result
  }
}

/**
 * Applies the just-in-time network/subprocess sandbox. Runs *after*
 * `loadPackagesFromImports` has resolved the script's imports and
 * *before* the user script is `exec`-ed. See spec §Sandbox Restrictions.
 *
 * Mechanism summary:
 *   - `setattr` on `pyodide.http.pyfetch` (only if already in sys.modules)
 *   - `setattr` on `pyodide.code.run_js` and, if imported, `pyodide.ffi.run_js`
 *   - `setattr` on `js.fetch`, `js.XMLHttpRequest`, `js.WebSocket`, and
 *     `js.navigator.sendBeacon` (Python-side proxy attributes only;
 *     `globalThis.*` on the JS side is intentionally not touched — see spec
 *     "JS-side scope").
 *   - Install `types.ModuleType` stubs into `sys.modules` for `socket`,
 *     `urllib.request`, `urllib.error`, `http.client`, `ftplib`, `smtplib`,
 *     `telnetlib`, `poplib`, `imaplib`, `nntplib`, `xmlrpc.client`, and
 *     `subprocess`. Each stub's `__getattr__` raises `RuntimeError`.
 *   - `setattr` on each enumerated `os.*` subprocess attribute that exists
 *     on the current Pyodide build; silently skip names that are absent.
 *
 * Throws if any of the above unexpectedly fails. Caller wraps the throw
 * into the `"sandbox setup failed: "` error message.
 */
function applySandbox(): void {
  if (!pyodide) return;
  pyodide.runPython(`
import sys
import types

_NET_MSG = "network access is disabled"
_JS_MSG = "dynamic JS evaluation is disabled"
_PROC_MSG = "subprocess execution is disabled"


def _make_stub(message):
    def _stub(*args, **kwargs):
        raise RuntimeError(message)
    return _stub


def _make_stub_module(name, message):
    mod = types.ModuleType(name)
    def __getattr__(attr):
        raise RuntimeError(message)
    mod.__getattr__ = __getattr__
    return mod


# ---- pyodide.http.pyfetch -------------------------------------------------
# Only patch if pyodide.http is already imported. Do not force-import it.
if 'pyodide.http' in sys.modules:
    import pyodide.http as _ph
    setattr(_ph, 'pyfetch', _make_stub(_NET_MSG))

# ---- pyodide.code.run_js / pyodide.ffi.run_js -----------------------------
import pyodide.code as _pc
setattr(_pc, 'run_js', _make_stub(_JS_MSG))
if 'pyodide.ffi' in sys.modules:
    import pyodide.ffi as _pf
    setattr(_pf, 'run_js', _make_stub(_JS_MSG))

# ---- js.fetch / XMLHttpRequest / WebSocket / navigator.sendBeacon ---------
# Python-side bindings only. Does NOT touch globalThis.* on the JS side.
import js as _js
setattr(_js, 'fetch', _make_stub(_NET_MSG))
setattr(_js, 'XMLHttpRequest', _make_stub(_NET_MSG))
setattr(_js, 'WebSocket', _make_stub(_NET_MSG))
# navigator.sendBeacon lives on the js.navigator proxy, not on js directly.
setattr(_js.navigator, 'sendBeacon', _make_stub(_NET_MSG))

# ---- sys.modules stubs for network / subprocess stdlib --------------------
_NET_MODULES = [
    'socket',
    'urllib.request', 'urllib.error',
    'http.client',
    'ftplib', 'smtplib', 'telnetlib', 'poplib', 'imaplib', 'nntplib',
    'xmlrpc.client',
]
for _name in _NET_MODULES:
    sys.modules[_name] = _make_stub_module(_name, _NET_MSG)

sys.modules['subprocess'] = _make_stub_module('subprocess', _PROC_MSG)

# ---- os.* subprocess surface ---------------------------------------------
# setattr (NOT delattr): user tracebacks must uniformly read RuntimeError,
# not a mix of AttributeError / RuntimeError. Names absent on the current
# Pyodide build (Pyodide's WASM omits fork/exec already) are skipped.
import os as _os
_OS_BLOCKED = [
    'system', 'popen',
    'execv', 'execvp', 'execve', 'execl', 'execlp', 'execle',
    'execvpe',
    'spawnl', 'spawnle', 'spawnlp', 'spawnlpe',
    'spawnv', 'spawnve', 'spawnvp', 'spawnvpe',
    'posix_spawn', 'posix_spawnp',
    'forkpty', 'fork',
]
_proc_stub = _make_stub(_PROC_MSG)
for _name in _OS_BLOCKED:
    if hasattr(_os, _name):
        setattr(_os, _name, _proc_stub)
`);
}

self.onmessage = async (event: MessageEvent) => {
  const msg = event.data;

  if (msg.type === "init" && state === "unloaded") {
    state = "loading";
    try {
      console.log("[pyodide-worker] init: importing", PYODIDE_ESM_URL);
      // Dynamic ESM import of Pyodide from CDN. Works in module workers
      // without needing importScripts (which is classic-only).
      const pyodideModule: {
        loadPyodide: (options?: Record<string, unknown>) => Promise<PyodideInterface>;
      } = await import(/* @vite-ignore */ PYODIDE_ESM_URL);
      console.log("[pyodide-worker] init: loadPyodide starting");
      pyodide = await pyodideModule.loadPyodide({ indexURL: PYODIDE_CDN_BASE });
      console.log("[pyodide-worker] init: loadPyodide done");

      // Snapshot initial modules so per-exec cleanup can detect
      // user-imported modules (and sandbox stubs) added later.
      // No sandbox is applied here — applying it at init would break
      // Pyodide's own loader, which uses pathlib / ctypes / open.
      pyodide.runPython(`
import sys
sys._initial_modules = set(sys.modules.keys())
`);

      state = "idle";
      self.postMessage({ type: "ready" });
    } catch (e: unknown) {
      state = "error";
      console.error("[pyodide-worker] init failed:", e);
      self.postMessage({ type: "error", message: errorMessage(e) });
    }
    return;
  }

  if (msg.type === "exec" && state === "idle") {
    state = "running";
    const script: string = msg.script;
    const argv: string[] = msg.argv;

    let stdoutContent = "";
    let stderrContent = "";

    const start = performance.now();

    // Single 30 s budget scoped to the entire Running phase. The timer's
    // promise, when it fires, wins any Promise.race it is attached to;
    // if it ever wins, we terminate the worker without posting any more
    // messages (spec: "30 s timeout fires → Terminated; host receives no
    // further messages from this worker instance"). The host wrapper
    // (pyodideWorker.ts) has its own 30 s timer that also calls terminate,
    // so a runaway synchronous user script is killed there.
    let timeoutId: ReturnType<typeof setTimeout> | undefined;
    const timeoutPromise = new Promise<never>((_resolve, reject) => {
      timeoutId = setTimeout(() => {
        reject(new Error("__PYODIDE_WORKER_TIMEOUT__"));
      }, EXEC_TIMEOUT_MS);
    });

    const runPhase = (async () => {
      // Step 1: Load packages the script imports. Runs unsandboxed.
      try {
        await pyodide!.loadPackagesFromImports(script);
      } catch (e: unknown) {
        throw new Error("package load failed: " + errorMessage(e));
      }

      // Steps 2–5: Apply sandbox + patch stdio/argv. Wrapped together
      // because a failure in either is "the Pyodide instance is broken"
      // per spec §Sandbox Restrictions → Failure mode, and both fall
      // under the "Sandbox application (steps 2-5) fails" transition.
      try {
        applySandbox();

        pyodide!.globals.set("__argv__", argv);
        pyodide!.runPython(`
import sys
import io

class _LimitedStdout:
    def __init__(self, limit):
        self._buf = io.StringIO()
        self._byte_count = 0
        self._limit = limit

    def write(self, s):
        encoded_len = len(s.encode('utf-8'))
        self._byte_count += encoded_len
        if self._byte_count > self._limit:
            raise RuntimeError("stdout limit exceeded (10 MB)")
        return self._buf.write(s)

    def getvalue(self):
        return self._buf.getvalue()

    def flush(self):
        pass

sys.argv = ['<user-script>', *list(__argv__)]
sys.stdin = io.StringIO("")
sys.stdout = _LimitedStdout(${STDOUT_LIMIT_BYTES})
sys.stderr = io.StringIO()
del __argv__
`);
      } catch (e: unknown) {
        throw new Error("sandbox setup failed: " + errorMessage(e));
      }

      // Step 8: Execute the user script. Captures full traceback as a
      // string because Pyodide's PythonError.message can be empty.
      pyodide!.globals.set("__user_script__", script);
      pyodide!.runPython(`
import traceback as _tb
try:
    exec(compile(__user_script__, '<user-script>', 'exec'), globals())
    __dry_run_error__ = None
except BaseException:
    __dry_run_error__ = _tb.format_exc()
finally:
    del __user_script__
`);
      const userErr = pyodide!.globals.get("__dry_run_error__");
      if (typeof userErr === "string" && userErr.length > 0) {
        pyodide!.runPython("del __dry_run_error__");
        // Surface stdout-limit-exceeded with the exact spec-mandated
        // message rather than the wrapping traceback. Other script
        // exceptions pass through as the full traceback string.
        if (userErr.includes("stdout limit exceeded (10 MB)")) {
          throw new Error("stdout limit exceeded (10 MB)");
        }
        throw new Error(userErr);
      }
      pyodide!.runPython("del __dry_run_error__");

      stdoutContent = pyodide!.runPython("sys.stdout.getvalue()") as string;
      stderrContent = pyodide!.runPython("sys.stderr.getvalue()") as string;
    })();

    try {
      await Promise.race([runPhase, timeoutPromise]);
    } catch (e: unknown) {
      if (timeoutId !== undefined) clearTimeout(timeoutId);

      // Worker-internal 30 s timeout fired. Terminate the worker entirely;
      // the host receives no further messages from this instance. The host's
      // own timer (pyodideWorker.ts) will also terminate — this is a safety
      // net for a hanging `loadPackagesFromImports`.
      if (e instanceof Error && e.message === "__PYODIDE_WORKER_TIMEOUT__") {
        state = "terminated";
        self.close();
        return;
      }

      cleanupPythonNamespace();
      state = "idle";
      self.postMessage({ type: "error", message: errorMessage(e) });
      return;
    }

    if (timeoutId !== undefined) clearTimeout(timeoutId);

    const durationMs = performance.now() - start;

    // Clean up Python namespace (and drop sandbox stubs) for isolation
    // between exec calls.
    cleanupPythonNamespace();

    state = "idle";
    self.postMessage({
      type: "result",
      stdout: stdoutContent,
      stderr: stderrContent,
      durationMs,
    });
    return;
  }
};
