/**
 * Pyodide Web Worker script.
 *
 * Lifecycle: Unloaded -> Loading -> Idle -> Running -> Idle (or Error/Terminated).
 * Receives `init` and `exec` messages from host; posts `ready`, `result`, or `error` back.
 */

declare function importScripts(...urls: string[]): void;

// Pyodide global set by importScripts
declare const loadPyodide: (options?: Record<string, unknown>) => Promise<PyodideInterface>;

interface PyodideInterface {
  runPython(code: string): unknown;
  globals: {
    get(name: string): unknown;
    set(name: string, value: unknown): void;
  };
  loadPackagesFromImports(code: string): Promise<void>;
}

type WorkerState = "unloaded" | "loading" | "idle" | "running" | "error" | "terminated";

const PYODIDE_CDN_URL = "https://cdn.jsdelivr.net/pyodide/v0.27.5/full/pyodide.js";
const STDOUT_LIMIT_BYTES = 10 * 1024 * 1024; // 10 MB

let state: WorkerState = "unloaded";
let pyodide: PyodideInterface | null = null;

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
      importScripts(PYODIDE_CDN_URL);
      pyodide = await loadPyodide();

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
      const message = e instanceof Error ? e.message : String(e);
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

    try {
      // Pass stdinData to the Python environment via globals
      pyodide!.globals.set("__stdin_data__", stdinData);

      // Patch sys.stdin with stdinData, capture stderr with StringIO,
      // and capture stdout with a limit-checking wrapper that aborts
      // execution when 10 MB is exceeded.
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
      await pyodide!.loadPackagesFromImports(script);

      // Execute the user script
      pyodide!.runPython(script);

      // Capture stdout and stderr
      stdoutContent = pyodide!.runPython("sys.stdout.getvalue()") as string;
      stderrContent = pyodide!.runPython("sys.stderr.getvalue()") as string;
    } catch (e: unknown) {
      // Clean up Python namespace before reporting the error
      cleanupPythonNamespace();
      state = "idle";
      const message = e instanceof Error ? e.message : String(e);
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
