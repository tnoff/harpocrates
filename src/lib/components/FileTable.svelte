<script lang="ts">
  import { selectionStore } from "$lib/stores/selection.svelte";

  interface FileEntry {
    id: number;
    object_uuid: string;
    filename: string;
    local_path: string;
    file_size: number;
    original_md5: string;
    created_at: string;
  }

  interface Props {
    files: FileEntry[];
    selectable?: boolean;
  }

  let { files, selectable = true }: Props = $props();

  function formatSize(bytes: number): string {
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
    if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
    return `${(bytes / (1024 * 1024 * 1024)).toFixed(1)} GB`;
  }

  function toggleAll() {
    if (selectionStore.count === files.length) {
      selectionStore.clear();
    } else {
      selectionStore.selectAll(files.map(f => f.id));
    }
  }

  const allSelected = $derived(files.length > 0 && selectionStore.count === files.length);
</script>

<div class="table-wrap">
  <table>
    <thead>
      <tr>
        {#if selectable}
          <th class="col-check">
            <input type="checkbox" checked={allSelected} onchange={toggleAll} />
          </th>
        {/if}
        <th>Filename</th>
        <th>Path</th>
        <th>Size</th>
        <th>Date</th>
        <th>MD5</th>
      </tr>
    </thead>
    <tbody>
      {#each files as file}
        <tr class:selected={selectionStore.has(file.id)}>
          {#if selectable}
            <td>
              <input type="checkbox" checked={selectionStore.has(file.id)} onchange={() => selectionStore.toggle(file.id)} />
            </td>
          {/if}
          <td class="col-name">{file.filename}</td>
          <td class="col-path" title={file.local_path}>{file.local_path}</td>
          <td class="col-nowrap">{formatSize(file.file_size)}</td>
          <td class="col-nowrap">{new Date(file.created_at).toLocaleDateString()}</td>
          <td class="col-mono" title={file.original_md5}>{file.original_md5.slice(0, 8)}...</td>
        </tr>
      {:else}
        <tr>
          <td colspan={selectable ? 7 : 6} class="col-empty">No files found</td>
        </tr>
      {/each}
    </tbody>
  </table>
</div>

<style>
  .table-wrap {
    overflow: auto;
    border: 1px solid #e2e8f0;
    border-radius: 0.5rem;
  }

  table {
    width: 100%;
    font-size: 0.875rem;
    border-collapse: collapse;
  }

  thead {
    background: #f8fafc;
    text-align: left;
  }

  th {
    padding: 0.5rem 0.75rem;
    font-weight: 500;
    color: #475569;
    border-bottom: 1px solid #e2e8f0;
    white-space: nowrap;
  }

  .col-check { width: 2rem; }

  tbody tr {
    border-bottom: 1px solid #f1f5f9;
    transition: background-color 0.1s;
  }

  tbody tr:last-child { border-bottom: none; }
  tbody tr:hover { background: #f8fafc; }
  tbody tr.selected { background: rgb(59 130 246 / 0.05); }

  td { padding: 0.5rem 0.75rem; }

  .col-name { font-weight: 500; }
  .col-path {
    color: #64748b;
    max-width: 12rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .col-nowrap { white-space: nowrap; }
  .col-mono { font-family: monospace; font-size: 0.75rem; color: #64748b; }
  .col-empty { text-align: center; padding: 2rem 0.75rem; color: #64748b; }

  @media (prefers-color-scheme: dark) {
    .table-wrap { border-color: #334155; }
    thead { background: #1e293b; }
    th { color: #94a3b8; border-bottom-color: #334155; }
    tbody tr { border-bottom-color: #0f172a; }
    tbody tr:hover { background: rgb(30 41 59 / 0.5); }
    tbody tr.selected { background: rgb(59 130 246 / 0.08); }
    .col-path, .col-mono { color: #94a3b8; }
    .col-empty { color: #94a3b8; }
  }
</style>
