/**
 * Integration-style regression tests for dryRun() against REAL Pyodide.
 *
 * These tests are all `it.skip` because they require Vitest browser mode
 * (network + WASM). They are NOT scaffolding — they are executable tests
 * that the team can unskip the moment browser mode is wired up.
 *
 * Why they matter:
 *   The original bug ("sandbox poisoned pathlib at init, breaking
 *   Pyodide's own _package_loader") was silent under any mocked-worker
 *   test suite. Only running a real script that imports numpy/scipy/pandas
 *   would surface it. These skipped tests document the exact end-to-end
 *   invariants we can't currently run but MUST hold.
 *
 * To enable:
 *   1. Configure vitest browser mode (e.g. add `test.browser` config to
 *      frontend/vite.config.ts and install @vitest/browser + playwright).
 *   2. Remove the `.skip` from each test below.
 *   3. Run: `cd frontend && npx vitest run --browser` (adjust to match
 *      the browser-mode script).
 */

import { describe, it, expect } from "vitest";
import { dryRun } from "../dryRun";

describe("dryRun() — real Pyodide integration (requires vitest browser mode)", () => {
  // Item 8 from the test plan: this is the single most important
  // regression for the init-time-sandbox bug. If `applySandbox` ever leaks
  // back to init-time, numpy will fail to load and this test will fail.
  it.skip(
    "runs `import numpy as np; print(np.array([1,2,3]).sum())` successfully (requires vitest browser mode — unskip when wired up)",
    async () => {
      const script =
        "import numpy as np; print(np.array([1,2,3]).sum())";
      // One data row is the minimum; the sum doesn't depend on argv here.
      const csvRows = ["dummy"];
      const result = await dryRun(script, csvRows);

      expect(result.rows).toHaveLength(1);
      expect(result.rows[0].stdout.trim()).toBe("6");
      expect(result.rows[0].stderr).toBe("");
      expect(result.totalDurationMs).toBeGreaterThan(0);
    },
    // 60s timeout for the whole test — Pyodide boot + numpy load on a cold
    // cache can take 20-40s. 30s is the per-exec spec budget; the total
    // test includes init.
    60_000,
  );

  it.skip(
    "runs a script that imports pandas without a 'pathlib' loader error (requires vitest browser mode)",
    async () => {
      // Direct regression probe: pandas pulls in a lot of stdlib including
      // pathlib. If pathlib is poisoned at init, this import fails inside
      // Pyodide's _package_loader with a cascade of AttributeError /
      // ImportError. The fix: pathlib stays unblocked.
      const script =
        "import pandas as pd; print(pd.DataFrame({'a':[1,2,3]})['a'].sum())";
      const result = await dryRun(script, ["dummy"]);
      expect(result.rows[0].stdout.trim()).toBe("6");
    },
    60_000,
  );

  it.skip(
    "network is blocked: `import urllib.request; urllib.request.urlopen(...)` raises RuntimeError (requires vitest browser mode)",
    async () => {
      // Positive sandbox test: once we can run for real, verify the
      // just-in-time sandbox actually bites.
      const script =
        "import urllib.request\n" +
        "try:\n" +
        "    urllib.request.urlopen('http://example.com')\n" +
        "    print('LEAK')\n" +
        "except RuntimeError as e:\n" +
        "    print(f'blocked: {e}')\n";
      const result = await dryRun(script, ["dummy"]);
      expect(result.rows[0].stdout).toContain("blocked: network access is disabled");
    },
    60_000,
  );

  // Follow-on regression: scipy.stats' import chain transitively touches
  // `subprocess.<attr>` (via scipy.fft -> _pocketfft_umath). An earlier
  // iteration of applySandbox installed a sys.modules['subprocess'] stub
  // whose __getattr__ raised RuntimeError, so `from scipy.stats import ...`
  // exploded at import with a "subprocess execution is disabled" error.
  // The fix drops subprocess (and the os.* subprocess surface) from the
  // blocklist entirely. When browser-mode vitest is wired up, unskip this
  // test — it would have caught the exact regression the source-level
  // assertions in worker.source.test.ts now guard against.
  it.skip(
    "(browser) scipy.stats import under sandbox does not raise subprocess error",
    async () => {
      const script =
        "import numpy\n" +
        "from scipy import fft\n" +
        "from scipy.stats import chisquare\n" +
        "print('ok')";
      // Single row; the script does not consume argv. Shape matches the
      // sibling skipped tests in this file (string rows, .stdout.trim()).
      const result = await dryRun(script, ["dummy"]);
      expect(result.rows[0].stdout.trim()).toBe("ok");
      expect(result.rows[0].stderr).toBe("");
    },
    60_000,
  );

  // Regression: cleanupPythonNamespace used to wipe Pyodide-installed
  // packages (numpy, scipy, openblas, …) between exec calls. Row 1 loaded
  // numpy fresh; row 2+ re-imported it on top of the wiped entry, which
  // Python detects and surfaces as
  //   UserWarning: The NumPy module was reloaded (imported a second time)
  // on stderr — plus ~100-500 ms wasted per exec re-loading the package.
  //
  // The fix refreshes sys._initial_modules at the end of cleanup so
  // packages installed during an exec persist into the next row's
  // baseline. Source-level ordering guards live in worker.source.test.ts;
  // this is the end-to-end probe for browser mode.
  it.skip(
    "(browser) numpy not reloaded between exec calls — no UserWarning on row 2+",
    async () => {
      const script =
        "import numpy\nimport warnings\nwith warnings.catch_warnings():\n    warnings.simplefilter('error')\n    import numpy\nprint('ok')";
      const csv = ["row1", "row2", "row3"];
      const result = await dryRun(script, csv);
      expect(result.rows.length).toBe(3);
      for (const row of result.rows) {
        expect(row.stdout.trim()).toBe("ok");
        expect(row.stderr).toBe(""); // no UserWarning on any row
      }
    },
    60_000,
  );
});
