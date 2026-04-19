<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import UploadForm from "../../modules/upload/ui/UploadForm.vue";
import DryRunPanel from "../../modules/pyodide-runtime/ui/DryRunPanel.vue";
import StartJobButton from "../../modules/task-distribution/ui/StartJobButton.vue";
import DownloadResultButton from "../../modules/result-aggregation/ui/DownloadResultButton.vue";
import { deleteJob } from "../../modules/upload/ui/api";
import type {
  JobStarted,
  UploadCompleted,
} from "../../integrations/ui/events";
import type { Toast } from "../../integrations/ui/notifications";
import type { Job, JobStatus } from "../../integrations/ui/types";
import ToastContainer from "./ToastContainer.vue";
import VolunteerView from "./VolunteerView.vue";

type ToastWithId = Toast & { id: number };
type Route = "scientist" | "volunteer";

const JOB_POLL_INTERVAL_MS = 3000;

function routeFromHash(): Route {
  return window.location.hash === "#/volunteer" ? "volunteer" : "scientist";
}

const route = ref<Route>(routeFromHash());

function onHashChange() {
  route.value = routeFromHash();
}

onMounted(() => {
  window.addEventListener("hashchange", onHashChange);
});

onBeforeUnmount(() => {
  window.removeEventListener("hashchange", onHashChange);
  stopJobPolling();
});

const step = ref<1 | 2>(1);
const upload = ref<UploadCompleted | null>(null);
const startedJob = ref<JobStarted | null>(null);
const jobStatus = ref<JobStatus | null>(null);
const toasts = ref<ToastWithId[]>([]);

let nextToastId = 1;
let jobPollTimer: ReturnType<typeof setInterval> | null = null;

function pushToast(t: Toast) {
  toasts.value.push({ ...t, id: nextToastId++ });
}

function removeToast(id: number) {
  toasts.value = toasts.value.filter((t) => t.id !== id);
}

async function fetchJobStatus(jobId: string): Promise<JobStatus | null> {
  try {
    const res = await fetch(`/api/jobs/${jobId}`);
    if (!res.ok) {
      return null;
    }
    const job = (await res.json()) as Job;
    return job.status;
  } catch {
    return null;
  }
}

function stopJobPolling() {
  if (jobPollTimer !== null) {
    clearInterval(jobPollTimer);
    jobPollTimer = null;
  }
}

function startJobPolling(jobId: string) {
  stopJobPolling();
  // Fire immediately, then on an interval.
  void pollOnce(jobId);
  jobPollTimer = setInterval(() => {
    void pollOnce(jobId);
  }, JOB_POLL_INTERVAL_MS);
}

async function pollOnce(jobId: string) {
  // Bail if the job we're polling for is no longer the active one.
  if (startedJob.value === null || startedJob.value.jobId !== jobId) {
    return;
  }
  const status = await fetchJobStatus(jobId);
  if (status === null) {
    return;
  }
  // Still the active job?
  if (startedJob.value === null || startedJob.value.jobId !== jobId) {
    return;
  }
  jobStatus.value = status;
  if (status === "completed" || status === "failed") {
    stopJobPolling();
  }
}

watch(
  () => startedJob.value?.jobId ?? null,
  (jobId) => {
    stopJobPolling();
    jobStatus.value = null;
    if (jobId !== null) {
      startJobPolling(jobId);
    }
  },
);

function onUploaded(payload: UploadCompleted) {
  upload.value = payload;
  startedJob.value = null;
  step.value = 2;
}

function onJobStarted(payload: JobStarted) {
  startedJob.value = payload;
}

async function onBack() {
  // If a job has already been started, don't try to delete — it's in processing.
  // Just reset local wizard state.
  if (startedJob.value === null) {
    const current = upload.value;
    if (current !== null) {
      try {
        await deleteJob(current.jobId);
      } catch (err) {
        const message =
          err instanceof Error ? err.message : "Failed to delete job.";
        pushToast({ level: "error", message, durationMs: 0 });
      }
    }
  }
  upload.value = null;
  startedJob.value = null;
  step.value = 1;
}

const taskCountLabel = computed(() =>
  startedJob.value === null
    ? ""
    : startedJob.value.taskCount === 1
      ? "1 task queued"
      : `${startedJob.value.taskCount} tasks queued`,
);

const statusLabel = computed(() => {
  switch (jobStatus.value) {
    case "uploaded":
      return "Status: uploaded";
    case "processing":
      return "Status: processing";
    case "completed":
      return "Status: completed";
    case "failed":
      return "Status: failed";
    default:
      return "";
  }
});
</script>

<template>
  <VolunteerView v-if="route === 'volunteer'" />
  <div v-else class="app">
    <header class="app-header">
      <h1>FOINC</h1>
      <p>Distributed Volunteer Computing Platform</p>
      <a class="nav-link" href="#/volunteer">Volunteer &rarr;</a>
    </header>

    <nav class="stepper" aria-label="Wizard progress">
      <span
        class="pill"
        :class="{
          'pill--active': step === 1,
          'pill--checked': step === 2,
        }"
        :aria-current="step === 1 ? 'step' : undefined"
      >
        <span v-if="step === 2" class="pill__check" aria-hidden="true"
          >&#10003;</span
        >
        1 &middot; Upload
      </span>
      <span
        class="pill"
        :class="{
          'pill--active': step === 2,
          'pill--muted': step === 1,
        }"
        :aria-current="step === 2 ? 'step' : undefined"
      >
        2 &middot; Dry run
      </span>
    </nav>

    <main class="app-main">
      <UploadForm
        v-if="step === 1"
        @uploaded="onUploaded"
        @notify="pushToast"
      />
      <template v-else-if="step === 2 && upload">
        <template v-if="startedJob === null">
          <DryRunPanel :upload="upload" @notify="pushToast" />
          <StartJobButton
            :upload="upload"
            @started="onJobStarted"
            @notify="pushToast"
          />
        </template>
        <template v-else>
          <section class="job-confirmation" role="status">
            <h2 class="job-confirmation__title">Job started</h2>
            <p class="job-confirmation__body">
              {{ taskCountLabel }}.
              <a class="job-confirmation__link" href="#/volunteer"
                >Open the volunteer page</a
              >
              to help run them.
            </p>
            <p v-if="statusLabel" class="job-confirmation__status">
              {{ statusLabel }}
            </p>
          </section>
          <DownloadResultButton
            :jobId="startedJob.jobId"
            :jobStatus="jobStatus ?? 'processing'"
          />
        </template>
        <button type="button" class="back-button" @click="onBack">Back</button>
      </template>
    </main>

    <ToastContainer :toasts="toasts" @dismiss="removeToast" />
  </div>
</template>

<style scoped>
.app {
  max-width: 720px;
  margin: 0 auto;
  padding: 2rem 1rem;
  font-family: system-ui, -apple-system, sans-serif;
  color: #1a1a1a;
}

.app-header {
  text-align: center;
  margin-bottom: 1.5rem;
}

.app-header h1 {
  margin: 0 0 0.25rem;
  font-size: 1.75rem;
}

.app-header p {
  margin: 0 0 0.5rem;
  color: #666;
  font-size: 0.95rem;
}

.nav-link {
  display: inline-block;
  margin-top: 0.25rem;
  font-size: 0.875rem;
  color: #1565c0;
  text-decoration: none;
}

.nav-link:hover {
  text-decoration: underline;
}

.stepper {
  display: flex;
  justify-content: center;
  gap: 0.5rem;
  margin-bottom: 1.5rem;
  flex-wrap: wrap;
}

.pill {
  display: inline-flex;
  align-items: center;
  gap: 0.35rem;
  padding: 0.35rem 0.85rem;
  border-radius: 9999px;
  font-size: 0.875rem;
  font-weight: 500;
  background: #f0f0f0;
  color: #666;
  border: 1px solid #ddd;
  transition:
    background-color 0.15s ease,
    color 0.15s ease,
    border-color 0.15s ease;
}

.pill--active {
  background: #1565c0;
  color: #fff;
  border-color: #1565c0;
}

.pill--checked {
  background: #e8f5e9;
  color: #2e7d32;
  border-color: #a5d6a7;
}

.pill--muted {
  background: #f5f5f5;
  color: #999;
  border-color: #e5e5e5;
}

.pill__check {
  font-weight: bold;
}

.app-main {
  width: 100%;
  display: flex;
  flex-direction: column;
  gap: 1.5rem;
}

.job-confirmation {
  border: 1px solid #a5d6a7;
  background: #e8f5e9;
  color: #1b5e20;
  border-radius: 6px;
  padding: 1rem 1.25rem;
}

.job-confirmation__title {
  margin: 0 0 0.35rem;
  font-size: 1.05rem;
}

.job-confirmation__body {
  margin: 0;
  font-size: 0.95rem;
  line-height: 1.4;
}

.job-confirmation__status {
  margin: 0.5rem 0 0;
  font-size: 0.9rem;
  font-weight: 500;
  color: #1b5e20;
}

.job-confirmation__link {
  color: #1565c0;
  text-decoration: none;
}

.job-confirmation__link:hover {
  text-decoration: underline;
}

.back-button {
  align-self: flex-start;
  padding: 0.5rem 1rem;
  background: #fff;
  color: #1a1a1a;
  border: 1px solid #ccc;
  border-radius: 4px;
  cursor: pointer;
  font-size: 0.9rem;
}

.back-button:hover {
  background: #f5f5f5;
}
</style>
