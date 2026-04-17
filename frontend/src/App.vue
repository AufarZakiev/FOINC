<script setup lang="ts">
import { ref } from "vue";
import UploadForm from "../../modules/upload/ui/UploadForm.vue";
import DryRunPanel from "../../modules/pyodide-runtime/ui/DryRunPanel.vue";
import type { UploadCompleted } from "../../integrations/ui/events";

const upload = ref<UploadCompleted | null>(null);

function onUploaded(payload: UploadCompleted) {
  upload.value = payload;
}
</script>

<template>
  <div class="app">
    <header class="app-header">
      <h1>FOINC</h1>
      <p>Distributed Volunteer Computing Platform</p>
    </header>
    <main class="app-main">
      <UploadForm @uploaded="onUploaded" />
      <DryRunPanel v-if="upload" :upload="upload" />
    </main>
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
  margin-bottom: 2rem;
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

.app-main {
  width: 100%;
  display: flex;
  flex-direction: column;
  gap: 2rem;
}
</style>
