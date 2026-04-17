/**
 * Pyodide Web Worker script.
 *
 * Lifecycle: Unloaded -> Loading -> Idle -> Running -> Idle (or Error/Terminated).
 * Receives `init` and `exec` messages from host; posts `ready`, `result`, or `error` back.
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

let state: WorkerState = "unloaded";
let pyodide: PyodideInterface | null = null;

/**
 * Produces a non-empty, prefixed error string from an unknown thrown value.
 * Pyodide's PythonError has a `message` but it is sometimes empty; fall back
 * to name, stack, or a JSON/string dump so the host always gets useful info.
 */
function formatError(stage: string, e: unknown): string {
  if (e instanceof Error) {
    let body = e.message;
    if (!body || body.trim().length === 0) {
      // Pyodide's PythonError often has an empty .message but a useful
      // toString(); strip the "<Name>: " prefix if present.
      const str = String(e);
      const prefix = `${e.name}: `;
      body = str.startsWith(prefix) ? str.slice(prefix.length) : str;
    }
    if (!body || body.trim().length === 0) body = e.name || "unknown Error";
    return `[${stage}] ${e.name}: ${body}`;
  }
  if (typeof e === "string" && e.length > 0) return `[${stage}] ${e}`;
  if (e && typeof e === "object") {
    try {
      return `[${stage}] ${JSON.stringify(e)}`;
    } catch {
      return `[${stage}] ${String(e)}`;
    }
  }
  return `[${stage}] ${String(e) || "unknown error"}`;
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

# Remove any user-imported modules added since init
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

      // Sandbox restrictions: remove/block restricted modules and builtins
      pyodide.runPython(`
import sys

# Remove 'open' from builtins
import builtins
if hasattr(builtins, 'open'):
    del builtins.open

# Remove restricted modules from sys.modules and block future imports
_blocked_modules = [
    'pathlib', 'subprocess', 'ctypes',
    'urllib', 'urllib.request', 'urllib.parse', 'urllib.error',
    'requests', 'socket',
]
for _mod in _blocked_modules:
    sys.modules[_mod] = None

# Remove os.system while keeping os available for other safe uses
import os
if hasattr(os, 'system'):
    del os.system
`);

      // Snapshot initial modules so cleanup can detect user-imported modules
      pyodide.runPython(`
import sys
sys._initial_modules = set(sys.modules.keys())
`);

      state = "idle";
      self.postMessage({ type: "ready" });
    } catch (e: unknown) {
      state = "error";
      console.error("[pyodide-worker] init failed:", e);
      const message = formatError("init", e);
      self.postMessage({ type: "error", message });
    }
    return;
  }

  if (msg.type === "exec" && state === "idle") {
    state = "running";
    const script: string = msg.script;
    const stdinData: string = msg.stdinData;

    let stdoutContent = "";
    let stderrContent = "";

    const start = performance.now();

    let execStep = "start";
    try {
      // Pass stdinData to the Python environment via globals
      execStep = "globals.set";
      pyodide!.globals.set("__stdin_data__", stdinData);

      // Patch sys.stdin with stdinData, capture stderr with StringIO,
      // and capture stdout with a limit-checking wrapper that aborts
      // execution when 10 MB is exceeded.
      execStep = "patch-io";
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

sys.stdin = io.StringIO(__stdin_data__)
sys.stdout = _LimitedStdout(${STDOUT_LIMIT_BYTES})
sys.stderr = io.StringIO()
del __stdin_data__
`);

      // Load any packages the script imports
      execStep = "loadPackagesFromImports";
      await pyodide!.loadPackagesFromImports(script);

      // Execute the user script inside a Python try/except so we capture
      // the full traceback as a string. Pyodide's PythonError.message can
      // be empty in some cases, so we bypass it by exposing the formatted
      // traceback on a global that we read from JS after runPython returns.
      execStep = "runPython(user script)";
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
        throw new Error(userErr);
      }
      pyodide!.runPython("del __dry_run_error__");

      // Capture stdout and stderr
      execStep = "capture stdout";
      stdoutContent = pyodide!.runPython("sys.stdout.getvalue()") as string;
      execStep = "capture stderr";
      stderrContent = pyodide!.runPython("sys.stderr.getvalue()") as string;
    } catch (e: unknown) {
      console.error(`[pyodide-worker] exec failed at step "${execStep}":`, e);
      // Clean up Python namespace before reporting the error
      cleanupPythonNamespace();
      state = "idle";
      const message = formatError(`exec/${execStep}`, e);
      self.postMessage({ type: "error", message });
      return;
    }

    const durationMs = performance.now() - start;

    // Clean up Python namespace for isolation between exec calls
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
