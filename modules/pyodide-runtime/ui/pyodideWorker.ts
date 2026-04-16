/**
 * Host-side PyodideWorker class.
 *
 * Wraps a Web Worker running worker.ts and provides typed async methods
 * for init, exec, and terminate.
 */

export interface ExecResult {
  stdout: string;
  stderr: string;
  durationMs: number;
}

const EXEC_TIMEOUT_MS = 30_000;

export class PyodideWorker {
  private worker: Worker;
  private terminated: boolean = false;

  constructor(worker: Worker) {
    this.worker = worker;
  }

  /**
   * Sends the `init` message; resolves when `ready` is received; rejects on `error`.
   */
  init(): Promise<void> {
    if (this.terminated) {
      return Promise.reject(new Error("Worker is terminated"));
    }

    return new Promise<void>((resolve, reject) => {
      const handler = (event: MessageEvent) => {
        const msg = event.data;
        if (msg.type === "ready") {
          this.worker.removeEventListener("message", handler);
          resolve();
        } else if (msg.type === "error") {
          this.worker.removeEventListener("message", handler);
          this.terminated = true;
          reject(new Error(msg.message));
        }
      };

      this.worker.addEventListener("message", handler);
      this.worker.postMessage({ type: "init" });
    });
  }

  /**
   * Sends an `exec` message; resolves with ExecResult; rejects on `error` or timeout.
   * If the 30s timeout fires, the Worker is terminated and subsequent calls reject immediately.
   */
  exec(script: string, stdinData: string): Promise<ExecResult> {
    if (this.terminated) {
      return Promise.reject(new Error("Worker is terminated"));
    }

    return new Promise<ExecResult>((resolve, reject) => {
      let timeoutId: ReturnType<typeof setTimeout>;

      const handler = (event: MessageEvent) => {
        const msg = event.data;
        if (msg.type === "result") {
          clearTimeout(timeoutId);
          this.worker.removeEventListener("message", handler);
          resolve({
            stdout: msg.stdout,
            stderr: msg.stderr,
            durationMs: msg.durationMs,
          });
        } else if (msg.type === "error") {
          clearTimeout(timeoutId);
          this.worker.removeEventListener("message", handler);
          reject(new Error(msg.message));
        }
      };

      timeoutId = setTimeout(() => {
        this.worker.removeEventListener("message", handler);
        this.terminate();
        reject(new Error("Execution timed out (30s)"));
      }, EXEC_TIMEOUT_MS);

      this.worker.addEventListener("message", handler);
      this.worker.postMessage({ type: "exec", script, stdinData });
    });
  }

  /**
   * Forcibly terminates the underlying Web Worker.
   */
  terminate(): void {
    if (!this.terminated) {
      this.terminated = true;
      this.worker.terminate();
    }
  }
}

/**
 * Creates and returns a PyodideWorker wrapper.
 * Does not send `init` - caller must call `init()` on the returned handle.
 */
export function createPyodideWorker(): PyodideWorker {
  const worker = new Worker(new URL("./worker.ts", import.meta.url));
  return new PyodideWorker(worker);
}
