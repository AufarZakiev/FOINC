/**
 * Cross-module notification contract.
 *
 * Modules emit `notify` events carrying a `Toast` payload. The frontend
 * shell listens and renders them in a `ToastContainer`. A module MUST NOT
 * render its own toast UI; the shell owns presentation so messages from
 * different modules look the same and share the same dismiss queue.
 */

/** Severity level for a toast notification. */
export type ToastLevel = "success" | "error" | "info";

/**
 * A single user-facing notification emitted by a module.
 *
 * The frontend shell is free to display multiple toasts simultaneously and
 * to auto-dismiss them after `durationMs` (defaulting to 5000 when omitted).
 * Errors typically should use `durationMs: 0` (no auto-dismiss) so the user
 * can read them — but that's a shell-level policy, not a module concern.
 */
export interface Toast {
  /** Severity. Affects icon/color in the shell. */
  level: ToastLevel;
  /** Human-readable message shown to the user. No HTML. */
  message: string;
  /** Auto-dismiss delay in ms. Omit for shell default (5000). Use 0 to disable auto-dismiss. */
  durationMs?: number;
}
