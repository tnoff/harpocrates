<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { toast } from "$lib/stores/toast.svelte";

  interface VerifyResult { backup_entry_id: number; filename: string; status: string; detail: string | null; }
  interface VerifyProgress { op_id: string; processed: number; total: number; current_file: string; passed: number; failed: number; errors: number; }
  interface VerifyComplete { op_id: string; passed: number; failed: number; errors: number; results: VerifyResult[]; }

  interface Props {
    selectedIds: number[];
    onclose: () => void;
  }

  let { selectedIds, onclose }: Props = $props();

  let queued = $state(false);
  let running = $state(false);
  let summary = $state<VerifyComplete | null>(null);
  let progress = $state<VerifyProgress | null>(null);
  let errorMsg = $state("");

  $effect(() => {
    let cleanups: (() => void)[] = [];

    (async () => {
      let opId: string;
      try {
        opId = await invoke<string>("verify_integrity", { backupEntryIds: selectedIds });
        queued = true;
      } catch (e) {
        toast.error(String(e));
        return;
      }

      const ul1 = await listen<VerifyProgress>("verify:progress", (event) => {
        if (event.payload.op_id !== opId) return;
        if (!running) { queued = false; running = true; }
        progress = event.payload;
      });

      const ul2 = await listen<VerifyComplete>("verify:complete", (event) => {
        if (event.payload.op_id !== opId) return;
        queued = false;
        running = false;
        summary = event.payload;
        cleanups.forEach((f) => f());
        cleanups = [];
      });

      const ul3 = await listen<{ id: string; error: string }>("op:failed", (event) => {
        if (event.payload.id !== opId) return;
        queued = false;
        running = false;
        errorMsg = event.payload.error;
        cleanups.forEach((f) => f());
        cleanups = [];
      });

      cleanups = [ul1, ul2, ul3];
    })();

    return () => {
      cleanups.forEach((f) => f());
    };
  });

  function statusColor(status: string): string {
    if (status === "passed" || status === "ok") return "status-passed";
    if (status === "failed") return "status-failed";
    return "status-error";
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="overlay" onclick={onclose} onkeydown={(e) => e.key === 'Escape' && onclose()} role="presentation">
  <div
    role="dialog"
    aria-modal="true"
    aria-labelledby="verify-title"
    tabindex="-1"
    class="dialog"
    onclick={(e) => e.stopPropagation()}
    onkeydown={(e) => e.stopPropagation()}
  >
    <h3 id="verify-title" class="dialog-title">Verify Integrity</h3>

    {#if errorMsg}
      <p class="error-text">{errorMsg}</p>

    {:else if queued && !running}
      <div class="section">
        <p class="muted-text">Waiting in queue...</p>
      </div>

    {:else if running}
      <div class="section">
        {#if progress}
          <div class="progress-section">
            <div class="progress-header">
              <span class="progress-file" title={progress.current_file}>{progress.current_file}</span>
              <span class="progress-count">{progress.processed} / {progress.total}</span>
            </div>
            <div class="progress-track">
              <div class="progress-bar" style="width: {progress.total > 0 ? Math.round(progress.processed / progress.total * 100) : 0}%"></div>
            </div>
            <div class="inline-stats">
              <span class="text-success">Passed: {progress.passed}</span>
              <span class="text-danger">Failed: {progress.failed}</span>
              <span class="text-warning">Errors: {progress.errors}</span>
            </div>
          </div>
        {:else}
          <p class="muted-text">Starting verification...</p>
        {/if}
      </div>

    {:else if summary}
      <div class="badge-row">
        <span class="badge badge-green">Passed: {summary.passed}</span>
        <span class="badge badge-red">Failed: {summary.failed}</span>
        <span class="badge badge-amber">Errors: {summary.errors}</span>
      </div>

      <div class="results-table-wrap">
        <table class="results-table">
          <thead>
            <tr>
              <th>Filename</th>
              <th>Status</th>
              <th>Detail</th>
            </tr>
          </thead>
          <tbody>
            {#each summary.results as result}
              <tr>
                <td>{result.filename}</td>
                <td class="fw-medium {statusColor(result.status)}">{result.status}</td>
                <td class="detail-col">{result.detail ?? "—"}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}

    <button onclick={onclose} class="btn-close">Close</button>
  </div>
</div>

<style>
  .overlay {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.5);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 50;
  }

  .dialog {
    background: white;
    border-radius: 0.75rem;
    box-shadow: 0 20px 25px -5px rgb(0 0 0 / 0.15);
    padding: 1.5rem;
    max-width: 42rem;
    width: calc(100% - 2rem);
    max-height: 80vh;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 1rem;
  }

  .dialog-title {
    font-size: 1.125rem;
    font-weight: 600;
    margin: 0;
  }

  .section { display: flex; flex-direction: column; gap: 0.75rem; }

  .progress-section { display: flex; flex-direction: column; gap: 0.5rem; }

  .progress-header {
    display: flex;
    justify-content: space-between;
    font-size: 0.75rem;
    color: #64748b;
    min-width: 0;
  }

  .progress-file { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .progress-count { margin-left: 0.5rem; flex-shrink: 0; }

  .progress-track {
    width: 100%;
    background: #e2e8f0;
    border-radius: 9999px;
    height: 0.5rem;
    overflow: hidden;
  }

  .progress-bar {
    background: #3b82f6;
    height: 0.5rem;
    border-radius: 9999px;
    transition: width 0.15s;
  }

  .inline-stats {
    display: flex;
    gap: 1rem;
    font-size: 0.75rem;
  }

  .text-success { color: #22c55e; }
  .text-danger { color: #ef4444; }
  .text-warning { color: #f59e0b; }

  .muted-text { font-size: 0.875rem; color: #64748b; margin: 0; }
  .error-text { font-size: 0.875rem; color: #ef4444; margin: 0; }

  .badge-row { display: flex; gap: 0.75rem; flex-wrap: wrap; }

  .badge {
    padding: 0.25rem 0.75rem;
    border-radius: 9999px;
    font-size: 0.875rem;
  }

  .badge-green { background: #dcfce7; color: #15803d; }
  .badge-red { background: #fee2e2; color: #b91c1c; }
  .badge-amber { background: #fef3c7; color: #92400e; }

  .results-table-wrap {
    border: 1px solid #e2e8f0;
    border-radius: 0.5rem;
    overflow: auto;
  }

  .results-table {
    width: 100%;
    font-size: 0.875rem;
    border-collapse: collapse;
  }

  .results-table thead {
    background: #f8fafc;
    text-align: left;
  }

  .results-table th {
    padding: 0.5rem 0.75rem;
    font-weight: 500;
    color: #475569;
    border-bottom: 1px solid #e2e8f0;
  }

  .results-table tbody tr { border-bottom: 1px solid #f1f5f9; }
  .results-table tbody tr:last-child { border-bottom: none; }
  .results-table tbody tr:hover { background: #f8fafc; }
  .results-table td { padding: 0.5rem 0.75rem; }

  .fw-medium { font-weight: 500; }
  .status-passed { color: #22c55e; }
  .status-failed { color: #ef4444; }
  .status-error { color: #f59e0b; }

  .detail-col { font-size: 0.75rem; color: #64748b; }

  .btn-close {
    padding: 0.5rem 1rem;
    background: #f1f5f9;
    color: #334155;
    border-radius: 0.5rem;
    border: none;
    font-size: 0.875rem;
    cursor: pointer;
    transition: background-color 0.15s;
    align-self: flex-start;
  }

  .btn-close:hover { background: #e2e8f0; }

  @media (prefers-color-scheme: dark) {
    .dialog { background: #1e293b; color: #f1f5f9; }
    .progress-header, .muted-text { color: #94a3b8; }
    .progress-track { background: #334155; }
    .badge-green { background: rgb(21 128 61 / 0.2); color: #4ade80; }
    .badge-red { background: rgb(185 28 28 / 0.2); color: #f87171; }
    .badge-amber { background: rgb(146 64 14 / 0.2); color: #fbbf24; }
    .results-table-wrap { border-color: #334155; }
    .results-table thead { background: #0f172a; }
    .results-table th { color: #94a3b8; border-bottom-color: #334155; }
    .results-table tbody tr { border-bottom-color: #0f172a; }
    .results-table tbody tr:hover { background: rgb(30 41 59 / 0.5); }
    .detail-col { color: #94a3b8; }
    .btn-close { background: #334155; color: #cbd5e1; }
    .btn-close:hover { background: #475569; }
  }
</style>
