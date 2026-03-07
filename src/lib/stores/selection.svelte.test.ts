import { describe, it, expect, beforeEach } from 'vitest';
import { selectionStore } from './selection.svelte';

beforeEach(() => {
  selectionStore.clear();
});

describe('selectionStore', () => {
  it('starts empty', () => {
    expect(selectionStore.count).toBe(0);
    expect(selectionStore.ids.size).toBe(0);
  });

  it('toggle adds an id', () => {
    selectionStore.toggle(1);
    expect(selectionStore.has(1)).toBe(true);
    expect(selectionStore.count).toBe(1);
  });

  it('toggle removes an existing id', () => {
    selectionStore.toggle(1);
    selectionStore.toggle(1);
    expect(selectionStore.count).toBe(0);
    expect(selectionStore.has(1)).toBe(false);
  });

  it('toggle multiple ids independently', () => {
    selectionStore.toggle(1);
    selectionStore.toggle(2);
    selectionStore.toggle(3);
    expect(selectionStore.count).toBe(3);
    expect(selectionStore.has(1)).toBe(true);
    expect(selectionStore.has(2)).toBe(true);
    expect(selectionStore.has(3)).toBe(true);
  });

  it('selectAll replaces entire selection', () => {
    selectionStore.toggle(1);
    selectionStore.selectAll([2, 3]);
    expect(selectionStore.has(1)).toBe(false);
    expect(selectionStore.count).toBe(2);
    expect(selectionStore.has(2)).toBe(true);
    expect(selectionStore.has(3)).toBe(true);
  });

  it('selectAll with empty array clears', () => {
    selectionStore.selectAll([1, 2]);
    selectionStore.selectAll([]);
    expect(selectionStore.count).toBe(0);
  });

  it('clear empties the set', () => {
    selectionStore.toggle(1);
    selectionStore.toggle(2);
    selectionStore.clear();
    expect(selectionStore.count).toBe(0);
  });

  it('array returns all ids', () => {
    selectionStore.selectAll([10, 20]);
    expect(selectionStore.array).toContain(10);
    expect(selectionStore.array).toContain(20);
    expect(selectionStore.array).toHaveLength(2);
  });

  it('has returns false for missing id', () => {
    expect(selectionStore.has(99)).toBe(false);
  });

  it('count stays accurate across operations', () => {
    selectionStore.toggle(1);
    selectionStore.toggle(2);
    expect(selectionStore.count).toBe(2);
    selectionStore.clear();
    expect(selectionStore.count).toBe(0);
    selectionStore.selectAll([5, 6, 7]);
    expect(selectionStore.count).toBe(3);
    selectionStore.toggle(5);
    expect(selectionStore.count).toBe(2);
  });
});
