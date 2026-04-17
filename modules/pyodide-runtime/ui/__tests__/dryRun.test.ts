import { describe, it, expect, vi, beforeEach } from "vitest";
import type { ExecResult } from "../pyodideWorker";

// ---------------------------------------------------------------------------
// Mock createPyodideWorker before importing dryRun
// ---------------------------------------------------------------------------

const mockInit = vi.fn<() => Promise<void>>();
const mockExec = vi.fn<(script: string, stdinData: string) => Promise<ExecResult>>();
const mockTerminate = vi.fn();

vi.mock("../pyodideWorker", () => ({
  createPyodideWorker: () => ({
    init: mockInit,
    exec: mockExec,
    terminate: mockTerminate,
  }),
}));

// Import after mock is set up
import { dryRun } from "../dryRun";

// ---------------------------------------------------------------------------
// dryRun tests
// ---------------------------------------------------------------------------

describe("dryRun()", () => {
  const script = "import sys; print(sys.stdin.readline(), end='')";
  const rows = ["Alice,30", "Bob,25", "Carol,40"];

  beforeEach(() => {
    mockInit.mockReset();
    mockExec.mockReset();
    mockTerminate.mockReset();

    // Default: init succeeds
    mockInit.mockResolvedValue(undefined);

    // Default: exec returns a valid result
    mockExec.mockImplementation(async (_script: string, stdinData: string) => ({
      stdout: stdinData,
      stderr: "",
      durationMs: 10,
    }));
  });

  // ---- Row-count validation ---------------------------------------------

  it("throws if csvRows.length is > 3 (too many)", async () => {
    await expect(
      dryRun(script, ["r1", "r2", "r3", "r4"]),
    ).rejects.toThrow("dryRun requires 1 to 3 CSV rows");
  });

  it("throws if csvRows is empty", async () => {
    await expect(dryRun(script, [])).rejects.toThrow(
      "dryRun requires 1 to 3 CSV rows",
    );
  });

  it("does not call createPyodideWorker when row count is wrong (empty)", async () => {
    await dryRun(script, []).catch(() => {});
    expect(mockInit).not.toHaveBeenCalled();
    expect(mockTerminate).not.toHaveBeenCalled();
  });

  it("does not call createPyodideWorker when row count is wrong (too many)", async () => {
    await dryRun(script, ["a", "b", "c", "d"]).catch(() => {});
    expect(mockInit).not.toHaveBeenCalled();
    expect(mockTerminate).not.toHaveBeenCalled();
  });

  // ---- Valid row counts: 1, 2, 3 ----------------------------------------

  it("succeeds with exactly 1 row and returns 1 result row", async () => {
    const result = await dryRun(script, ["Alice,30"]);

    expect(result.rows).toHaveLength(1);
    expect(result.rows[0].input).toBe("Alice,30");
    expect(mockExec).toHaveBeenCalledTimes(1);
  });

  it("succeeds with exactly 2 rows and returns 2 result rows", async () => {
    const result = await dryRun(script, ["Alice,30", "Bob,25"]);

    expect(result.rows).toHaveLength(2);
    expect(result.rows[0].input).toBe("Alice,30");
    expect(result.rows[1].input).toBe("Bob,25");
    expect(mockExec).toHaveBeenCalledTimes(2);
  });

  it("succeeds with exactly 3 rows and returns 3 result rows", async () => {
    const result = await dryRun(script, rows);

    expect(result.rows).toHaveLength(3);
    expect(mockExec).toHaveBeenCalledTimes(3);
  });

  // ---- Result shape ------------------------------------------------------

  it("returns DryRunResult with raw data rows as input (not header-prefixed)", async () => {
    const result = await dryRun(script, rows);

    expect(result.rows).toHaveLength(3);
    expect(result.rows[0].input).toBe("Alice,30");
    expect(result.rows[1].input).toBe("Bob,25");
    expect(result.rows[2].input).toBe("Carol,40");
    // Make sure input is NOT the concatenated header\nrow form.
    expect(result.rows[0].input).not.toContain("\n");
    expect(result.rows[0].input).not.toContain("name,age");
    expect(typeof result.totalDurationMs).toBe("number");
  });

  it("populates stdout, stderr, and durationMs from exec results", async () => {
    mockExec
      .mockResolvedValueOnce({ stdout: "out1", stderr: "err1", durationMs: 5 })
      .mockResolvedValueOnce({ stdout: "out2", stderr: "", durationMs: 15 })
      .mockResolvedValueOnce({ stdout: "out3", stderr: "warn", durationMs: 25 });

    const result = await dryRun(script, rows);

    expect(result.rows[0].stdout).toBe("out1");
    expect(result.rows[0].stderr).toBe("err1");
    expect(result.rows[0].durationMs).toBe(5);

    expect(result.rows[1].stdout).toBe("out2");
    expect(result.rows[1].stderr).toBe("");
    expect(result.rows[1].durationMs).toBe(15);

    expect(result.rows[2].stdout).toBe("out3");
    expect(result.rows[2].stderr).toBe("warn");
    expect(result.rows[2].durationMs).toBe(25);
  });

  it("totalDurationMs equals the sum of per-row durationMs", async () => {
    mockExec
      .mockResolvedValueOnce({ stdout: "", stderr: "", durationMs: 5 })
      .mockResolvedValueOnce({ stdout: "", stderr: "", durationMs: 15 })
      .mockResolvedValueOnce({ stdout: "", stderr: "", durationMs: 25 });

    const result = await dryRun(script, rows);

    expect(result.totalDurationMs).toBe(45);
    expect(result.totalDurationMs).toBe(
      result.rows.reduce((a, r) => a + r.durationMs, 0),
    );
  });

  it("totalDurationMs equals row durationMs for single-row run", async () => {
    mockExec.mockResolvedValueOnce({
      stdout: "",
      stderr: "",
      durationMs: 7.5,
    });

    const result = await dryRun(script, ["Alice,30"]);

    expect(result.totalDurationMs).toBe(7.5);
    expect(result.rows[0].durationMs).toBe(7.5);
  });

  it("calls init before any exec", async () => {
    const callOrder: string[] = [];
    mockInit.mockImplementation(async () => {
      callOrder.push("init");
    });
    mockExec.mockImplementation(async () => {
      callOrder.push("exec");
      return { stdout: "", stderr: "", durationMs: 0 };
    });

    await dryRun(script, rows);

    expect(callOrder[0]).toBe("init");
    expect(callOrder.filter((c) => c === "exec")).toHaveLength(3);
  });

  it("calls exec sequentially with one raw data row per call (no header)", async () => {
    await dryRun(script, rows);

    expect(mockExec).toHaveBeenCalledTimes(3);
    expect(mockExec).toHaveBeenNthCalledWith(1, script, "Alice,30");
    expect(mockExec).toHaveBeenNthCalledWith(2, script, "Bob,25");
    expect(mockExec).toHaveBeenNthCalledWith(3, script, "Carol,40");
  });

  // ---- Error handling -----------------------------------------------------

  it("stops on first row error and does not execute remaining rows", async () => {
    mockExec
      .mockResolvedValueOnce({ stdout: "ok", stderr: "", durationMs: 5 })
      .mockRejectedValueOnce(new Error("Script failed on row 2"));

    await expect(dryRun(script, rows)).rejects.toThrow(
      "Script failed on row 2",
    );

    // Only 2 exec calls should have been made (row 1 succeeded, row 2 failed, row 3 skipped)
    expect(mockExec).toHaveBeenCalledTimes(2);
  });

  it("stops on first row error (row 1 fails)", async () => {
    mockExec.mockRejectedValueOnce(new Error("Row 1 boom"));

    await expect(dryRun(script, rows)).rejects.toThrow("Row 1 boom");

    expect(mockExec).toHaveBeenCalledTimes(1);
  });

  it("terminates worker even on exec error (finally block)", async () => {
    mockExec.mockRejectedValue(new Error("exec failed"));

    await dryRun(script, rows).catch(() => {});

    expect(mockTerminate).toHaveBeenCalledOnce();
  });

  it("terminates worker on success", async () => {
    await dryRun(script, rows);

    expect(mockTerminate).toHaveBeenCalledOnce();
  });

  it("fails if worker init fails", async () => {
    mockInit.mockRejectedValue(new Error("Pyodide load failed"));

    await expect(dryRun(script, rows)).rejects.toThrow(
      "Pyodide load failed",
    );

    // No exec calls should have been made
    expect(mockExec).not.toHaveBeenCalled();
  });

  it("terminates worker even when init fails (finally block)", async () => {
    mockInit.mockRejectedValue(new Error("init boom"));

    await dryRun(script, rows).catch(() => {});

    expect(mockTerminate).toHaveBeenCalledOnce();
  });

  it("propagates timeout error, stops remaining rows, and terminates worker", async () => {
    mockExec
      .mockResolvedValueOnce({ stdout: "ok", stderr: "", durationMs: 5 })
      .mockRejectedValueOnce(new Error("Execution timed out (30s)"));

    await expect(dryRun(script, rows)).rejects.toThrow(
      "Execution timed out (30s)",
    );

    expect(mockExec).toHaveBeenCalledTimes(2);
    expect(mockTerminate).toHaveBeenCalledOnce();
  });
});
