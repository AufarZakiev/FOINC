<script setup lang="ts">
import { ref } from "vue";
import UploadForm from "../../modules/upload/ui/UploadForm.vue";
import DryRunPanel from "../../modules/pyodide-runtime/ui/DryRunPanel.vue";
import { deleteJob } from "../../modules/upload/ui/api";
import type { UploadCompleted } from "../../integrations/ui/events";
import type { Toast } from "../../integrations/ui/notifications";
import ToastContainer from "./ToastContainer.vue";

type ToastWithId = Toast & { id: number };

const step = ref<1 | 2>(1);
const upload = ref<UploadCompleted | null>(null);
const toasts = ref<ToastWithId[]>([]);

let nextToastId = 1;

function pushToast(t: Toast) {
  toasts.value.push({ ...t, id: nextToastId++ });
}

function removeToast(id: number) {
  toasts.value = toasts.value.filter((t) => t.id !== id);
}

function onUploaded(payload: UploadCompleted) {
  upload.value = payload;
  step.value = 2;
}

async function onBack() {
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
  upload.value = null;
  step.value = 1;
}
</script>

<template>
  <div class="app">
    <header class="app-header">
      <h1>FOINC</h1>
      <p>Distributed Volunteer Computing Platform</p>
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
        <span v-if="step === 2" class="pill__check" aria-hidden="true">&#10003;</span>
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
      <UploadForm v-if="step === 1" @uploaded="onUploaded" @notify="pushToast" />
      <template v-else-if="step === 2 && upload">
        <DryRunPanel :upload="upload" @notify="pushToast" />
        <button type="button" class="back-button" @click="onBack">
          Back
        </button>
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
  margin: 0;
  color: #666;
  font-size: 0.95rem;
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
