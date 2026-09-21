(() => {
  "use strict";

  const allFilterValue = "sentinel:all";
  const unassignedFilterValue = "sentinel:unassigned";

  function create(graph) {
    const nodesByKey = new Map(graph.nodes.map((node) => [node.key, node]));
    const blockersByKey = relationIndex(graph, "blocked", "blocker");
    const dependentsByKey = relationIndex(graph, "blocker", "blocked");
    const neighborsByKey = undirectedNeighbors(graph);
    const components = connectedComponents(graph.nodes, neighborsByKey);
    const filters = filterDefinitions(graph.nodes, components);

    return Object.freeze({
      nodesByKey,
      blockersByKey,
      dependentsByKey,
      filters,
      declaredPriority,
      projectsFor,
      layerKey,
      edgeKey,
      matchesSearch,
      matchesFilters: (node, values) => filters.every(
        (filter) => filter.matches(node, values.get(filter.key) ?? filter.defaultValue)
      ),
      neighborhood: (root, depth) => neighborhood(root, depth, neighborsByKey),
      reachable: (root, direction) => reachable(
        graph.edges,
        root,
        direction,
        blockersByKey,
        dependentsByKey
      ),
      shortestDirectedPath: (root, target) => shortestDirectedPath(
        root,
        target,
        dependentsByKey
      )
    });
  }

  function relationIndex(graph, sourceField, targetField) {
    const index = new Map();
    for (const edge of graph.edges) {
      const values = index.get(edge[sourceField]) || [];
      values.push(edge[targetField]);
      index.set(edge[sourceField], values);
    }
    return index;
  }

  function undirectedNeighbors(graph) {
    const neighbors = new Map(graph.nodes.map((node) => [node.key, []]));
    for (const edge of graph.edges) {
      neighbors.get(edge.blocker)?.push(edge.blocked);
      neighbors.get(edge.blocked)?.push(edge.blocker);
    }
    return neighbors;
  }

  function breadthFirst(start, neighborsFor, maximumDepth = Number.POSITIVE_INFINITY) {
    const order = [];
    const distances = new Map([[start, 0]]);
    const parents = new Map([[start, null]]);
    const queue = [start];
    for (let queueIndex = 0; queueIndex < queue.length; queueIndex += 1) {
      const current = queue[queueIndex];
      const currentDepth = distances.get(current);
      order.push(current);
      if (currentDepth >= maximumDepth) continue;
      for (const neighbor of neighborsFor(current)) {
        if (distances.has(neighbor)) continue;
        distances.set(neighbor, currentDepth + 1);
        parents.set(neighbor, current);
        queue.push(neighbor);
      }
    }
    return { order, distances, parents };
  }

  function connectedComponents(nodes, neighborsByKey) {
    const byKey = new Map();
    const summaries = [];
    for (const node of nodes) {
      if (byKey.has(node.key)) continue;
      const id = `component-${summaries.length + 1}`;
      const traversal = breadthFirst(node.key, (key) => neighborsByKey.get(key) || []);
      for (const key of traversal.order) byKey.set(key, id);
      summaries.push({ id, members: traversal.order });
    }
    return { byKey, summaries };
  }

  function neighborhood(root, depth, neighborsByKey) {
    return new Set(
      breadthFirst(root, (key) => neighborsByKey.get(key) || [], depth).order
    );
  }

  function reachable(edges, root, direction, blockersByKey, dependentsByKey) {
    const relations = direction === "upstream" ? blockersByKey : dependentsByKey;
    const traversal = breadthFirst(root, (key) => relations.get(key) || []);
    const keys = new Set(traversal.order);
    return {
      nodes: traversal.order.slice(1),
      edges: edges.filter((edge) => keys.has(edge.blocker) && keys.has(edge.blocked))
    };
  }

  function shortestDirectedPath(root, target, dependentsByKey) {
    const traversal = breadthFirst(root, (key) => dependentsByKey.get(key) || []);
    if (!traversal.parents.has(target)) return null;
    const path = [];
    for (let current = target; current !== null; current = traversal.parents.get(current)) {
      path.push(current);
    }
    return path.reverse();
  }

  function filterDefinitions(nodes, components) {
    const areas = uniqueValues(nodes.flatMap((node) => node.labels.filter(isAreaLabel)));
    const assignees = uniqueValues(nodes.flatMap((node) => node.assignees));
    const projects = uniqueValues(nodes.flatMap(projectsFor));
    const definitions = [
      valueFilter(
        "readiness",
        "#readiness-filter",
        uniqueValues(nodes.map((node) => node.readiness)),
        (node) => node.readiness
      ),
      valueFilter(
        "state",
        "#state-filter",
        uniqueValues(nodes.map((node) => node.state)),
        (node) => node.state
      ),
      {
        ...valueFilter(
          "priority",
          "#priority-filter",
          ["p0", "p1", "p2", "p3", "p4", "unspecified", "conflict"],
          declaredPriority
        ),
        matches: (node, value) => value === allFilterValue
          || (node.kind === "issue" && actualValue(declaredPriority(node)) === value)
      },
      valueFilter("area", "#area-filter", areas, (node) => node.labels, {
        hideWhenEmpty: true,
        multiple: true
      }),
      {
        key: "assignee",
        controlSelector: "#assignee-filter",
        defaultValue: allFilterValue,
        options: [option(unassignedFilterValue, "Unassigned"), ...options(assignees)],
        matches: (node, value) => {
          if (value === allFilterValue) return true;
          if (node.kind !== "issue") return false;
          if (value === unassignedFilterValue) return node.assignees.length === 0;
          return node.assignees.some((assignee) => actualValue(assignee) === value);
        }
      },
      {
        key: "component",
        controlSelector: "#component-filter",
        defaultValue: allFilterValue,
        hideWhenEmpty: true,
        minimumOptionCount: 2,
        options: components.summaries.map((component, index) => option(
          actualValue(component.id),
          `Component ${index + 1} · ${component.members.length} nodes · ${component.members[0]}`
        )),
        matches: (node, value) => value === allFilterValue
          || actualValue(components.byKey.get(node.key)) === value
      }
    ];
    if (projects.length > 0) {
      definitions.push(valueFilter(
        "project",
        null,
        projects,
        projectsFor,
        { label: "Project", allLabel: "All Projects", multiple: true }
      ));
    }
    return definitions;
  }

  function valueFilter(key, controlSelector, values, valueFor, settings = {}) {
    const multiple = settings.multiple === true;
    return {
      key,
      controlSelector,
      defaultValue: allFilterValue,
      hideWhenEmpty: settings.hideWhenEmpty === true,
      label: settings.label,
      allLabel: settings.allLabel,
      options: options(values),
      matches: (node, value) => {
        if (value === allFilterValue) return true;
        const actual = valueFor(node);
        if (multiple) return actual.some((candidate) => actualValue(candidate) === value);
        return actualValue(actual) === value;
      }
    };
  }

  function matchesSearch(node, query, numberQuery) {
    return !query
      || (numberQuery !== null && String(node.number) === numberQuery)
      || (node.key || "").toLocaleLowerCase().includes(query)
      || (node.title || "").toLocaleLowerCase().includes(query);
  }

  function declaredPriority(node) {
    if (node.priority) return node.priority.state === "declared" ? node.priority.value : node.priority.state;
    const priorities = new Set(
      node.labels
        .map((label) => normalize(label).match(/^priority:(p[0-4])$/)?.[1])
        .filter(Boolean)
    );
    if (priorities.size === 0) return "unspecified";
    if (priorities.size > 1) return "conflict";
    return [...priorities][0];
  }

  function projectsFor(node) {
    return Array.isArray(node.projects)
      ? node.projects.filter((value) => typeof value === "string")
      : [];
  }

  function isAreaLabel(label) {
    return normalize(label).startsWith("area:");
  }

  function uniqueValues(values) {
    const byNormalizedValue = new Map();
    for (const value of values) {
      const normalized = normalize(value);
      if (!byNormalizedValue.has(normalized)) byNormalizedValue.set(normalized, value);
    }
    return [...byNormalizedValue.values()].sort((left, right) => left.localeCompare(right));
  }

  function options(values) {
    return values.map((value) => option(actualValue(value), value));
  }

  function option(value, label) {
    return { value, label };
  }

  function normalize(value) {
    return value.toLocaleLowerCase();
  }

  function actualValue(value) {
    return `value:${normalize(value)}`;
  }

  function layerKey(node) {
    return node.position.layer === null ? "unresolved" : `layer-${node.position.layer}`;
  }

  function edgeKey(blocker, blocked) {
    return `${blocker}\u0000${blocked}`;
  }

  globalThis.GritGraphQuery = Object.freeze({ create });
})();
