# Module: Pyodide Runtime

## Purpose
Browser-side Web Worker that loads Pyodide, executes a scientist's Python script against CSV rows using an argv/stdout contract, and performs timed dry runs on 1 to 3 rows.

## State Machine

### Worker Lifecycle

| State | Event | → State | Side effect |
|-------|-------|---------|-------------|
| Unloaded | `init` message received | Loading | Begin fetching Pyodide from `cdn.jsdelivr.net/pyodide/` |
| Loading | Pyodide ready | Idle | Post `{ type: "ready" }` to host |
| Loading | Pyodide fetch/init fails | Error | Post `{ type: "error", message }` to host |
| Idle | `exec` message received | Running | (1) Call `pyodide.loadPackagesFromImports(script)` to let Pyodide install any imported packages (numpy, scipy, pandas, etc.); (2) apply the just-in-time network sandbox (see Sandbox Restrictions); (3) patch `sys.argv = ['<user-script>', ...argv]`; (4) patch `sys.stdin` to empty `StringIO('')`; (5) redirect `sys.stdout`/`sys.stderr`; (6) start `performance.now()` timer; (7) start 30 s timeout timer; (8) `exec` the user script. Steps 1-2 MUST happen in this order: package install runs unsandboxed so Pyodide's loader (which uses `pathlib`, `ctypes`, `open`) works; the sandbox is applied only after install completes, immediately before user code runs. The 30 s timeout timer (step 7) is conceptually scoped to the entire `Running` phase, but for implementation simplicity it is started only in step 7; to make package-load latency count against the timeout, step 1 is additionally wrapped in a `Promise.race` against the same 30 s budget computed from the `Idle → Running` transition start, so a hanging `loadPackagesFromImports` trips the `30 s timeout fires` transition. |
| Running | Script completes | Idle | Post `{ type: "result", stdout, stderr, durationMs }` to host |
| Running | Script raises exception | Idle | Post `{ type: "error", message }` to host (message = traceback string). This row also covers any `RuntimeError` raised by a sandbox stub when user code touches a blocked surface (see Sandbox Restrictions → Error propagation); no separate transition is needed. |
| Running | `loadPackagesFromImports` (step 1) fails | Idle | Post `{ type: "error", message: "package load failed: " + underlying.message }` to host. Covers CDN failures, unknown package names, and wheel install errors. |
| Running | Sandbox application (steps 2-5) fails | Idle | Post `{ type: "error", message: "sandbox setup failed: " + underlying.message }` to host. Not expected in practice (see Sandbox Restrictions → Failure mode); listed for completeness so the state machine is total. |
| Running | stdout exceeds 10 MB | Idle | Terminate execution; post `{ type: "error", message: "stdout limit exceeded (10 MB)" }` |
| Running | 30 s timeout fires | Terminated | Terminate the Worker entirely; host receives no further messages from this worker instance. Covers both a hanging `loadPackagesFromImports` (step 1) and a hanging user script (step 8); the budget is a single 30 s window starting at the `Idle → Running` transition. |
| Error | — | — | Terminal; host must create a new Worker to retry |
| Terminated | — | — | Terminal; host must create a new Worker to retry |

### Dry Run

| State | Event | → State | Side effect |
|-------|-------|---------|-------------|
| Validating | `dryRun(script, csvRows)` called with 1-3 rows | Initializing | Create Worker; call `init()` |
| Validating | `dryRun` called with 0 or >3 rows | Failed | Return error "dryRun requires 1 to 3 CSV rows" to caller; do not create Worker |
| Initializing | Worker posts `ready` | Executing | Send first `exec` message |
| Initializing | Worker init fails (enters Error state) | Failed | Return init error to caller; terminate Worker |
| Executing | All rows return results | Complete | Return `DryRunResult` to caller; terminate Worker |
| Executing | Any row returns error | Failed | Return the error to caller; do not execute remaining rows; terminate Worker |
| Executing | Worker enters Terminated state | Failed | Return timeout error to caller |

## API / Interface

### Worker postMessage Protocol

**Host → Worker**

`init`
```json
{ "type": "init" }
```
Triggers Pyodide loading. Must be sent exactly once after Worker creation.

`exec`
```json
{
  "type": "exec",
  "script": "<python source code>",
  "argv": ["1", "3"]
}
```
Executes the script with `sys.argv = ["<user-script>", ...argv]` and `sys.stdin` patched to empty `StringIO("")`. `argv` is the CSV data row pre-split by the host into fields (header is NOT included). Only valid when Worker is in `Idle` state.

Each `exec` call carries the script plus the CSV row pre-split into fields. For a dry run of N rows (1-3), `dryRun` issues N `exec` calls, each with one pre-split `argv` array.

**Worker → Host**

`ready`
```json
{ "type": "ready" }
```
Pyodide loaded and initialized.

`result`
```json
{
  "type": "result",
  "stdout": "<captured stdout>",
  "stderr": "<captured stderr>",
  "durationMs": 42.7
}
```
`durationMs`: wall-clock time of script execution measured via `performance.now()`.

`error`
```json
{
  "type": "error",
  "message": "<human-readable error string>"
}
```
Sent on Pyodide load failure, script exception, or stdout limit exceeded.

### Exported TypeScript Functions

`DryRunResult` and `ExecResult` are module-internal TypeScript types (not in `integrations/`); their shapes are defined inline below as part of the function signatures that expose them.

`createPyodideWorker(): PyodideWorker`
Creates and returns a Worker wrapper. Does not send `init` — caller must call `init()` on the returned handle.

`PyodideWorker.init(): Promise<void>`
Sends the `init` message; resolves when `ready` is received; rejects on `error`.

`PyodideWorker.exec(script: string, argv: string[]): Promise<ExecResult>`
Sends an `exec` message; resolves with `{ stdout: string, stderr: string, durationMs: number }`; rejects on `error` or timeout. If the 30 s timeout fires, the Worker is terminated and subsequent calls reject immediately.

`PyodideWorker.terminate(): void`
Forcibly terminates the underlying Web Worker.

`dryRun(script: string, csvRows: string[]): Promise<DryRunResult>`
Creates a Worker, initializes it, splits each row in `csvRows` via `row.split(",")` to produce an `argv` array, executes `script` against each `argv` (no header), and collects results. Returns `DryRunResult`. Terminates the Worker when done. `csvRows` must contain 1, 2, or 3 rows; otherwise rejects with "dryRun requires 1 to 3 CSV rows".

CSV-split limitation: values containing commas are not supported; rows are split on the first and every subsequent comma without quoting/escaping awareness (not RFC 4180-compliant).

`DryRunResult` shape:
```
{
  rows: Array<{ input: string; stdout: string; stderr: string; durationMs: number }>;
  totalDurationMs: number;
}
```
`rows[i].input` is the raw CSV data line for row `i`. `totalDurationMs` is the sum of `rows[*].durationMs`.

### UI Components

Vue 3 + TypeScript Single File Components under `modules/pyodide-runtime/ui/`.

This module does not accept files or user text input — that is the upload module's job. UI components here consume an `UploadCompleted` payload (see `integrations/ui/events.ts`) and run a dry run against it.

`DryRunResults.vue`
Displays the outcome of a dry run.
- Props: `result: DryRunResult`.
- Emits: none.
- UI: a table with one row per CSV data row, columns: input (the raw CSV line), stdout (a `<pre>` block), stderr (a `<pre>` block), `durationMs`. Footer shows `totalDurationMs`.

`DryRunPanel.vue`
Runs a dry run for an already-uploaded job and renders the outcome.
- Props: `upload: UploadCompleted` (from `integrations/ui/events.ts`). Required.
- Emits: `notify: [payload: Toast]` (`Toast` from `integrations/ui/notifications.ts`). Used to surface transient errors from `dryRun` rejections to the frontend shell's `ToastContainer`; the panel does not render toasts itself.
- State: `loading: boolean`, `result: DryRunResult | null`. No local error state.
- Data flow:
  1. From `upload.csv`, split on `\n`, trim each line, discard lines empty after trimming. Drop the first remaining line (CSV header). The next 1-3 lines become `csvRows`.
  2. If parsing yields 0 rows, render an inline message `"CSV must contain at least 1 data row"` and do not call `dryRun`. This is a steady-state hint about the current `upload` prop (not a transient error), so it is rendered inline rather than emitted as a toast — it must remain visible until the prop changes to valid CSV.
  3. If parsing yields >3 rows, use only the first 3 (caller slice matches `dryRun`'s contract).
  4. Render a "Run dry run" button. On click: set `loading=true`, call `dryRun(upload.script, csvRows)`; on resolve store `result` and render `DryRunResults`; on reject emit `notify` with `{ level: "error", message: tail(error.message) }` and do not store the error locally or render it inline. After either outcome, clear `loading`. After `loading` clears on reject, the panel returns to the idle-button state (no `result`, no local error).
  5. Toast message derivation (`tail`): the worker's `error.message` is a full Python traceback, which is unusable in a toast. `tail(message)` extracts the last non-empty line of `message` (typically the `RuntimeError: network access is disabled` tail, or the last exception-summary line produced by Pyodide). Concretely: split `message` on `\n`, drop trailing empty strings, take the last element; if the result is empty, fall back to the original `message`. The full traceback is not preserved anywhere inside `DryRunPanel` — `dryRun` rejects without producing a per-row result on failure (see state machine `Executing | Any row returns error | Failed`), so there is no per-row `error` field to populate. A future success-path for partial results could store full tracebacks in the `DryRunResult.rows[*].stderr` field; that is out of scope for this module today.
- Only one of idle-button / loading / result / csv-invalid-message is visible at a time. There is no inline error state.
- When the `upload` prop changes to a new payload, reset `result` to `null`.

### Resource Limits

| Resource | Limit | Enforcement |
|----------|-------|-------------|
| Execution time per `exec` call | 30 s | Worker termination via `Worker.terminate()` |
| stdout size per `exec` call | 10 MB | Checked during stdout capture; execution aborted if exceeded |
| WASM memory | 256 MB | Managed by Pyodide internally (WASM linear memory limit) |

### Sandbox Restrictions

Two threat models, handled differently:

**1. Host filesystem isolation — given by the environment.**
Pyodide runs in a Web Worker with an in-memory MEMFS that has no path to the host filesystem and is destroyed with the worker. No Python-level enforcement is required or attempted. `open`, `pathlib`, and filesystem calls remain available to user scripts; they can only touch the worker's ephemeral MEMFS.

**2. Network egress isolation — enforced just-in-time, per `exec`.**
The only exfiltration path is the browser's network APIs reachable through Pyodide's JS bridge or Python stdlib. These MUST be blocked before user code runs. Every mechanism below is **Python-side only**: attribute replacement via `setattr` on live Python module objects, and entries in `sys.modules`. No JS-side globals (`globalThis.fetch`, `self.fetch`, etc.) are touched — see "JS-side scope" below for why.

| Surface | Mechanism | Import-time behaviour | First-use behaviour |
|---------|-----------|-----------------------|---------------------|
| `pyodide.http.pyfetch` | `setattr(pyodide.http, "pyfetch", stub)` where `stub` raises `RuntimeError("network access is disabled")`. Only applied if `pyodide.http` is already present in `sys.modules` at sandbox time; do not force-import it. If user code imports `pyodide.http` later, the replacement is not reapplied; this is safe because `pyodide.http.pyfetch` is implemented on top of `js.fetch`, which is poisoned in the next row. | — | `pyfetch(...)` raises `RuntimeError` (either directly from the stub, or — if imported post-sandbox — from the poisoned `js.fetch` on which `pyfetch` depends). |
| `pyodide.code.run_js`, `pyodide.ffi.run_js` | `setattr(pyodide.code, "run_js", stub)` and, if present in `sys.modules`, `setattr(pyodide.ffi, "run_js", stub)`, where `stub` raises `RuntimeError("dynamic JS evaluation is disabled")`. This closes the only Python-reachable route from user code to raw JS eval. | — | `run_js(...)` raises `RuntimeError`. |
| `js.fetch`, `js.XMLHttpRequest`, `js.WebSocket`, `js.navigator.sendBeacon` | The `js` module is a Pyodide-provided proxy to `globalThis`. For each named attribute, perform `setattr(js, "<name>", stub)` from Python, where `stub` is a Python callable that raises `RuntimeError("network access is disabled")`. This rebinds the Python-side `js.<name>` lookup; it does NOT mutate `globalThis.<name>` on the JS side. | — | `js.fetch(...)` etc. raise `RuntimeError`. For `js.navigator.sendBeacon`, replace `sendBeacon` on the `js.navigator` proxy, not on `js`. |
| `socket`, `urllib.request`, `urllib.error`, `http.client`, `ftplib`, `smtplib`, `telnetlib`, `poplib`, `imaplib`, `nntplib`, `xmlrpc.client` | Install a stub `types.ModuleType` instance into `sys.modules` under each name. The stub defines `__getattr__(name)` that raises `RuntimeError("network access is disabled")`. **Import itself succeeds** (because `sys.modules[name]` is a real module object, not `None`); the `RuntimeError` fires on first attribute access, producing a traceback with the sandbox's standard message. This is the intentional choice over `sys.modules[name] = None`, which would raise `ImportError` at `import` time with a generic message. | `import socket` succeeds; etc. | `socket.socket(...)`, `urllib.request.urlopen(...)`, etc. raise `RuntimeError`. |
| `urllib.parse` | Not blocked. | Normal import. | Normal use. |

**JS-side scope.** This sandbox poisons Python-side bindings only. `globalThis.fetch`, `self.fetch`, and similar JS globals on the worker remain reachable in principle, but **only** from JS code. User scripts are Python; the two Python-reachable routes into JS (`js.<name>` proxy attribute access and `pyodide.code.run_js` / `pyodide.ffi.run_js`) are both poisoned in the table above. Therefore JS-side global poisoning is intentionally out of scope: there is no remaining Python-reachable caller that could invoke them.

**Error propagation for blocked operations.** When a sandbox stub raises `RuntimeError("…")`, it propagates like any other script exception: the user script's `exec` call unwinds with a traceback, the `Running | Script raises exception | Idle` transition fires, the worker posts `{ type: "error", message: <traceback> }` to the host, the `dryRun` promise rejects with that message, and `DryRunPanel` emits a `notify` toast (toast payload shape: see UI Components → `DryRunPanel.vue`). No new state, no new message type.

**Failure mode of the sandbox itself.** The mechanisms above are pure Python dict assignments (`sys.modules[name] = stub_module`) and `setattr` on well-known module objects (`pyodide.http`, `pyodide.code`, `pyodide.ffi`, `js`). On a functioning Pyodide instance these operations cannot raise — the modules are standard, the dict is writable, and the attribute names are all valid Python identifiers. If any of them does raise, the Pyodide instance is assumed broken: the runtime catches the exception in step 2-5 and surfaces it via the `Running | Sandbox application (steps 2-5) fails | Idle` transition defined in the State Machine. User code never runs in that case.

**Timing requirement.** All of the above MUST be applied inside the `Idle → Running` transition, *after* `pyodide.loadPackagesFromImports(script)` returns successfully and *before* the user script is `exec`-ed. Package installation (numpy, scipy, openblas, etc.) uses Pyodide's own loader, which depends on `pathlib`, `ctypes`, and `open`; applying the sandbox at worker init breaks legitimate package loading and MUST NOT be done.

**Not blocked, intentionally.** `pathlib`, `ctypes`, `builtins.open`, the `os` module (in full), `subprocess`, and the full Python stdlib for CPU/in-memory work (math, itertools, collections, json, csv, re, statistics, …) remain available to user scripts. Filesystem isolation is handled by the worker boundary, not by removing modules.

`subprocess` (and the `os.system` / `os.popen` / `os.exec*` / `os.spawn*` / `os.posix_spawn*` / `os.forkpty` / `os.fork` family) is deliberately left unblocked for three reasons: (1) Pyodide's WASM runtime has no `fork`/`exec` primitives, so `subprocess.Popen(...)` and `os.system(...)` fail naturally with `OSError` — additional Python-side enforcement adds no isolation; (2) scientific libraries (scipy, numpy, matplotlib, scikit-learn, statsmodels) probe subprocess-ish calls behind `try/except OSError` during import, and a synthetic `RuntimeError` short-circuits those defensive paths and surfaces as a spurious failure in the user's script; (3) the sandbox's threat model is **network egress isolation only**, and `subprocess` cannot open network connections — even if a child process could spawn (it can't), it has no path to inherit the worker's browser-scoped `fetch`. Blocking subprocess surfaces was "by association" with network stdlib, not by threat analysis, and is removed here.

### Supported Packages

The runtime MUST execute user scripts that import any package shipped in the default Pyodide distribution. The following packages are guaranteed to work end-to-end (import + basic use) and are the reference set for conformance:

- `numpy`
- `scipy`
- `pandas`

Additional packages from the Pyodide distribution (including but not limited to `sympy`, `matplotlib`, `scikit-learn`, `statsmodels`, `networkx`) are expected to load via `loadPackagesFromImports` on first import. The module does NOT ship a custom package index or wheel mirror; whatever Pyodide's default CDN serves is what user scripts get.

## Non-goals
- Module stays isolated: no file upload / picker / text input (upload module's job), no aggregation detection, no persistent state between `exec` calls, and no backend component — each execution runs entirely in the browser against an already-uploaded `UploadCompleted`.
- Input/output simplifications: dry runs are capped at 3 rows with no partial results on failure; CSV parsing is not RFC 4180-compliant (commas are a hard separator, no quoting/escaping/embedded newlines); stdout is rendered as raw text in a `<pre>` block, not as CSV-structured output.
- No mounting of `DryRunPanel` — composition and routing live in `frontend/`, not in this module.
- No inline toast rendering — the frontend shell owns `ToastContainer`; this module only emits `notify` events.
