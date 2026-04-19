import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { PyodideWorker, createPyodideWorker } from "../pyodideWorker";

/**
 * Creates a mock Web Worker that stores listeners and allows
 * manual dispatch of messages back to the host.
 */
function createMockWorker() {
  const listeners: Map<string, Set<Function>> = new Map();

  const worker = {
    addEventListener: vi.fn((type: string, handler: Function) => {
      if (!listeners.has(type)) listeners.set(type, new Set());
      listeners.get(type)!.add(handler);
    }),
    removeEventListener: vi.fn((type: string, handler: Function) => {
      listeners.get(type)?.delete(handler);
    }),
    postMessage: vi.fn(),
    terminate: vi.fn(),
  } as unknown as Worker;

  /** Simulate the worker posting a message back to the host. */
  function postFromWorker(data: Record<string, unknown>) {
    const handlers = listeners.get("message");
    if (handlers) {
      for (const handler of handlers) {
        handler({ data } as MessageEvent);
      }
    }
  }

  return { worker, postFromWorker };
}

// ---------------------------------------------------------------------------
// PyodideWorker tests
// ---------------------------------------------------------------------------

describe("PyodideWorker", () => {
  let mock: ReturnType<typeof createMockWorker>;
  let pw: PyodideWorker;

  beforeEach(() => {
    mock = createMockWorker();
    pw = new PyodideWorker(mock.worker);
  });

  // ---- init ---------------------------------------------------------------

  describe("init()", () => {
    it("resolves when worker posts ready", async () => {
      const initPromise = pw.init();

      // Worker should have received the init message
      expect(mock.worker.postMessage).toHaveBeenCalledWith({ type: "init" });

      // Simulate worker responding with ready
      mock.postFromWorker({ type: "ready" });

      await expect(initPromise).resolves.toBeUndefined();
    });

    it("rejects when worker posts error", async () => {
      const initPromise = pw.init();

      mock.postFromWorker({ type: "error", message: "Pyodide load failed" });

      await expect(initPromise).rejects.toThrow("Pyodide load failed");
    });

    it("rejects immediately if worker is terminated", async () => {
      pw.terminate();

      await expect(pw.init()).rejects.toThrow("Worker is terminated");
    });

    it("removes the message listener after ready", async () => {
      const initPromise = pw.init();
      mock.postFromWorker({ type: "ready" });
      await initPromise;

      expect(mock.worker.removeEventListener).toHaveBeenCalledWith(
        "message",
        expect.any(Function),
      );
    });

    it("removes the message listener after error", async () => {
      const initPromise = pw.init();
      mock.postFromWorker({ type: "error", message: "fail" });
      await initPromise.catch(() => {});

      expect(mock.worker.removeEventListener).toHaveBeenCalledWith(
        "message",
        expect.any(Function),
      );
    });

    it("rejects subsequent exec calls after init failure (Error state is terminal)", async () => {
      const initPromise = pw.init();
      mock.postFromWorker({ type: "error", message: "load failed" });
      await initPromise.catch(() => {});

      await expect(pw.exec("code", ["data"])).rejects.toThrow(
        "Worker is terminated",
      );
    });
  });

  // ---- exec ---------------------------------------------------------------

  describe("exec()", () => {
    it("resolves with ExecResult when worker posts result", async () => {
      const execPromise = pw.exec("print('hi')", ["1", "3"]);

      expect(mock.worker.postMessage).toHaveBeenCalledWith({
        type: "exec",
        script: "print('hi')",
        argv: ["1", "3"],
      });

      mock.postFromWorker({
        type: "result",
        stdout: "hi\n",
        stderr: "",
        durationMs: 12.5,
      });

      const result = await execPromise;
      expect(result.stdout).toBe("hi\n");
      expect(result.stderr).toBe("");
      expect(result.durationMs).toBe(12.5);
    });

    it("posts argv as an array in the exec message", async () => {
      const execPromise = pw.exec("print('x')", ["1", "3"]);

      // The second argument to exec must be forwarded as `argv` (array), not `stdinData`.
      expect(mock.worker.postMessage).toHaveBeenCalledWith({
        type: "exec",
        script: "print('x')",
        argv: ["1", "3"],
      });
      // And the value must be the exact array reference shape.
      const posted = (mock.worker.postMessage as ReturnType<typeof vi.fn>).mock
        .calls[0][0];
      expect(Array.isArray(posted.argv)).toBe(true);
      expect(posted.argv).toEqual(["1", "3"]);
      expect("stdinData" in posted).toBe(false);

      // Clean up the pending promise.
      mock.postFromWorker({
        type: "result",
        stdout: "",
        stderr: "",
        durationMs: 0,
      });
      await execPromise;
    });

    it("rejects when worker posts error", async () => {
      const execPromise = pw.exec("bad code", ["1", "3"]);

      mock.postFromWorker({
        type: "error",
        message: "SyntaxError: invalid syntax",
      });

      await expect(execPromise).rejects.toThrow("SyntaxError: invalid syntax");
    });

    it("rejects with stdout limit error when worker reports it", async () => {
      const execPromise = pw.exec("print('x' * 10**8)", ["1", "3"]);

      mock.postFromWorker({
        type: "error",
        message: "stdout limit exceeded (10 MB)",
      });

      await expect(execPromise).rejects.toThrow(
        "stdout limit exceeded (10 MB)",
      );
    });

    it("rejects immediately if worker is terminated", async () => {
      pw.terminate();

      await expect(pw.exec("print('hi')", ["1", "3"])).rejects.toThrow(
        "Worker is terminated",
      );
    });

    it("removes the message listener after result", async () => {
      const execPromise = pw.exec("code", ["data"]);
      mock.postFromWorker({
        type: "result",
        stdout: "",
        stderr: "",
        durationMs: 0,
      });
      await execPromise;

      expect(mock.worker.removeEventListener).toHaveBeenCalledWith(
        "message",
        expect.any(Function),
      );
    });

    it("removes the message listener after error", async () => {
      const execPromise = pw.exec("code", ["data"]);
      mock.postFromWorker({ type: "error", message: "fail" });
      await execPromise.catch(() => {});

      expect(mock.worker.removeEventListener).toHaveBeenCalledWith(
        "message",
        expect.any(Function),
      );
    });
  });

  // ---- exec timeout -------------------------------------------------------

  describe("exec() timeout", () => {
    beforeEach(() => {
      vi.useFakeTimers();
    });

    afterEach(() => {
      vi.useRealTimers();
    });

    it("terminates the worker and rejects after 30s timeout", async () => {
      const execPromise = pw.exec("import time; time.sleep(60)", ["1", "3"]);

      // Advance time to just past the 30s timeout
      vi.advanceTimersByTime(30_000);

      await expect(execPromise).rejects.toThrow("Execution timed out (30s)");

      // Worker should have been terminated
      expect(mock.worker.terminate).toHaveBeenCalled();
    });

    it("does not trigger timeout if result arrives before 30s", async () => {
      const execPromise = pw.exec("print('fast')", ["1", "3"]);

      // Advance 10 seconds, then respond
      vi.advanceTimersByTime(10_000);

      mock.postFromWorker({
        type: "result",
        stdout: "fast\n",
        stderr: "",
        durationMs: 100,
      });

      const result = await execPromise;
      expect(result.stdout).toBe("fast\n");

      // Worker should NOT have been terminated
      expect(mock.worker.terminate).not.toHaveBeenCalled();
    });

    it("subsequent exec calls reject after timeout termination", async () => {
      const execPromise = pw.exec("slow", ["1", "3"]);
      vi.advanceTimersByTime(30_000);
      await execPromise.catch(() => {});

      // After timeout, the worker is terminated; further calls should reject
      await expect(pw.exec("print('hi')", ["1", "3"])).rejects.toThrow(
        "Worker is terminated",
      );
    });
  });

  // ---- terminate ----------------------------------------------------------

  describe("terminate()", () => {
    it("calls worker.terminate()", () => {
      pw.terminate();
      expect(mock.worker.terminate).toHaveBeenCalledOnce();
    });

    it("is idempotent - calling twice does not throw or double-terminate", () => {
      pw.terminate();
      pw.terminate();

      // Should only have called the underlying terminate once
      expect(mock.worker.terminate).toHaveBeenCalledOnce();
    });
  });
});

// ---------------------------------------------------------------------------
// createPyodideWorker factory
// ---------------------------------------------------------------------------

describe("createPyodideWorker()", () => {
  it("returns a PyodideWorker instance", () => {
    // Mock the Worker constructor since we're not in a browser
    const originalWorker = globalThis.Worker;
    globalThis.Worker = vi.fn(function (this: Record<string, unknown>) {
      this.addEventListener = vi.fn();
      this.removeEventListener = vi.fn();
      this.postMessage = vi.fn();
      this.terminate = vi.fn();
    }) as unknown as typeof Worker;

    try {
      const pw = createPyodideWorker();
      expect(pw).toBeInstanceOf(PyodideWorker);
    } finally {
      globalThis.Worker = originalWorker;
    }
  });
});
