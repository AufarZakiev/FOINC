/**
 * Source-level regression tests for worker.ts.
 *
 * Real Pyodide cannot run in jsdom/Node, so these tests parse the worker
 * source as a string and assert structural invariants. They are ugly, but
 * they catch the exact regressions we care about:
 *
 *   - The sandbox is applied JUST-IN-TIME per exec, NEVER at init. The
 *     original bug was the sandbox running at init-time and poisoning
 *     `pathlib` in `sys.modules`, which broke Pyodide's own
 *     `_package_loader.py` — any user script importing numpy/scipy/pandas
 *     would cascade-fail. These tests would have caught that bug.
 *
 *   - `pathlib`, `ctypes`, and `builtins.open` must NOT appear as blocked
 *     surfaces anywhere in the worker.
 *
 *   - The two error-prefix strings mandated by spec
 *     ("package load failed: " and "sandbox setup failed: ") are present
 *     on the right code paths.
 *
 *   - The worker-internal 30 s timeout exists as a Promise.race against
 *     the runPhase.
 *
 * These tests cannot fully substitute for end-to-end execution. An
 * `it.skip` integration test is tracked in `dryRun.integration.test.ts`
 * documenting the real-Pyodide path.
 */

import { describe, it, expect } from "vitest";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);

// worker.ts lives in the parent directory of __tests__/.
const WORKER_PATH = resolve(__dirname, "..", "worker.ts");
const WORKER_SRC = readFileSync(WORKER_PATH, "utf-8");

// ---------------------------------------------------------------------------
// Helpers for slicing worker.ts into well-known regions.
// ---------------------------------------------------------------------------

/**
 * Returns the body of `function applySandbox() { ... }` — the whole sandbox
 * routine. Used to assert that sandbox-only operations (e.g. `setattr(_js,
 * 'fetch', ...)`) live inside applySandbox and nowhere else.
 */
function getApplySandboxBody(): string {
  const marker = "function applySandbox()";
  const start = WORKER_SRC.indexOf(marker);
  if (start === -1) {
    throw new Error("applySandbox() not found in worker.ts");
  }
  // Find the opening brace after the marker.
  const braceOpen = WORKER_SRC.indexOf("{", start);
  // Walk forward counting braces to find the matching close. Good enough
  // for this module's known source shape.
  let depth = 0;
  let i = braceOpen;
  for (; i < WORKER_SRC.length; i++) {
    const c = WORKER_SRC[i];
    if (c === "{") depth++;
    else if (c === "}") {
      depth--;
      if (depth === 0) break;
    }
  }
  return WORKER_SRC.slice(braceOpen, i + 1);
}

/**
 * Returns the body of `function cleanupPythonNamespace() { ... }` — the
 * per-exec cleanup routine. Used to assert the refresh assignment to
 * `sys._initial_modules` happens AFTER the removal loops (otherwise the
 * "don't wipe Pyodide packages between execs" fix is a no-op).
 */
function getCleanupBody(): string {
  const marker = "function cleanupPythonNamespace()";
  const start = WORKER_SRC.indexOf(marker);
  if (start === -1) {
    throw new Error("cleanupPythonNamespace() not found in worker.ts");
  }
  const braceOpen = WORKER_SRC.indexOf("{", start);
  let depth = 0;
  let i = braceOpen;
  for (; i < WORKER_SRC.length; i++) {
    const c = WORKER_SRC[i];
    if (c === "{") depth++;
    else if (c === "}") {
      depth--;
      if (depth === 0) break;
    }
  }
  return WORKER_SRC.slice(braceOpen, i + 1);
}

/**
 * Returns the body of the `if (msg.type === "init" && ...)` block — the
 * init-time code path. Used to assert that sandbox operations are NOT
 * present here.
 */
function getInitBlockBody(): string {
  const marker = 'if (msg.type === "init"';
  const start = WORKER_SRC.indexOf(marker);
  if (start === -1) {
    throw new Error('init block (msg.type === "init") not found in worker.ts');
  }
  const braceOpen = WORKER_SRC.indexOf("{", start);
  let depth = 0;
  let i = braceOpen;
  for (; i < WORKER_SRC.length; i++) {
    const c = WORKER_SRC[i];
    if (c === "{") depth++;
    else if (c === "}") {
      depth--;
      if (depth === 0) break;
    }
  }
  return WORKER_SRC.slice(braceOpen, i + 1);
}

/**
 * Returns the body of the `if (msg.type === "exec" && ...)` block — the
 * exec-time code path. Used to assert that sandbox setup + error prefixes
 * live here.
 */
function getExecBlockBody(): string {
  const marker = 'if (msg.type === "exec"';
  const start = WORKER_SRC.indexOf(marker);
  if (start === -1) {
    throw new Error('exec block (msg.type === "exec") not found in worker.ts');
  }
  const braceOpen = WORKER_SRC.indexOf("{", start);
  let depth = 0;
  let i = braceOpen;
  for (; i < WORKER_SRC.length; i++) {
    const c = WORKER_SRC[i];
    if (c === "{") depth++;
    else if (c === "}") {
      depth--;
      if (depth === 0) break;
    }
  }
  return WORKER_SRC.slice(braceOpen, i + 1);
}

// ---------------------------------------------------------------------------
// Item 7 (most important regression): sandbox is NOT applied at init-time
// ---------------------------------------------------------------------------

describe("worker.ts — sandbox is applied just-in-time (regression for init-time poisoning)", () => {
  it("applySandbox() is NOT called from the init code path", () => {
    const initBody = getInitBlockBody();
    // The init block must not invoke applySandbox directly.
    expect(initBody).not.toMatch(/\bapplySandbox\s*\(/);
  });

  it("applySandbox() IS called from the exec code path", () => {
    const execBody = getExecBlockBody();
    expect(execBody).toMatch(/\bapplySandbox\s*\(/);
  });

  it("pathlib is NOT listed as a blocked stdlib module anywhere in worker.ts", () => {
    // Original bug: `sys.modules['pathlib'] = None` (or a stub) was
    // installed at init, which broke Pyodide's own `_package_loader.py`.
    // The fix drops pathlib from the block list entirely.
    // Looking for either "'pathlib'" or '"pathlib"' as a blocked-name literal.
    expect(WORKER_SRC).not.toMatch(/['"]pathlib['"]/);
  });

  it("ctypes is NOT listed as a blocked stdlib module anywhere in worker.ts", () => {
    // Same rationale as pathlib — Pyodide's loader uses ctypes.
    expect(WORKER_SRC).not.toMatch(/['"]ctypes['"]/);
  });

  it("builtins.open / del builtins.open is NOT present anywhere in worker.ts", () => {
    // Original bug included removing builtins.open. The fix keeps open
    // available to user scripts (filesystem isolation is given by the
    // worker boundary, not by removing open).
    expect(WORKER_SRC).not.toMatch(/\bbuiltins\.open\b/);
    expect(WORKER_SRC).not.toMatch(/\bdel\s+builtins\b/);
  });

  it("loadPackagesFromImports is called BEFORE applySandbox in the exec body", () => {
    const execBody = getExecBlockBody();
    const loadIdx = execBody.indexOf("loadPackagesFromImports");
    const sandboxIdx = execBody.search(/\bapplySandbox\s*\(/);
    expect(loadIdx).toBeGreaterThan(-1);
    expect(sandboxIdx).toBeGreaterThan(-1);
    // Step 1 (package install) must precede step 2 (sandbox apply).
    expect(loadIdx).toBeLessThan(sandboxIdx);
  });

  it("js module poisoning literals live inside applySandbox(), not elsewhere", () => {
    // `setattr(_js, 'fetch', ...)` is a sandbox-only operation. If it
    // ever leaks out of applySandbox, the regression is back.
    const sandboxBody = getApplySandboxBody();
    expect(sandboxBody).toMatch(/setattr\(_js,\s*['"]fetch['"]/);

    // And the same literal must not appear outside applySandbox's body.
    const outsideSandbox =
      WORKER_SRC.slice(0, WORKER_SRC.indexOf(sandboxBody)) +
      WORKER_SRC.slice(
        WORKER_SRC.indexOf(sandboxBody) + sandboxBody.length,
      );
    expect(outsideSandbox).not.toMatch(/setattr\(_js,\s*['"]fetch['"]/);
  });
});

// ---------------------------------------------------------------------------
// Items 4-5: error prefixes on load / sandbox failure paths
// ---------------------------------------------------------------------------

describe("worker.ts — error prefix regression (spec-mandated wording)", () => {
  it('"package load failed: " prefix is wired to the loadPackagesFromImports failure path', () => {
    // The spec requires exactly this prefix for step 1 failures.
    expect(WORKER_SRC).toContain('"package load failed: "');

    // And it must be in the exec block, adjacent to the
    // loadPackagesFromImports call.
    const execBody = getExecBlockBody();
    expect(execBody).toContain('"package load failed: "');
    expect(execBody).toContain("loadPackagesFromImports");
  });

  it('"sandbox setup failed: " prefix is wired to the applySandbox failure path', () => {
    // The spec requires exactly this prefix for step 2-5 failures.
    expect(WORKER_SRC).toContain('"sandbox setup failed: "');

    // And it must be in the exec block, adjacent to the applySandbox call.
    const execBody = getExecBlockBody();
    expect(execBody).toContain('"sandbox setup failed: "');
    expect(execBody).toMatch(/\bapplySandbox\s*\(/);
  });

  // These are the "real" integration-style tests — they would need the
  // worker running against real Pyodide to observe the message the host
  // receives. Kept as `it.skip` so the path is ready for browser mode.

  it.skip(
    "(requires vitest browser mode) exec with bad import emits error prefixed 'package load failed: '",
    async () => {
      // Pseudo: createPyodideWorker(); init(); exec("import no_such_pkg_xyz", []);
      // expect(error.message).toMatch(/^package load failed: /);
    },
  );

  it.skip(
    "(requires vitest browser mode) sandbox setup failure emits error prefixed 'sandbox setup failed: '",
    async () => {
      // Hard to provoke naturally — spec §Sandbox Restrictions → Failure
      // mode says this branch is not expected in practice. Would require
      // monkey-patching applySandbox to throw, which is not possible from
      // the host side. Left as a placeholder for completeness.
    },
  );
});

// ---------------------------------------------------------------------------
// Item 6: the 30 s worker-internal timeout still exists after the refactor
// ---------------------------------------------------------------------------

describe("worker.ts — 30 s worker-internal timeout", () => {
  it("declares EXEC_TIMEOUT_MS = 30_000", () => {
    expect(WORKER_SRC).toMatch(/EXEC_TIMEOUT_MS\s*=\s*30_000/);
  });

  it("uses Promise.race against a timeout in the exec path", () => {
    const execBody = getExecBlockBody();
    // Implementation specifics: runPhase + timeoutPromise + Promise.race.
    expect(execBody).toContain("Promise.race");
    expect(execBody).toContain("timeoutPromise");
    expect(execBody).toContain("__PYODIDE_WORKER_TIMEOUT__");
  });

  it("terminates the worker (self.close) when the internal timeout fires", () => {
    const execBody = getExecBlockBody();
    // The timeout branch must call self.close() — otherwise a hanging
    // loadPackagesFromImports would leak.
    expect(execBody).toMatch(/__PYODIDE_WORKER_TIMEOUT__[\s\S]*self\.close\(\)/);
  });

  it("setTimeout in exec uses EXEC_TIMEOUT_MS (not a hard-coded literal drift)", () => {
    const execBody = getExecBlockBody();
    // The timer must be driven by the named constant, so changing the
    // spec'd budget is a one-line edit.
    expect(execBody).toMatch(/setTimeout\([^,]+,\s*EXEC_TIMEOUT_MS\s*\)/);
  });
});

// ---------------------------------------------------------------------------
// Sanity: the "not blocked, intentionally" list per spec
// ---------------------------------------------------------------------------

describe("worker.ts — intentionally unblocked surfaces (spec §Sandbox Restrictions)", () => {
  // Coarse substring scan would false-positive because worker.ts comments
  // legitimately reference `pathlib`/`ctypes` when explaining why they are
  // NOT blocked ("Pyodide's own loader, which uses pathlib / ctypes / open").
  // The strict checks live above: quoted-string literal absence and
  // builtins.open absence. Here we assert on the sandbox body specifically —
  // nothing inside applySandbox may reference the intentionally-unblocked
  // surfaces as targets of setattr / sys.modules / del.

  it("applySandbox body does not install stubs for pathlib / ctypes / builtins.open", () => {
    const sandboxBody = getApplySandboxBody();
    // Assert absence as sys.modules keys and as setattr/delattr targets.
    expect(sandboxBody).not.toMatch(/sys\.modules\[['"]pathlib['"]\]/);
    expect(sandboxBody).not.toMatch(/sys\.modules\[['"]ctypes['"]\]/);
    expect(sandboxBody).not.toMatch(/setattr\(\s*builtins/);
    expect(sandboxBody).not.toMatch(/delattr\(\s*builtins/);
    expect(sandboxBody).not.toMatch(/\bdel\s+builtins\./);
  });
});

// ---------------------------------------------------------------------------
// Follow-on regression: subprocess + os.* subprocess surface fully unblocked
//
// After the init-time-sandbox fix landed, a second import-chain bug was found:
// scipy (specifically scipy.stats via scipy.fft -> _pocketfft_umath) touches
// `subprocess.<attr>` at import time. The earlier iteration of applySandbox
// installed a `sys.modules['subprocess']` stub whose `__getattr__` raised
// RuntimeError, poisoning scipy import. Same class of bug applied to the
// `os.system / os.popen / os.fork*/exec*/spawn*/posix_spawn*` setattr loop:
// scientific packages enumerate os attributes at import and trip the stub.
//
// Spec + code were updated to drop both surfaces entirely. These assertions
// would have caught that regression at source level — vitest catches the
// bug without needing real Pyodide in the node environment. The end-to-end
// companion lives as it.skip in dryRun.integration.test.ts.
// ---------------------------------------------------------------------------

describe("worker.ts — subprocess / os.* subprocess surface is fully unblocked (regression)", () => {
  it("applySandbox body does not reference 'subprocess' anywhere", () => {
    // No sys.modules['subprocess'] stub, no _PROC_MSG constant, no
    // "subprocess execution is disabled" literal, no comment that would
    // suggest the stub is coming back. The intent is a hard absence so the
    // earlier iteration cannot silently return.
    const sandboxBody = getApplySandboxBody();
    expect(sandboxBody).not.toContain("subprocess");
  });

  it("applySandbox body does not contain a _PROC_MSG constant", () => {
    const sandboxBody = getApplySandboxBody();
    expect(sandboxBody).not.toContain("_PROC_MSG");
  });

  it("applySandbox body does not contain the 'subprocess execution is disabled' literal", () => {
    const sandboxBody = getApplySandboxBody();
    expect(sandboxBody).not.toContain("subprocess execution is disabled");
  });

  it("applySandbox body does not alias `os` (no `import os as _os`)", () => {
    // The earlier iteration imported os as `_os` to then enumerate and
    // setattr a subprocess-surface list onto it. With that surface dropped,
    // the alias itself should no longer exist.
    const sandboxBody = getApplySandboxBody();
    expect(sandboxBody).not.toMatch(/\bimport\s+os\s+as\s+_os\b/);
  });

  it("applySandbox body does not contain a setattr(_os, ...) loop", () => {
    // Residual `setattr(_os, attr, _make_stub(...))` would poison
    // os.system / os.popen / os.fork / os.exec / os.spawn / os.posix_spawn
    // / os.forkpty on import — the exact kind of thing scipy trips over.
    const sandboxBody = getApplySandboxBody();
    expect(sandboxBody).not.toMatch(/setattr\(\s*_os\b/);
  });
});

// ---------------------------------------------------------------------------
// Extend the "not blocked, intentionally" invariants to cover the new
// surfaces. Assertions are on applySandbox's body so comments elsewhere in
// worker.ts that legitimately explain "these are unblocked" do not false-
// positive. Paired with the broader `not.toContain("subprocess")` above,
// this pins the full list the code-reviewer approved.
// ---------------------------------------------------------------------------

describe("worker.ts — subprocess + os.* surfaces are NOT stubbed (extended invariant list)", () => {
  const UNBLOCKED_SURFACES = [
    "subprocess",
    "os.system",
    "os.popen",
    "os.fork",
    "os.exec",
    "os.spawn",
    "os.posix_spawn",
    "os.forkpty",
  ];

  for (const surface of UNBLOCKED_SURFACES) {
    it(`applySandbox body does not stub '${surface}'`, () => {
      const sandboxBody = getApplySandboxBody();
      // Plain substring absence — if any of these appear inside the sandbox
      // body (as a string literal target of setattr, a sys.modules key, or
      // a comment referring to a stub), that is the regression.
      expect(sandboxBody).not.toContain(surface);
    });
  }
});

// ---------------------------------------------------------------------------
// Intent-preservation sanity: the 11 network stdlib stubs and the 7 Python-
// side setattr targets the code-reviewer called out must still be present.
// Without this, "nothing should block anything" would pass vacuously if
// someone accidentally gutted applySandbox.
// ---------------------------------------------------------------------------

describe("worker.ts — network stubs + js./pyodide. setattr targets preserved (intent sanity)", () => {
  // Per applySandbox body: socket, urllib.request, urllib.error, http.client,
  // ftplib, smtplib, telnetlib, poplib, imaplib, nntplib, xmlrpc.client.
  const NET_STDLIB_STUBS = [
    "socket",
    "urllib.request",
    "urllib.error",
    "http.client",
    "ftplib",
    "smtplib",
    "telnetlib",
    "poplib",
    "imaplib",
    "nntplib",
    "xmlrpc.client",
  ];

  it("applySandbox body still lists all 11 network stdlib stubs", () => {
    const sandboxBody = getApplySandboxBody();
    for (const mod of NET_STDLIB_STUBS) {
      expect(sandboxBody).toMatch(
        new RegExp(`['"]${mod.replace(/\./g, "\\.")}['"]`),
      );
    }
  });

  it("applySandbox body exposes exactly the 11 known network stubs to sys.modules", () => {
    // Belt + braces: count the distinct quoted-literal module names that
    // live inside the _NET_MODULES list. If someone adds a new entry
    // without updating the test, this will tell us.
    const sandboxBody = getApplySandboxBody();
    const listMatch = sandboxBody.match(/_NET_MODULES\s*=\s*\[([\s\S]*?)\]/);
    expect(listMatch).not.toBeNull();
    const listBody = listMatch![1];
    const quoted = listBody.match(/['"][^'"]+['"]/g) ?? [];
    expect(quoted).toHaveLength(NET_STDLIB_STUBS.length);
  });

  // The 7 setattr targets per the docstring block above applySandbox:
  //   pyodide.http.pyfetch
  //   pyodide.code.run_js
  //   pyodide.ffi.run_js
  //   js.fetch
  //   js.XMLHttpRequest
  //   js.WebSocket
  //   js.navigator.sendBeacon
  const SETATTR_TARGETS: Array<{ owner: string; attr: string }> = [
    { owner: "_ph", attr: "pyfetch" },
    { owner: "_pc", attr: "run_js" },
    { owner: "_pf", attr: "run_js" },
    { owner: "_js", attr: "fetch" },
    { owner: "_js", attr: "XMLHttpRequest" },
    { owner: "_js", attr: "WebSocket" },
    { owner: "_js.navigator", attr: "sendBeacon" },
  ];

  for (const { owner, attr } of SETATTR_TARGETS) {
    it(`applySandbox body still setattrs '${attr}' on ${owner}`, () => {
      const sandboxBody = getApplySandboxBody();
      // Escape the dot in `_js.navigator` for the regex.
      const ownerRe = owner.replace(/\./g, "\\.");
      const re = new RegExp(
        `setattr\\(\\s*${ownerRe}\\s*,\\s*['"]${attr}['"]`,
      );
      expect(sandboxBody).toMatch(re);
    });
  }
});

// ---------------------------------------------------------------------------
// Regression: cleanupPythonNamespace refreshes `sys._initial_modules` AFTER
// the removal loops, so Pyodide-installed packages (numpy, scipy, openblas,
// …) loaded during an exec are rolled into the baseline for the next exec.
//
// Earlier iteration wiped those packages between execs, producing
//   UserWarning: The NumPy module was reloaded (imported a second time)
// on row 2+ and paying ~100-500 ms per subsequent exec to reload them.
// These assertions would have caught that at source level without Pyodide.
// ---------------------------------------------------------------------------

describe("worker.ts — cleanupPythonNamespace refreshes initial-modules snapshot (numpy-reload regression)", () => {
  it("cleanupPythonNamespace body contains an assignment to sys._initial_modules", () => {
    // At minimum the refreshed snapshot at the end must be an assignment
    // (not just a `hasattr` read). Without it, packages installed during
    // this exec would be wiped on the next call.
    const cleanupBody = getCleanupBody();
    expect(cleanupBody).toMatch(/sys\._initial_modules\s*=\s*set\(/);
  });

  it("refresh assignment appears AFTER the `for _k in _to_remove: del sys.modules[_k]` loop", () => {
    // If the refresh runs before the removal loop, the loop's deletions
    // are immediately re-snapshotted-as-absent and the next exec re-wipes
    // them — the fix becomes a no-op. Order is load-bearing.
    const cleanupBody = getCleanupBody();
    const removalIdx = cleanupBody.search(
      /for\s+_k\s+in\s+_to_remove\s*:\s*\n\s*del\s+sys\.modules\[_k\]/,
    );
    expect(removalIdx).toBeGreaterThan(-1);

    // Find the LAST `sys._initial_modules =` assignment — that's the
    // refresh, as opposed to the init-guarded seed at the top of the
    // function (which lives inside `if not hasattr(...)`).
    const assignRe = /sys\._initial_modules\s*=\s*set\(/g;
    let lastAssignIdx = -1;
    let m: RegExpExecArray | null;
    while ((m = assignRe.exec(cleanupBody)) !== null) {
      lastAssignIdx = m.index;
    }
    expect(lastAssignIdx).toBeGreaterThan(-1);
    expect(lastAssignIdx).toBeGreaterThan(removalIdx);
  });

  it("refresh assignment appears AFTER the __main__ namespace `delattr` loop", () => {
    // Ordering invariant (spec-neutral but correctness-critical): the
    // refresh must be the final statement of the cleanup, so the
    // next-call baseline reflects the fully-cleaned state. If refresh
    // ran before the __main__ delattr loop, __main__ user symbols
    // wouldn't actually be part of sys.modules anyway (they live on
    // __main__'s namespace), so it's spec-neutral — but a future reader
    // could reasonably expect the refresh to close the function, and
    // this ordering keeps that intuition intact.
    const cleanupBody = getCleanupBody();
    const delattrIdx = cleanupBody.search(
      /for\s+_k\s+in\s+_to_del\s*:[\s\S]*?delattr\(_main,\s*_k\)/,
    );
    expect(delattrIdx).toBeGreaterThan(-1);

    const assignRe = /sys\._initial_modules\s*=\s*set\(/g;
    let lastAssignIdx = -1;
    let m: RegExpExecArray | null;
    while ((m = assignRe.exec(cleanupBody)) !== null) {
      lastAssignIdx = m.index;
    }
    expect(lastAssignIdx).toBeGreaterThan(-1);
    expect(lastAssignIdx).toBeGreaterThan(delattrIdx);
  });
});
