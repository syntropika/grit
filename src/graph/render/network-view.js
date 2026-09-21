(() => {
  "use strict";

  function create(graph, presentation) {
    const nodesByKey = new Map(graph.nodes.map((node) => [node.key, node]));
    const blockersByKey = new Map();
    const dependentsByKey = new Map();
    const neighborsByKey = new Map(graph.nodes.map((node) => [node.key, []]));
    for (const edge of graph.edges) {
      append(blockersByKey, edge.blocked, edge.blocker);
      append(dependentsByKey, edge.blocker, edge.blocked);
      neighborsByKey.get(edge.blocker)?.push(edge.blocked);
      neighborsByKey.get(edge.blocked)?.push(edge.blocker);
    }

    const allKeys = graph.nodes.map((node) => node.key);
    const initialKeys = presentation.mode === "constrained"
      ? presentation.initial_node_keys
      : allKeys;
    let currentKeys = new Set(initialKeys);

    return Object.freeze({
      node: (key) => nodesByKey.get(key),
      blockers: (key) => Object.freeze([...(blockersByKey.get(key) || [])]),
      dependents: (key) => Object.freeze([...(dependentsByKey.get(key) || [])]),
      current: () => new Set(currentKeys),
      contains: (key) => currentKeys.has(key),
      reset: () => replace(initialKeys),
      showFull: () => replace(allKeys),
      showNeighborhood: (root) => replace(
        boundedNeighborhood(root, presentation.initial_network_node_limit, neighborsByKey)
      )
    });

    function replace(keys) {
      currentKeys = new Set(keys);
      return new Set(currentKeys);
    }
  }

  function append(index, key, value) {
    const values = index.get(key) || [];
    values.push(value);
    index.set(key, values);
  }

  function boundedNeighborhood(root, limit, neighborsByKey) {
    const selected = new Set([root]);
    const queue = [root];
    for (let index = 0; index < queue.length && selected.size < limit; index += 1) {
      for (const neighbor of neighborsByKey.get(queue[index]) || []) {
        if (selected.has(neighbor)) continue;
        selected.add(neighbor);
        queue.push(neighbor);
        if (selected.size === limit) break;
      }
    }
    return selected;
  }

  globalThis.GritNetworkView = Object.freeze({ create });
})();
