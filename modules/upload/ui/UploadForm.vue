<script setup lang="ts">
import { ref } from "vue";
import { uploadFiles } from "./api";
import type { UploadCompleted } from "../../../integrations/ui/events";
import type { Toast } from "../../../integrations/ui/notifications";

const emit = defineEmits<{
  uploaded: [payload: UploadCompleted];
  notify: [payload: Toast];
}>();

const csvFile = ref<File | null>(null);
const scriptFile = ref<File | null>(null);
const loading = ref(false);
const dragOver = ref(false);

const csvInput = ref<HTMLInputElement | null>(null);
const scriptInput = ref<HTMLInputElement | null>(null);

function validateExtension(file: File, expected: string): boolean {
  return file.name.toLowerCase().endsWith(expected);
}

function notifyError(message: string) {
  emit("notify", { level: "error", message });
}

function handleCsvSelect(event: Event) {
  const input = event.target as HTMLInputElement;
  const file = input.files?.[0] ?? null;
  if (file && !validateExtension(file, ".csv")) {
    notifyError(`Invalid file: "${file.name}". Expected a .csv file.`);
    csvFile.value = null;
    input.value = "";
    return;
  }
  csvFile.value = file;
}

function handleScriptSelect(event: Event) {
  const input = event.target as HTMLInputElement;
  const file = input.files?.[0] ?? null;
  if (file && !validateExtension(file, ".py")) {
    notifyError(`Invalid file: "${file.name}". Expected a .py file.`);
    scriptFile.value = null;
    input.value = "";
    return;
  }
  scriptFile.value = file;
}

function handleDrop(event: DragEvent) {
  dragOver.value = false;
  const files = event.dataTransfer?.files;
  if (!files) return;

  for (let i = 0; i < files.length; i++) {
    const file = files[i];
    if (validateExtension(file, ".csv")) {
      csvFile.value = file;
    } else if (validateExtension(file, ".py")) {
      scriptFile.value = file;
    }
  }
}

function handleDragOver() {
  dragOver.value = true;
}

function handleDragLeave() {
  dragOver.value = false;
}

async function handleSubmit() {
  if (!csvFile.value || !scriptFile.value) return;

  loading.value = true;

  try {
    const uploadedJob = await uploadFiles(csvFile.value, scriptFile.value);
    const [script, csv] = await Promise.all([
      scriptFile.value.text(),
      csvFile.value.text(),
    ]);
    emit("uploaded", {
      jobId: uploadedJob.job_id,
      script,
      csv,
    });
  } catch (e: unknown) {
    const message = e instanceof Error ? e.message : "Upload failed";
    notifyError(message);
  } finally {
    loading.value = false;
  }
}

function reset() {
  csvFile.value = null;
  scriptFile.value = null;
  loading.value = false;
  if (csvInput.value) csvInput.value.value = "";
  if (scriptInput.value) scriptInput.value.value = "";
}
</script>

<template>
  <div class="upload-form">
    <div
      class="drop-zone"
      :class="{ 'drop-zone--active': dragOver }"
      @drop.prevent="handleDrop"
      @dragover.prevent="handleDragOver"
      @dragleave.prevent="handleDragLeave"
    >
      <p class="drop-zone__text">
        Drag &amp; drop your <strong>.csv</strong> and <strong>.py</strong> files
        here, or use the file pickers below.
      </p>
      <div class="drop-zone__files" v-if="csvFile || scriptFile">
        <span v-if="csvFile" class="file-badge">{{ csvFile.name }}</span>
        <span v-if="scriptFile" class="file-badge">{{ scriptFile.name }}</span>
      </div>
    </div>

    <div class="file-pickers">
      <label class="file-picker">
        <span class="file-picker__label">CSV file (.csv)</span>
        <input
          ref="csvInput"
          type="file"
          accept=".csv"
          @change="handleCsvSelect"
        />
        <span class="file-picker__name" v-if="csvFile">{{ csvFile.name }}</span>
      </label>

      <label class="file-picker">
        <span class="file-picker__label">Python script (.py)</span>
        <input
          ref="scriptInput"
          type="file"
          accept=".py"
          @change="handleScriptSelect"
        />
        <span class="file-picker__name" v-if="scriptFile">{{ scriptFile.name }}</span>
      </label>
    </div>

    <div class="actions">
      <button
        class="btn btn--primary"
        :disabled="!csvFile || !scriptFile || loading"
        @click="handleSubmit"
      >
        Upload
      </button>
      <button
        class="btn btn--secondary"
        @click="reset"
        :disabled="loading"
      >
        Reset
      </button>
      <div v-if="loading" class="loading-indicator" role="status" aria-live="polite">
        <span class="spinner" aria-hidden="true"></span>
        <span>Uploading...</span>
      </div>
    </div>
  </div>
</template>

<style scoped>
.upload-form {
  display: flex;
  flex-direction: column;
  gap: 1.25rem;
}

.drop-zone {
  border: 2px dashed #ccc;
  border-radius: 8px;
  padding: 2rem;
  text-align: center;
  transition: border-color 0.15s, background-color 0.15s;
  cursor: default;
}

.drop-zone--active {
  border-color: #3b82f6;
  background-color: #eff6ff;
}

.drop-zone__text {
  margin: 0 0 0.75rem;
  color: #555;
}

.drop-zone__files {
  display: flex;
  gap: 0.5rem;
  justify-content: center;
  flex-wrap: wrap;
}

.file-badge {
  display: inline-block;
  background: #e8f0fe;
  color: #1a56db;
  padding: 0.25rem 0.75rem;
  border-radius: 4px;
  font-size: 0.85rem;
}

.file-pickers {
  display: flex;
  gap: 1rem;
  flex-wrap: wrap;
}

.file-picker {
  flex: 1;
  min-width: 200px;
  display: flex;
  flex-direction: column;
  gap: 0.35rem;
}

.file-picker__label {
  font-weight: 600;
  font-size: 0.9rem;
  color: #333;
}

.file-picker__name {
  font-size: 0.85rem;
  color: #555;
}

.actions {
  display: flex;
  gap: 0.75rem;
  align-items: center;
}

.loading-indicator {
  display: inline-flex;
  align-items: center;
  gap: 0.5rem;
  color: #555;
  font-size: 0.9rem;
}

.spinner {
  display: inline-block;
  width: 16px;
  height: 16px;
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

.btn {
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

.btn--secondary {
  background: #e5e7eb;
  color: #374151;
}

.btn--secondary:not(:disabled):hover {
  background: #d1d5db;
}
</style>
