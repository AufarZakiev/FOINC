# Module: Pyodide Runtime

## Purpose
Browser-side Web Worker that loads Pyodide, executes a scientist's Python script against CSV rows using a stdin/stdout contract, and performs timed dry runs on the first 3 rows.

## State Machine

### Worker Lifecycle

| State | Event | → State | Side effect |
|-------|-------|---------|-------------|
| Unloaded | `init` message received | Loading | Begin fetching Pyodide from `cdn.jsdelivr.net/pyodide/` |
| Loading | Pyodide ready | Idle | Post `{ type: "ready" }` to host |
| Loading | Pyodide fetch/init fails | Error | Post `{ type: "error", message }` to host |
| Idle | `exec` message received | Running | Patch `sys.stdin` with provided CSV data; redirect `sys.stdout`/`sys.stderr`; start `performance.now()` timer; start 30 s timeout timer |
| Running | Script completes | Idle | Post `{ type: "result", stdout, stderr, durationMs }` to host |
| Running | Script raises exception | Idle | Post `{ type: "error", message }` to host (message = traceback string) |
| Running | stdout exceeds 10 MB | Idle | Terminate execution; post `{ type: "error", message: "stdout limit exceeded (10 MB)" }` |
| Running | 30 s timeout fires | Terminated | Terminate the Worker entirely; host receives no further messages from this worker instance |
| Error | — | — | Terminal; host must create a new Worker to retry |
| Terminated | — | — | Terminal; host must create a new Worker to retry |

### Dry Run

| State | Event | → State | Side effect |
|-------|-------|---------|-------------|
| Pending | `dryRun(script, csvRows)` called | Executing | Send rows 1, 2, 3 sequentially as individual `exec` calls to the Worker |
| Executing | All 3 rows return results | Complete | Return `DryRunResult` to caller |
| Executing | Any row returns error | Failed | Return the error to caller; do not execute remaining rows |
| Executing | Worker enters Terminated state | Failed | Return timeout error to caller |
| Pending | Worker init fails (enters Error state) | Failed | Return init error to caller |

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
  "stdinData": "<header line>\n<data line>"
}
```
Executes the script with `stdinData` piped to `sys.stdin`. `stdinData` is a two-line string: the first line is the CSV header, the second line is the data row. Only valid when Worker is in `Idle` state.

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

`createPyodideWorker(): PyodideWorker`
Creates and returns a Worker wrapper. Does not send `init` — caller must call `init()` on the returned handle.

`PyodideWorker.init(): Promise<void>`
Sends the `init` message; resolves when `ready` is received; rejects on `error`.

`PyodideWorker.exec(script: string, stdinData: string): Promise<ExecResult>`
Sends an `exec` message; resolves with `{ stdout: string, stderr: string, durationMs: number }`; rejects on `error` or timeout. If the 30 s timeout fires, the Worker is terminated and subsequent calls reject immediately.

`PyodideWorker.terminate(): void`
Forcibly terminates the underlying Web Worker.

`dryRun(script: string, csvRows: string[], header: string): Promise<DryRunResult>`
Creates a Worker, initializes it, executes `script` against each of the first 3 `csvRows` (each prefixed with `header`), collects results. Returns `DryRunResult`. Terminates the Worker when done. `csvRows` must contain exactly 3 rows (caller slices).

### Resource Limits

| Resource | Limit | Enforcement |
|----------|-------|-------------|
| Execution time per `exec` call | 30 s | Worker termination via `Worker.terminate()` |
| stdout size per `exec` call | 10 MB | Checked during stdout capture; execution aborted if exceeded |
| WASM memory | 256 MB | Managed by Pyodide internally (WASM linear memory limit) |

### Sandbox Restrictions

The Python environment has no access to:
- Filesystem (no `open()`, no `pathlib`)
- Network (no `urllib`, no `requests`, no sockets)
- `subprocess`, `os.system`, `ctypes`

These are enforced by not providing the relevant Emscripten/Pyodide modules. Pyodide packages are loaded lazily on first `import` within a script (not eagerly at init).

## Non-goals
- No aggregation detection — out of scope for this module.
- No persistent state between `exec` calls — each execution is isolated.
- No backend component — this module runs entirely in the browser.
- No configurable row count for dry runs — fixed at 3 rows.
