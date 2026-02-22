let selectedIds = $state<Set<number>>(new Set());

export const selectionStore = {
  get ids() { return selectedIds; },
  get count() { return selectedIds.size; },
  get array() { return [...selectedIds]; },

  toggle(id: number) {
    const next = new Set(selectedIds);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    selectedIds = next;
  },

  selectAll(ids: number[]) {
    selectedIds = new Set(ids);
  },

  clear() {
    selectedIds = new Set();
  },

  has(id: number) {
    return selectedIds.has(id);
  },
};
