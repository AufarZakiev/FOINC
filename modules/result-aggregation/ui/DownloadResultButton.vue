<script setup lang="ts">
import type { JobStatus } from "../../../integrations/ui/types";

/**
 * Download button for a completed job's assembled CSV.
 *
 * Renders a plain `<a href download>` pointing at `/api/jobs/{id}/result`
 * only when the job is `completed`. The browser handles the streaming
 * download; the backend attaches a filename via `Content-Disposition`, so
 * we don't need a click handler, a fetch call, or any error plumbing
 * here. If the anchor is visible the backend has already attested that
 * the job is `completed`; if a download fails afterwards (e.g. 500) the
 * browser surfaces that failure in its own UI.
 */
const props = defineProps<{
  jobId: string;
  jobStatus: JobStatus;
}>();
</script>

<template>
  <a
    v-if="props.jobStatus === 'completed'"
    class="btn btn--primary download-result"
    :href="`/api/jobs/${props.jobId}/result`"
    download
  >
    Download result
  </a>
</template>

<style scoped>
.btn {
  display: inline-flex;
  align-items: center;
  gap: 0.5rem;
  padding: 0.55rem 1.25rem;
  border: none;
  border-radius: 6px;
  font-size: 0.95rem;
  cursor: pointer;
  text-decoration: none;
  transition: background-color 0.15s;
}

.btn--primary {
  background: #3b82f6;
  color: #fff;
}

.btn--primary:hover {
  background: #2563eb;
}

.download-result {
  line-height: 1;
}
</style>
