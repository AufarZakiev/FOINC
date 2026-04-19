<script setup lang="ts">
import { ref } from "vue";
import { startJob } from "./api";
import type { JobStarted, UploadCompleted } from "../../../integrations/ui/events";
import type { Toast } from "../../../integrations/ui/notifications";

const props = defineProps<{
  upload: UploadCompleted;
}>();

const emit = defineEmits<{
  started: [payload: JobStarted];
  notify: [payload: Toast];
}>();

const loading = ref(false);

async function handleClick() {
  if (loading.value) return;
  loading.value = true;
  try {
    const response = await startJob(props.upload.jobId);
    emit("started", {
      jobId: response.job_id,
      taskCount: response.task_count,
    });
  } catch (e: unknown) {
    const message = e instanceof Error ? e.message : "Failed to start job";
    emit("notify", { level: "error", message });
  } finally {
    loading.value = false;
  }
}
</script>

<template>
  <div class="start-job">
    <button
      class="btn btn--primary"
      :disabled="loading"
      @click="handleClick"
    >
      <span v-if="loading" class="spinner" aria-hidden="true"></span>
      <span>{{ loading ? "Starting..." : "Process all" }}</span>
    </button>
  </div>
</template>

<style scoped>
.start-job {
  display: inline-flex;
  align-items: center;
  gap: 0.5rem;
}

.btn {
  display: inline-flex;
  align-items: center;
  gap: 0.5rem;
  padding: 0.55rem 1.25rem;
  border: none;
  border-radius: 6px;
  font-size: 0.95rem;
  cursor: pointer;
  transition: background-color 0.15s;
}

.btn:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

.btn--primary {
  background: #3b82f6;
  color: #fff;
}

.btn--primary:not(:disabled):hover {
  background: #2563eb;
}

.spinner {
  display: inline-block;
  width: 14px;
  height: 14px;
  border: 2px solid rgba(255, 255, 255, 0.4);
  border-top-color: #fff;
  border-radius: 50%;
  animation: spin 0.6s linear infinite;
}

@keyframes spin {
  to {
    transform: rotate(360deg);
  }
}
</style>
