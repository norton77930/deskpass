import type { TreeScale } from "./tree-format.ts";

/**
 * A UI tree the helper read. Its `treeId` names element references the helper instance that
 * read it holds, so the record means nothing once another instance runs.
 */
export interface StoredTree {
  treeId: string;
  windowId: string;
  /** The helper instance (`HelperClient.instance`) that read it. */
  instance: number | undefined;
  /** The screenshot read together with it, if any. */
  screenshotId?: string;
  /** The scale its positions were written in. */
  scale: TreeScale;
  /** How many numbered elements it has (`element_index` 1 to this); unknown when absent. */
  elementCount?: number;
}

/** Like the helper, which keeps the element references of this many trees per window. */
const MAX_PER_WINDOW = 5;

/** What steps record and find trees through: the store, or a batch's view of it (`asOfNow`). */
export type TreeView = Pick<TreeStore, "add" | "get" | "latest">;

export class TreeStore {
  private instance: number | undefined;
  private trees: StoredTree[] = [];

  add(tree: StoredTree): StoredTree {
    this.sync(tree.instance);
    this.trees = this.trees.filter((kept) => kept.treeId !== tree.treeId);
    this.trees.push(tree);
    const ofWindow = this.trees.filter((kept) => kept.windowId === tree.windowId);
    const dropped = new Set(ofWindow.slice(0, Math.max(0, ofWindow.length - MAX_PER_WINDOW)));
    this.trees = this.trees.filter((kept) => !dropped.has(kept));
    return tree;
  }

  /** The tree `treeId` of the running helper `instance`, if it is still kept. */
  get(treeId: string, instance: number | undefined): StoredTree | undefined {
    this.sync(instance);
    return this.trees.find((tree) => tree.treeId === treeId);
  }

  /** The newest tree of `windowId` read by the running helper `instance`. */
  latest(windowId: string, instance: number | undefined): StoredTree | undefined {
    this.sync(instance);
    return this.trees.filter((tree) => tree.windowId === windowId).at(-1);
  }

  /**
   * Ticket 76: the store as a batch sees it. Trees are recorded and found by id as usual, but each
   * window's latest tree stays the one it is now: a step without tree_id keeps to the tree the
   * batch began with (none once it is dropped, or the helper that read it has ended).
   */
  asOfNow(): TreeView {
    const latest = new Map<string, string>();
    for (const tree of this.trees) latest.set(tree.windowId, tree.treeId);
    return {
      add: (tree) => this.add(tree),
      get: (treeId, instance) => this.get(treeId, instance),
      latest: (windowId, instance) => {
        const treeId = latest.get(windowId);
        return treeId === undefined ? undefined : this.get(treeId, instance);
      },
    };
  }

  /** A helper restart (a timeout or crash) took every element reference with it. */
  private sync(instance: number | undefined): void {
    if (instance === this.instance && instance !== undefined) return;
    this.trees = [];
    this.instance = instance;
  }
}
