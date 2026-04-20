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
