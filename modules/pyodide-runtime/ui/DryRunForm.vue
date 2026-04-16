<script setup lang="ts">
import { ref, computed } from "vue";

type InputMode = "file" | "inline";

const emit = defineEmits<{
  submit: [payload: { script: string; header: string; csvRows: string[] }];
}>();

const scriptMode = ref<InputMode>("file");
const csvMode = ref<InputMode>("file");

const scriptFile = ref<File | null>(null);
const scriptText = ref<string>("");
const scriptFileError = ref<string | null>(null);

const csvFile = ref<File | null>(null);
const csvText = ref<string>("");
const csvFileError = ref<string | null>(null);

const submitError = ref<string | null>(null);

const scriptInput = ref<HTMLInputElement | null>(null);
const csvInput = ref<HTMLInputElement | null>(null);

function validateExtension(file: File, expected: string): boolean {
  return file.name.toLowerCase().endsWith(expected);
}

function readFileAsText(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result ?? ""));
    reader.onerror = () => reject(reader.error ?? new Error("Failed to read file"));
    reader.readAsText(file);
  });
}

function handleScriptFileSelect(event: Event) {
  const input = event.target as HTMLInputElement;
  const file = input.files?.[0] ?? null;
  if (file && !validateExtension(file, ".py")) {
    scriptFileError.value = `Invalid file: "${file.name}". Expected a .py file.`;
    scriptFile.value = null;
    input.value = "";
    return;
  }
  scriptFileError.value = null;
  scriptFile.value = file;
}

function handleCsvFileSelect(event: Event) {
  const input = event.target as HTMLInputElement;
  const file = input.files?.[0] ?? null;
  if (file && !validateExtension(file, ".csv")) {
    csvFileError.value = `Invalid file: "${file.name}". Expected a .csv file.`;
    csvFile.value = null;
    input.value = "";
    return;
  }
  csvFileError.value = null;
  csvFile.value = file;
}

function parseCsv(text: string): { header: string; rows: string[] } | null {
  const lines = text
    .split("\n")
    .map((l) => l.trim())
    .filter((l) => l.length > 0);
  if (lines.length === 0) return null;
  const [header, ...rows] = lines;
  return { header, rows };
}

const canSubmit = computed(() => {
  const hasScript =
    scriptMode.value === "file"
      ? scriptFile.value !== null && !scriptFileError.value
      : scriptText.value.trim().length > 0;
  const hasCsv =
    csvMode.value === "file"
      ? csvFile.value !== null && !csvFileError.value
      : csvText.value.trim().length > 0;
  return hasScript && hasCsv;
});

async function handleSubmit() {
  submitError.value = null;

  // Read script source
  let scriptSource = "";
  if (scriptMode.value === "file") {
    if (!scriptFile.value) {
      submitError.value = "Script file is required.";
      return;
    }
    try {
      scriptSource = await readFileAsText(scriptFile.value);
    } catch (e: unknown) {
      submitError.value =
        e instanceof Error ? e.message : "Failed to read script file";
      return;
    }
  } else {
    scriptSource = scriptText.value;
  }

  // Read CSV source
  let csvSource = "";
  if (csvMode.value === "file") {
    if (!csvFile.value) {
      submitError.value = "CSV file is required.";
      return;
    }
    try {
      csvSource = await readFileAsText(csvFile.value);
    } catch (e: unknown) {
      submitError.value =
        e instanceof Error ? e.message : "Failed to read CSV file";
      return;
    }
  } else {
    csvSource = csvText.value;
  }

  const parsed = parseCsv(csvSource);
  if (!parsed || parsed.rows.length === 0) {
    submitError.value = "CSV must contain at least 1 data row";
    return;
  }
  if (parsed.rows.length > 3) {
    submitError.value = "dryRun requires 1 to 3 CSV rows";
    return;
  }

  emit("submit", {
    script: scriptSource,
    header: parsed.header,
    csvRows: parsed.rows,
  });
}
</script>

<template>
  <div class="dry-run-form">
    <div class="field">
      <div class="field__header">
        <span class="field__label">Python script</span>
        <div class="toggle">
          <button
            type="button"
            class="toggle__btn"
            :class="{ 'toggle__btn--active': scriptMode === 'file' }"
            @click="scriptMode = 'file'"
          >
            File
          </button>
          <button
            type="button"
            class="toggle__btn"
            :class="{ 'toggle__btn--active': scriptMode === 'inline' }"
            @click="scriptMode = 'inline'"
          >
            Inline
          </button>
        </div>
      </div>

      <div v-if="scriptMode === 'file'" class="field__body">
        <input
          ref="scriptInput"
          type="file"
          accept=".py"
          @change="handleScriptFileSelect"
        />
        <span v-if="scriptFile" class="field__filename">{{ scriptFile.name }}</span>
        <p v-if="scriptFileError" class="field__error">{{ scriptFileError }}</p>
      </div>
      <div v-else class="field__body">
        <textarea
          v-model="scriptText"
          class="textarea"
          placeholder="# Your Python script here"
          rows="8"
        />
      </div>
    </div>

    <div class="field">
      <div class="field__header">
        <span class="field__label">CSV input (header + 1-3 data rows)</span>
        <div class="toggle">
          <button
            type="button"
            class="toggle__btn"
            :class="{ 'toggle__btn--active': csvMode === 'file' }"
            @click="csvMode = 'file'"
          >
            File
          </button>
          <button
            type="button"
            class="toggle__btn"
            :class="{ 'toggle__btn--active': csvMode === 'inline' }"
            @click="csvMode = 'inline'"
          >
            Inline
          </button>
        </div>
      </div>

      <div v-if="csvMode === 'file'" class="field__body">
        <input
          ref="csvInput"
          type="file"
          accept=".csv"
          @change="handleCsvFileSelect"
        />
        <span v-if="csvFile" class="field__filename">{{ csvFile.name }}</span>
        <p v-if="csvFileError" class="field__error">{{ csvFileError }}</p>
      </div>
      <div v-else class="field__body">
        <textarea
          v-model="csvText"
          class="textarea"
          placeholder="header1,header2&#10;value1,value2"
          rows="6"
        />
      </div>
    </div>

    <div class="actions">
      <button
        type="button"
        class="btn btn--primary"
        :disabled="!canSubmit"
        @click="handleSubmit"
      >
        Run dry run
      </button>
    </div>

    <p v-if="submitError" class="form-error">{{ submitError }}</p>
  </div>
</template>

<style scoped>
.dry-run-form {
  display: flex;
  flex-direction: column;
  gap: 1.25rem;
}

.field {
  display: flex;
  flex-direction: column;
  gap: 0.5rem;
}

.field__header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  gap: 0.75rem;
}

.field__label {
  font-weight: 600;
  font-size: 0.9rem;
  color: #333;
}

.field__body {
  display: flex;
  flex-direction: column;
  gap: 0.35rem;
}

.field__filename {
  font-size: 0.85rem;
  color: #555;
}

.field__error {
  margin: 0;
  color: #b91c1c;
  font-size: 0.85rem;
}

.toggle {
  display: inline-flex;
  border: 1px solid #d1d5db;
  border-radius: 6px;
  overflow: hidden;
}

.toggle__btn {
  border: none;
  background: #fff;
  padding: 0.3rem 0.75rem;
  font-size: 0.85rem;
  cursor: pointer;
  color: #374151;
}

.toggle__btn:not(:last-child) {
  border-right: 1px solid #d1d5db;
}

.toggle__btn--active {
  background: #3b82f6;
  color: #fff;
}

.textarea {
  width: 100%;
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  font-size: 0.85rem;
  padding: 0.5rem;
  border: 1px solid #d1d5db;
  border-radius: 6px;
  resize: vertical;
  box-sizing: border-box;
}

.actions {
  display: flex;
  gap: 0.75rem;
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

.form-error {
  margin: 0;
  color: #b91c1c;
  font-size: 0.9rem;
}
</style>
