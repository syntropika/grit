(() => {
  "use strict";

  const svgNamespace = "http://www.w3.org/2000/svg";
  const graph = JSON.parse(document.querySelector("#graph-data").textContent);
  const query = globalThis.GritGraphQuery.create(graph);
  const canvas = document.querySelector("#graph-canvas");
  const viewport = document.querySelector("#graph-viewport");
  const nodeLayer = document.querySelector("#graph-nodes");
  const edgeLayer = document.querySelector("#graph-edges");
  const layerLabels = document.querySelector("#layer-labels");
  const search = document.querySelector("#graph-search");
  const searchStatus = document.querySelector("#search-status");
  const viewStatus = document.querySelector("#view-status");
  const detailPanel = document.querySelector("#issue-details");
  const tableBody = document.querySelector("tbody");
  const graphElementsByKey = new Map();
  const edgeElementsByKey = new Map();
  const tableRowsByKey = new Map(
    [...tableBody.querySelectorAll("tr[data-node-key]")].map((row) => [row.dataset.nodeKey, row])
  );
  const filterControls = new Map();
  const filterValues = new Map(
    query.filters.map((filter) => [filter.key, filter.defaultValue])
  );
  const controls = {
    root: document.querySelector("#root-node"),
    depth: document.querySelector("#root-depth"),
    pathTarget: document.querySelector("#path-target")
  };
  const view = { isolatedKeys: null };
  let zoom = 1;

  renderGraph();
  populateControls();
  bindControls();
  applyView();

  function renderGraph() {
    const marginX = 80;
    const marginY = 70;
    const maxX = Math.max(0, ...graph.nodes.map((node) => node.position.x));
    const maxY = Math.max(0, ...graph.nodes.map((node) => node.position.y));
    canvas.setAttribute("viewBox", `0 0 ${Math.max(520, maxX + 260)} ${Math.max(380, maxY + 150)}`);

    for (const edge of graph.edges) {
      const blocker = query.nodesByKey.get(edge.blocker);
      const blocked = query.nodesByKey.get(edge.blocked);
      if (!blocker || !blocked) continue;
      const line = svgElement("line");
      line.classList.add("graph-edge");
      line.dataset.blocker = edge.blocker;
      line.dataset.blocked = edge.blocked;
      line.setAttribute("x1", blocker.position.x + marginX);
      line.setAttribute("y1", blocker.position.y + marginY);
      line.setAttribute("x2", blocked.position.x + marginX);
      line.setAttribute("y2", blocked.position.y + marginY);
      edgeElementsByKey.set(query.edgeKey(edge.blocker, edge.blocked), line);
      edgeLayer.append(line);
    }

    const labels = new Map();
    for (const node of graph.nodes) {
      const labelKey = query.layerKey(node);
      if (!labels.has(labelKey)) {
        labels.set(labelKey, {
          text: node.position.layer === null ? "Unresolved / SCC" : `Layer ${node.position.layer}`,
          x: node.position.x + marginX
        });
      }

      const group = svgElement("g");
      group.classList.add("graph-node", node.readiness);
      if (node.position.layer === null) group.classList.add("unresolved");
      group.dataset.nodeKey = node.key;
      group.dataset.sourceX = String(node.position.x);
      group.dataset.sourceY = String(node.position.y);
      group.setAttribute("transform", `translate(${node.position.x + marginX} ${node.position.y + marginY})`);
      group.setAttribute("tabindex", "0");
      group.setAttribute("role", "button");
      group.setAttribute("aria-pressed", "false");
      group.setAttribute("aria-label", nodeLabel(node));

      const title = svgElement("title");
      title.textContent = nodeLabel(node);
      const circle = svgElement("circle");
      circle.setAttribute("r", "9");
      const text = svgElement("text");
      text.classList.add("node-label");
      text.setAttribute("x", "15");
      text.setAttribute("y", "4");
      text.textContent = node.title || node.key;
      group.append(title, circle, text);
      group.addEventListener("click", () => selectNode(node.key));
      group.addEventListener("keydown", (event) => {
        if (event.key === "Enter" || event.key === " ") {
          event.preventDefault();
          selectNode(node.key);
        }
      });
      graphElementsByKey.set(node.key, group);
      nodeLayer.append(group);
    }

    for (const [key, label] of labels) {
      const text = svgElement("text");
      text.classList.add("layer-label");
      text.dataset.layerKey = key;
      text.setAttribute("x", label.x - 18);
      text.setAttribute("y", "24");
      text.textContent = label.text;
      layerLabels.append(text);
    }
  }

  function populateControls() {
    for (const filter of query.filters) {
      const control = filter.controlSelector
        ? document.querySelector(filter.controlSelector)
        : createOptionalFilter(filter);
      for (const item of filter.options) appendOption(control, item.value, item.label);
      if (filter.hideWhenEmpty) {
        const minimum = filter.minimumOptionCount ?? 1;
        control.closest("label").hidden = filter.options.length < minimum;
      }
      filterControls.set(filter.key, control);
    }

    for (const node of graph.nodes) {
      const label = node.title ? `${node.key} — ${node.title}` : node.key;
      appendOption(controls.root, node.key, label);
      appendOption(controls.pathTarget, node.key, label);
    }
    if (graph.nodes.length === 0) disableRelationshipControls();
  }

  function createOptionalFilter(filter) {
    const label = document.createElement("label");
    const id = `${filter.key}-filter`;
    label.htmlFor = id;
    label.append(filter.label);
    const select = document.createElement("select");
    select.id = id;
    appendOption(select, filter.defaultValue, filter.allLabel);
    label.append(select);
    document.querySelector("#optional-filters").append(label);
    return select;
  }

  function appendOption(select, value, label) {
    const option = document.createElement("option");
    option.value = value;
    option.textContent = label;
    select.append(option);
  }

  function disableRelationshipControls() {
    for (const control of [
      controls.root,
      controls.depth,
      controls.pathTarget,
      document.querySelector("#isolate-root"),
      document.querySelector("#highlight-upstream"),
      document.querySelector("#highlight-downstream"),
      document.querySelector("#highlight-path")
    ]) {
      control.disabled = true;
    }
    viewStatus.textContent = "No graph nodes are available";
  }

  function bindControls() {
    search.addEventListener("input", applyView);
    search.addEventListener("keydown", (event) => {
      if (event.key !== "Enter") return;
      const first = visibleTableButtons()[0];
      if (first) {
        event.preventDefault();
        selectNode(first.dataset.nodeKey);
        first.focus();
      }
    });
    for (const filter of query.filters) {
      const control = filterControls.get(filter.key);
      control.addEventListener("change", () => {
        filterValues.set(filter.key, control.value);
        applyView();
      });
    }
    tableBody.addEventListener("click", (event) => {
      const button = event.target.closest("button[data-node-key]");
      if (button) selectNode(button.dataset.nodeKey);
    });
    tableBody.addEventListener("keydown", handleTableKeyboard);
    document.querySelector("#zoom-in").addEventListener("click", () => setZoom(zoom + 0.2));
    document.querySelector("#zoom-out").addEventListener("click", () => setZoom(zoom - 0.2));
    document.querySelector("#zoom-reset").addEventListener("click", () => setZoom(1));
    document.querySelector("#isolate-root").addEventListener("click", isolateRoot);
    document.querySelector("#highlight-upstream").addEventListener("click", () => {
      highlightReachable("upstream");
    });
    document.querySelector("#highlight-downstream").addEventListener("click", () => {
      highlightReachable("downstream");
    });
    document.querySelector("#highlight-path").addEventListener("click", highlightPath);
    document.querySelector("#clear-view").addEventListener("click", clearView);
  }

  function applyView() {
    const searchQuery = search.value.trim().toLocaleLowerCase();
    const numberQuery = searchQuery.match(/^#?(\d+)$/)?.[1] || null;
    const visibleKeys = new Set();
    for (const node of graph.nodes) {
      const matches = query.matchesSearch(node, searchQuery, numberQuery)
        && query.matchesFilters(node, filterValues)
        && (view.isolatedKeys === null || view.isolatedKeys.has(node.key));
      const graphNode = graphElement(node.key);
      const row = tableRow(node.key);
      setHidden(graphNode, !matches);
      graphNode.setAttribute("tabindex", matches ? "0" : "-1");
      setHidden(row, !matches);
      if (matches) visibleKeys.add(node.key);
    }
    for (const edge of edgeLayer.children) {
      setHidden(edge, !visibleKeys.has(edge.dataset.blocker) || !visibleKeys.has(edge.dataset.blocked));
    }
    const visibleLayers = new Set(
      graph.nodes.filter((node) => visibleKeys.has(node.key)).map(query.layerKey)
    );
    for (const label of layerLabels.children) {
      setHidden(label, !visibleLayers.has(label.dataset.layerKey));
    }
    updateTableRelationships(visibleKeys);
    searchStatus.textContent = `${visibleKeys.size} of ${graph.nodes.length} nodes visible`;
  }

  function updateTableRelationships(visibleKeys) {
    for (const node of graph.nodes) {
      const row = tableRow(node.key);
      row.querySelector(".blockers-cell").textContent = visibleRelations(
        query.blockersByKey.get(node.key),
        visibleKeys
      );
      row.querySelector(".dependents-cell").textContent = visibleRelations(
        query.dependentsByKey.get(node.key),
        visibleKeys
      );
    }
  }

  function visibleRelations(relations, visibleKeys) {
    const visible = (relations || []).filter((key) => visibleKeys.has(key));
    return visible.length === 0 ? "—" : visible.join(", ");
  }

  function selectNode(key) {
    const node = query.nodesByKey.get(key);
    if (!node) return;
    controls.root.value = key;
    detailPanel.dataset.selectedKey = key;
    for (const element of document.querySelectorAll("[data-node-key]")) {
      const selected = element.dataset.nodeKey === key;
      element.classList.toggle("selected", selected);
      if (element.matches("button, .graph-node")) element.setAttribute("aria-pressed", String(selected));
    }
    renderDetails(node);
  }

  function clearSelection() {
    delete detailPanel.dataset.selectedKey;
    for (const element of document.querySelectorAll("[data-node-key]")) {
      element.classList.remove("selected");
      if (element.matches("button, .graph-node")) element.setAttribute("aria-pressed", "false");
    }
    detailPanel.replaceChildren();
    const heading = document.createElement("h2");
    heading.id = "detail-heading";
    heading.textContent = "Issue details";
    const prompt = document.createElement("p");
    prompt.textContent = "Select a node from the graph or table.";
    detailPanel.append(heading, prompt);
  }

  function renderDetails(node) {
    detailPanel.replaceChildren();
    const heading = document.createElement("h2");
    heading.id = "detail-heading";
    heading.textContent = node.title || node.key;
    const key = document.createElement("p");
    key.className = "detail-key";
    key.textContent = node.key;
    const badge = document.createElement("span");
    badge.className = `readiness-badge ${node.readiness}`;
    badge.textContent = node.readiness;
    const details = document.createElement("dl");
    appendDetail(details, "State", node.state);
    appendDetail(
      details,
      "Priority",
      node.kind === "issue" ? query.declaredPriority(node) : "—"
    );
    appendDetail(details, "Layer", node.position.layer === null ? "unresolved / SCC" : String(node.position.layer));
    appendDetail(details, "Assignees", node.assignees.join(", ") || "—");
    appendDetail(details, "Labels", node.labels.join(", ") || "—");
    const projects = query.projectsFor(node);
    if (projects.length > 0) appendDetail(details, "Projects", projects.join(", "));
    detailPanel.append(heading, key, badge, details);
    appendRelations("Blockers", query.blockersByKey.get(node.key) || []);
    appendRelations("Dependents", query.dependentsByKey.get(node.key) || []);
    if (node.url) {
      const link = document.createElement("a");
      link.href = node.url;
      link.textContent = "Open canonical GitHub Issue";
      link.rel = "noopener noreferrer";
      detailPanel.append(link);
    }
  }

  function appendDetail(list, term, value) {
    const dt = document.createElement("dt");
    dt.textContent = term;
    const dd = document.createElement("dd");
    dd.textContent = value;
    list.append(dt, dd);
  }

  function appendRelations(title, keys) {
    const heading = document.createElement("h3");
    heading.textContent = title;
    const list = document.createElement("ul");
    if (keys.length === 0) {
      const item = document.createElement("li");
      item.textContent = "None";
      list.append(item);
    } else {
      for (const key of keys) {
        const item = document.createElement("li");
        const button = document.createElement("button");
        button.type = "button";
        button.className = "relation-button";
        button.textContent = key;
        button.addEventListener("click", () => selectNode(key));
        item.append(button);
        list.append(item);
      }
    }
    detailPanel.append(heading, list);
  }

  function isolateRoot() {
    const root = controls.root.value;
    if (!query.nodesByKey.has(root)) return;
    const parsedDepth = Number(controls.depth.value);
    const depth = Number.isFinite(parsedDepth) ? Math.max(0, Math.floor(parsedDepth)) : 0;
    controls.depth.value = String(depth);
    view.isolatedKeys = query.neighborhood(root, depth);
    clearRelationshipHighlights();
    selectNode(root);
    applyView();
    viewStatus.textContent = `Limited to ${view.isolatedKeys.size} nodes within depth ${depth} of ${root}`;
  }

  function highlightReachable(direction) {
    const root = controls.root.value;
    if (!query.nodesByKey.has(root)) return;
    const result = query.reachable(root, direction);
    const className = `relationship-${direction}`;
    const description = direction === "upstream" ? "upstream blocker" : "downstream dependent";
    clearRelationshipHighlights();
    markNode(root, "relationship-root", "focus root");
    for (const key of result.nodes) markNode(key, className, description);
    for (const edge of result.edges) markEdge(edge.blocker, edge.blocked, className);
    selectNode(root);
    viewStatus.textContent = `Highlighted ${result.nodes.length} ${description}${result.nodes.length === 1 ? "" : "s"} for ${root}`;
  }

  function highlightPath() {
    const root = controls.root.value;
    const target = controls.pathTarget.value;
    if (!query.nodesByKey.has(root) || !query.nodesByKey.has(target)) return;
    const path = query.shortestDirectedPath(root, target);
    clearRelationshipHighlights();
    if (path === null) {
      viewStatus.textContent = `No directed Dependency path from ${root} to ${target}`;
      return;
    }
    for (const key of path) markNode(key, "relationship-path", "selected path");
    for (let index = 1; index < path.length; index += 1) {
      markEdge(path[index - 1], path[index], "relationship-path");
    }
    selectNode(root);
    viewStatus.textContent = `Highlighted ${path.length}-node path from ${root} to ${target}`;
  }

  function clearRelationshipHighlights() {
    for (const element of [...graphElementsByKey.values(), ...tableRowsByKey.values()]) {
      element.classList.remove(
        "relationship-root",
        "relationship-upstream",
        "relationship-downstream",
        "relationship-path"
      );
    }
    for (const edge of edgeLayer.children) {
      edge.classList.remove("relationship-upstream", "relationship-downstream", "relationship-path");
    }
    for (const row of tableRowsByKey.values()) row.querySelector(".relationship-cell").textContent = "—";
  }

  function markNode(key, className, description) {
    graphElement(key)?.classList.add(className);
    const row = tableRow(key);
    row?.classList.add(className);
    const cell = row?.querySelector(".relationship-cell");
    if (cell) cell.textContent = description;
  }

  function markEdge(blocker, blocked, className) {
    edgeElementsByKey.get(query.edgeKey(blocker, blocked))?.classList.add(className);
  }

  function clearView() {
    search.value = "";
    for (const filter of query.filters) {
      filterValues.set(filter.key, filter.defaultValue);
      filterControls.get(filter.key).value = filter.defaultValue;
    }
    view.isolatedKeys = null;
    controls.depth.value = "1";
    controls.root.selectedIndex = 0;
    controls.pathTarget.selectedIndex = 0;
    clearRelationshipHighlights();
    clearSelection();
    setZoom(1);
    applyView();
    viewStatus.textContent = "Canonical graph restored";
  }

  function handleTableKeyboard(event) {
    const current = event.target.closest("button[data-node-key]");
    if (!current) return;
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      selectNode(current.dataset.nodeKey);
      return;
    }
    const buttons = visibleTableButtons();
    const currentIndex = buttons.indexOf(current);
    let nextIndex = currentIndex;
    if (event.key === "ArrowDown") nextIndex = Math.min(buttons.length - 1, currentIndex + 1);
    else if (event.key === "ArrowUp") nextIndex = Math.max(0, currentIndex - 1);
    else if (event.key === "Home") nextIndex = 0;
    else if (event.key === "End") nextIndex = buttons.length - 1;
    else return;
    event.preventDefault();
    buttons[nextIndex]?.focus();
  }

  function visibleTableButtons() {
    return [...tableBody.querySelectorAll("tr:not([hidden]) button.table-node")];
  }

  function setZoom(value) {
    zoom = Math.min(2, Math.max(0.6, Math.round(value * 10) / 10));
    canvas.dataset.zoom = String(zoom);
    viewport.setAttribute("transform", `scale(${zoom})`);
    document.querySelector("#zoom-reset").textContent = `${Math.round(zoom * 100)}%`;
  }

  function graphElement(key) {
    return graphElementsByKey.get(key);
  }

  function setHidden(element, hidden) {
    element.toggleAttribute("hidden", hidden);
  }

  function tableRow(key) {
    return tableRowsByKey.get(key);
  }

  function nodeLabel(node) {
    return `${node.key}: ${node.title || "External blocker"}; ${node.readiness}`;
  }

  function svgElement(name) {
    return document.createElementNS(svgNamespace, name);
  }
})();
