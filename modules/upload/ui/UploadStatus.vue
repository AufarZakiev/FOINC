<script setup lang="ts">
import type { Job } from "../../../frontend/src/types/job";

defineProps<{
  loading: boolean;
  job: Job | null;
  error: string | null;
}>();

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

function formatDate(iso: string): string {
  return new Date(iso).toLocaleString();
}
</script>

<template>
  <div class="upload-status" v-if="loading || job || error">
    <!-- Loading -->
    <div v-if="loading" class="status-loading">
      <span class="spinner"></span>
      <span>Uploading files...</span>
    </div>

    <!-- Error -->
    <div v-else-if="error" class="status-error">
      <strong>Error:</strong> {{ error }}
    </div>

    <!-- Success -->
    <div v-else-if="job" class="status-success">
      <h3>Upload successful</h3>
      <table class="job-table">
        <tbody>
          <tr>
            <th>Job ID</th>
            <td>{{ job.job_id }}</td>
          </tr>
          <tr>
            <th>CSV file</th>
            <td>{{ job.csv_filename }} ({{ formatBytes(job.csv_size_bytes) }})</td>
          </tr>
          <tr>
            <th>Script file</th>
            <td>{{ job.script_filename }} ({{ formatBytes(job.script_size_bytes) }})</td>
          </tr>
          <tr>
            <th>Status</th>
            <td>{{ job.status }}</td>
          </tr>
          <tr>
            <th>Created at</th>
            <td>{{ formatDate(job.created_at) }}</td>
          </tr>
        </tbody>
      </table>
    </div>
  </div>
</template>

<style scoped>
.upload-status {
  margin-top: 0.5rem;
}

.status-loading {
  display: flex;
  align-items: center;
  gap: 0.5rem;
  color: #555;
  font-size: 0.95rem;
}

.spinner {
  display: inline-block;
  width: 18px;
  height: 18px;
  border: 2px solid #ccc;
  border-top-color: #3b82f6;
  border-radius: 50%;
  animation: spin 0.6s linear infinite;
}

@keyframes spin {
  to {
    transform: rotate(360deg);
  }
}

.status-error {
  background: #fef2f2;
  border: 1px solid #fecaca;
  color: #b91c1c;
  padding: 0.75rem 1rem;
  border-radius: 6px;
  font-size: 0.95rem;
}

.status-success h3 {
  margin: 0 0 0.75rem;
  font-size: 1rem;
  color: #166534;
}

.job-table {
  width: 100%;
  border-collapse: collapse;
  font-size: 0.9rem;
}

.job-table th,
.job-table td {
  text-align: left;
  padding: 0.4rem 0.75rem;
  border-bottom: 1px solid #e5e7eb;
}

.job-table th {
  width: 130px;
  color: #555;
  font-weight: 600;
}

.job-table td {
  color: #1a1a1a;
  word-break: break-all;
}
</style>
