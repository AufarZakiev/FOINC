<script setup lang="ts">
import UploadForm from "../../modules/upload/ui/UploadForm.vue";
</script>

<template>
  <div class="app">
    <header class="app-header">
      <h1>FOINC</h1>
      <p>Distributed Volunteer Computing Platform</p>
    </header>
    <main class="app-main">
      <UploadForm />
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
}
</style>
