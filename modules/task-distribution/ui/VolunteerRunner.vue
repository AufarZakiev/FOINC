<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref } from "vue";
import { getTaskStats, pollNextTask, submitTask } from "./api";
import type { TaskDispatch, TaskStats } from "../../../integrations/ui/types";
import type { Toast } from "../../../integrations/ui/notifications";
import {
  createPyodideWorker,
  type PyodideWorker,
} from "../../pyodide-runtime/ui/pyodideWorker";

const emit = defineEmits<{
  notify: [payload: Toast];
}>();

const WORKER_ID_KEY = "foinc.worker_id";
const POLL_INTERVAL_MS = 2_000;
const STATS_INTERVAL_MS = 2_000;

const workerId = ref<string>("");
const currentTask = ref<TaskDispatch | null>(null);
const elapsedMs = ref<number>(0);
const stats = ref<TaskStats | null>(null);
const activeJobId = ref<string | null>(null);

let pollTimer: ReturnType<typeof setTimeout> | null = null;
let statsTimer: ReturnType<typeof setTimeout> | null = null;
let elapsedTimer: ReturnType<typeof setInterval> | null = null;
let stopped = false;
let mounted = true;

// Single long-lived Pyodide worker field. Lazy-created on the first
// TaskDispatch; reused across subsequent dispatches; recreated on the next
// dispatch after a dead-worker signal; terminated on unmount.
let runner: PyodideWorker | null = null;
// True while a Lazy-create init() is in flight for `runner`. Polling is
// gated on !execInFlight && !initInFlight so that we never dispatch a
// second task concurrently.
let initInFlight = false;
let execInFlight = false;

function notifyError(message: string) {
  emit("notify", { level: "error", message });
}

function ensureWorkerId(): string {
  const existing = localStorage.getItem(WORKER_ID_KEY);
  if (existing && existing.length > 0) return existing;
  const fresh = crypto.randomUUID();
  localStorage.setItem(WORKER_ID_KEY, fresh);
  return fresh;
}

function scheduleNextPoll(delay = POLL_INTERVAL_MS) {
  if (stopped) return;
  if (pollTimer !== null) clearTimeout(pollTimer);
  pollTimer = setTimeout(pollForTask, delay);
}

function scheduleNextStats(delay = STATS_INTERVAL_MS) {
  if (stopped) return;
  if (statsTimer !== null) clearTimeout(statsTimer);
  statsTimer = setTimeout(refreshStats, delay);
}

// Classify an exec rejection as either dead-worker (recreate on next
// dispatch) or user-script fault (default: reuse). Only the two explicit
// dead-worker signals from modules/pyodide-runtime trigger recreation:
//   - "Execution timed out (30s)"  (30s timeout path; worker self-closed)
//   - "Worker is terminated"       (any exec after Terminated state)
// Every other rejection is classified as a user-script fault per spec's
// "default is reuse" rule — the one-dispatch lag when a worker is in fact
// dead is accepted; the NEXT exec will reject with "Worker is terminated"
// and promote it to Crash recovery then.
function isDeadWorkerRejection(e: unknown): boolean {
  if (!(e instanceof Error)) return false;
  const msg = e.message;
  return (
    msg.includes("Execution timed out (30s)") ||
    msg.includes("Worker is terminated")
  );
}

function terminateRunnerQuietly() {
  if (runner === null) return;
  try {
    runner.terminate();
  } catch {
    // Best-effort: per spec, terminate() exceptions are swallowed.
  }
  runner = null;
}

async function pollForTask() {
  if (stopped) return;
  // Single-dispatch invariant: skip polling while an exec or a Lazy-create
  // init is already in flight.
  if (currentTask.value !== null || execInFlight || initInFlight) {
    scheduleNextPoll();
    return;
  }
  try {
    const dispatch = await pollNextTask(workerId.value);
    if (dispatch !== null) {
      await runTask(dispatch);
    }
  } catch (e: unknown) {
    const message = e instanceof Error ? e.message : "Polling failed";
    notifyError(message);
  } finally {
    scheduleNextPoll();
  }
}

async function runTask(dispatch: TaskDispatch) {
  currentTask.value = dispatch;
  activeJobId.value = dispatch.job_id;
  const startedAt = performance.now();
  elapsedMs.value = 0;
  elapsedTimer = setInterval(() => {
    elapsedMs.value = performance.now() - startedAt;
  }, 100);

  const clearPerTaskState = () => {
    if (elapsedTimer !== null) {
      clearInterval(elapsedTimer);
      elapsedTimer = null;
    }
    currentTask.value = null;
  };

  // Lazy create — pay the ~10s init cost once, on the first dispatch.
  if (runner === null) {
    runner = createPyodideWorker();
    initInFlight = true;
    try {
      await runner.init();
    } catch (e: unknown) {
      // Unmount during init: swallow, drop runner, do nothing else.
      if (!mounted) {
        initInFlight = false;
        clearPerTaskState();
        return;
      }
      // (a) best-effort terminate (exceptions swallowed).
      // (b) drop the runner so the next dispatch lazy-creates fresh.
      terminateRunnerQuietly();
      initInFlight = false;
      // (c) emit notify (spec: only init-failure and backend 5xx notify).
      const message = e instanceof Error ? e.message : "Pyodide init failed";
      notifyError(message);
      // Do NOT submit; backend reclaims via deadline.
      clearPerTaskState();
      refreshStats();
      return;
    }
    initInFlight = false;
    // Another unmount check: if we unmounted while init was awaiting, the
    // onBeforeUnmount hook already terminated and nulled runner. Bail.
    if (!mounted || runner === null) {
      clearPerTaskState();
      return;
    }
  }

  // Reuse — call exec directly on the existing idle worker.
  execInFlight = true;
  try {
    const argv = dispatch.input_rows[0].split(",");
    const result = await runner.exec(dispatch.script, argv);
    // Guard resolution handler against post-unmount delivery.
    if (!mounted) return;
    try {
      await submitTask(dispatch.task_id, {
        worker_id: workerId.value,
        stdout: result.stdout,
        stderr: result.stderr,
        duration_ms: result.durationMs,
      });
    } catch (e: unknown) {
      if (!mounted) return;
      const message = e instanceof Error ? e.message : "Submit failed";
      notifyError(message);
    }
  } catch (e: unknown) {
    // Guard rejection handler against post-unmount delivery per spec.
    if (!mounted) return;
    // Classify: dead-worker → Crash recovery; user-script fault → reuse.
    if (isDeadWorkerRejection(e)) {
      // Best-effort terminate, drop the runner. Do NOT notify. The next
      // TaskDispatch will lazy-create fresh. One recreate per dead-worker
      // event — no retry loop on the failed task.
      terminateRunnerQuietly();
    }
    // Either way, do NOT submit — backend reclaims via deadline.
  } finally {
    execInFlight = false;
    if (mounted) {
      clearPerTaskState();
      // Force an immediate stats refresh so the UI reflects the new count
      // without waiting a full poll interval.
      refreshStats();
    }
  }
}

async function refreshStats() {
  if (stopped) return;
  if (activeJobId.value === null) {
    scheduleNextStats();
    return;
  }
  try {
    stats.value = await getTaskStats(activeJobId.value, workerId.value);
  } catch (e: unknown) {
    const message = e instanceof Error ? e.message : "Stats fetch failed";
    notifyError(message);
  } finally {
    scheduleNextStats();
  }
}

onMounted(() => {
  workerId.value = ensureWorkerId();
  scheduleNextPoll(0);
  scheduleNextStats();
});

onBeforeUnmount(() => {
  // Flip `mounted` FIRST so any in-flight exec resolution bails without
  // calling submitTask, emitting notify, or mutating `runner`.
  mounted = false;
  stopped = true;
  if (pollTimer !== null) clearTimeout(pollTimer);
  if (statsTimer !== null) clearTimeout(statsTimer);
  if (elapsedTimer !== null) clearInterval(elapsedTimer);
  // Best-effort terminate of the long-lived Pyodide worker. Exceptions
  // from terminate() are swallowed per spec.
  terminateRunnerQuietly();
});
</script>

<template>
  <div class="volunteer-runner">
    <div class="header">
      <h3>Volunteer worker</h3>
      <span class="worker-id">ID: {{ workerId.slice(0, 8) }}</span>
    </div>

    <div class="status">
      <template v-if="currentTask !== null">
        <p class="status__line">
          <span class="dot dot--running"></span>
          Running task <code>{{ currentTask.task_id.slice(0, 8) }}</code>
          ({{ (elapsedMs / 1000).toFixed(1) }}s)
        </p>
      </template>
      <template v-else>
        <p class="status__line">
          <span class="dot dot--idle"></span>
          Idle — waiting for work
        </p>
      </template>
    </div>

    <div class="stats" v-if="stats !== null">
      <div class="stat">
        <span class="stat__label">Pending</span>
        <span class="stat__value">{{ stats.pending }}</span>
      </div>
      <div class="stat">
        <span class="stat__label">In flight</span>
        <span class="stat__value">{{ stats.in_flight }}</span>
      </div>
      <div class="stat">
        <span class="stat__label">Completed (total)</span>
        <span class="stat__value">{{ stats.completed_total }}</span>
      </div>
      <div class="stat">
        <span class="stat__label">Completed (you)</span>
        <span class="stat__value">{{ stats.completed_by_me }}</span>
      </div>
    </div>
  </div>
</template>

<style scoped>
.volunteer-runner {
  display: flex;
  flex-direction: column;
  gap: 1rem;
  padding: 1rem;
  border: 1px solid #e5e7eb;
  border-radius: 8px;
  background: #fafafa;
}

.header {
  display: flex;
  justify-content: space-between;
  align-items: baseline;
}

.header h3 {
  margin: 0;
  font-size: 1.05rem;
}

.worker-id {
  font-family: ui-monospace, monospace;
  font-size: 0.8rem;
  color: #666;
}

.status__line {
  margin: 0;
  display: flex;
  align-items: center;
  gap: 0.5rem;
  color: #333;
}

.dot {
  width: 10px;
  height: 10px;
  border-radius: 50%;
  display: inline-block;
}

.dot--idle {
  background: #9ca3af;
}

.dot--running {
  background: #10b981;
  animation: pulse 1s ease-in-out infinite;
}

@keyframes pulse {
  0%, 100% { opacity: 1; }
  50% { opacity: 0.4; }
}

code {
  background: #eef2ff;
  padding: 0 0.3rem;
  border-radius: 3px;
  font-family: ui-monospace, monospace;
  font-size: 0.85rem;
}

.stats {
  display: grid;
  grid-template-columns: repeat(4, 1fr);
  gap: 0.75rem;
}

.stat {
  display: flex;
  flex-direction: column;
  padding: 0.5rem 0.75rem;
  background: #fff;
  border: 1px solid #e5e7eb;
  border-radius: 6px;
}

.stat__label {
  font-size: 0.75rem;
  color: #666;
  text-transform: uppercase;
  letter-spacing: 0.03em;
}

.stat__value {
  font-size: 1.25rem;
  font-weight: 600;
  color: #111;
}
</style>
