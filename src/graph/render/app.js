(() => {
  "use strict";

  const applicationStartedAt = performance.now();
  const svgNamespace = "http://www.w3.org/2000/svg";
  const artifact = JSON.parse(document.querySelector("#graph-data").textContent);
  const graph = {
    ...artifact,
    nodes: artifact.nodes.map((node) => ({
      ...node,
      ...node.common,
      assignees: node.assignees || [],
      labels: node.labels || [],
      state: lifecycleForStatus(node.status),
      readiness: node.status
    }))
  };
  const analysis = graph.analysis;
  const presentation = JSON.parse(
    document.querySelector("#graph-presentation-data").textContent
  );
  const networkView = globalThis.GritNetworkView.create(graph, presentation);
  const query = globalThis.GritGraphQuery.create(graph);
  const canvas = document.querySelector("#graph-canvas");
  const viewport = document.querySelector("#graph-viewport");
  const nodeLayer = document.querySelector("#graph-nodes");
  const edgeLayer = document.querySelector("#graph-edges");
  const layerLabels = document.querySelector("#layer-labels");
  const search = document.querySelector("#graph-search");
  const searchStatus = document.querySelector("#search-status");
  const sizeMetric = document.querySelector("#node-size-metric");
  const colorMetric = document.querySelector("#node-color-metric");
  const recommendationStatus = document.querySelector("#recommendation-status");
  const recommendationEvidence = document.querySelector("#recommendation-evidence");
  const recommendationSelect = document.querySelector("#recommendation-select");
  const viewStatus = document.querySelector("#view-status");
  const detailPanel = document.querySelector("#issue-details");
  const tableBody = document.querySelector("tbody");
  const networkMode = document.querySelector("#network-mode");
  const networkStatus = document.querySelector("#network-mode-status");
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
  const view = {
    isolatedKeys: null,
    highlightedNodes: new Map(),
    highlightedEdges: new Map(),
    recommendationSelected: false
  };
  let zoom = 1;

  const initialRenderStartedAt = performance.now();
  renderGraph(networkView.current(), "initial overview");
  const initialRenderDuration = performance.now() - initialRenderStartedAt;
  renderAnalysis();
  populateControls();
  bindControls();
  applyView();
  document.body.getBoundingClientRect();
  document.documentElement.dataset.gritLoadMs = applicationStartedAt.toFixed(3);
  document.documentElement.dataset.gritRenderMs = initialRenderDuration.toFixed(3);
  document.documentElement.dataset.gritTimeToInteractiveMs = performance.now().toFixed(3);
  document.documentElement.dataset.gritNetworkMode = presentation.mode;

  function lifecycleForStatus(status) {
    if (status === "ready" || status === "blocked" || status === "external_open") return "open";
    if (status === "closed" || status === "external_closed") return "closed";
    return "unknown";
  }

  function renderGraph(keys, description) {
    graphElementsByKey.clear();
    edgeElementsByKey.clear();
    edgeLayer.replaceChildren();
    nodeLayer.replaceChildren();
    layerLabels.replaceChildren();
    const renderedNodes = graph.nodes.filter((node) => keys.has(node.key));
    const marginX = 80;
    const marginY = 70;
    const { minX, minY, width, height } = positionBounds(renderedNodes);
    canvas.setAttribute(
      "viewBox",
      `0 0 ${Math.max(520, width + 260)} ${Math.max(380, height + 150)}`
    );

    for (const edge of graph.edges) {
      if (!keys.has(edge.blocker) || !keys.has(edge.blocked)) continue;
      const blocker = networkView.node(edge.blocker);
      const blocked = networkView.node(edge.blocked);
      if (!blocker || !blocked) continue;
      const line = svgElement("line");
      line.classList.add("graph-edge");
      line.dataset.blocker = edge.blocker;
      line.dataset.blocked = edge.blocked;
      line.setAttribute("x1", blocker.position.x - minX + marginX);
      line.setAttribute("y1", blocker.position.y - minY + marginY);
      line.setAttribute("x2", blocked.position.x - minX + marginX);
      line.setAttribute("y2", blocked.position.y - minY + marginY);
      edgeElementsByKey.set(query.edgeKey(edge.blocker, edge.blocked), line);
      edgeLayer.append(line);
    }

    const labels = new Map();
    for (const node of renderedNodes) {
      const labelKey = query.layerKey(node);
      if (!labels.has(labelKey)) {
        labels.set(labelKey, {
          text: node.position.layer === null ? "Unresolved / SCC" : `Layer ${node.position.layer}`,
          x: node.position.x - minX + marginX
        });
      }

      const group = svgElement("g");
      group.classList.add("graph-node", node.readiness);
      if (node.position.layer === null) group.classList.add("unresolved");
      group.dataset.nodeKey = node.key;
      group.dataset.sourceX = String(node.position.x);
      group.dataset.sourceY = String(node.position.y);
      group.setAttribute(
        "transform",
        `translate(${node.position.x - minX + marginX} ${node.position.y - minY + marginY})`
      );
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
    document.documentElement.dataset.gritRenderedNodes = String(renderedNodes.length);
    document.documentElement.dataset.gritRenderedEdges = String(edgeLayer.children.length);
    const selectedKey = detailPanel.dataset.selectedKey;
    const selectedGraphNode = selectedKey ? graphElement(selectedKey) : null;
    if (selectedGraphNode) {
      selectedGraphNode.classList.add("selected");
      selectedGraphNode.setAttribute("aria-pressed", "true");
    }
    for (const [key, highlight] of view.highlightedNodes) {
      graphElement(key)?.classList.add(highlight.className);
    }
    for (const [key, className] of view.highlightedEdges) {
      edgeElementsByKey.get(key)?.classList.add(className);
    }
    applyVisualMetrics();
    if (view.recommendationSelected) applyRecommendationHighlights();
    updateNetworkStatus(description);
  }

  function positionBounds(nodes) {
    if (nodes.length === 0) return { minX: 0, minY: 0, width: 0, height: 0 };
    let minX = nodes[0].position.x;
    let minY = nodes[0].position.y;
    let maxX = minX;
    let maxY = minY;
    for (let index = 1; index < nodes.length; index += 1) {
      const node = nodes[index];
      minX = Math.min(minX, node.position.x);
      minY = Math.min(minY, node.position.y);
      maxX = Math.max(maxX, node.position.x);
      maxY = Math.max(maxY, node.position.y);
    }
    return { minX, minY, width: maxX - minX, height: maxY - minY };
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
    document.querySelector("#show-initial-network").addEventListener("click", () => {
      renderGraph(networkView.reset(), "initial overview");
      applyView();
    });
    document.querySelector("#show-full-network").addEventListener("click", () => {
      renderGraph(networkView.showFull(), "full network requested by visitor");
      applyView();
    });
    document.querySelector("#isolate-root").addEventListener("click", isolateRoot);
    document.querySelector("#highlight-upstream").addEventListener("click", () => {
      highlightReachable("upstream");
    });
    document.querySelector("#highlight-downstream").addEventListener("click", () => {
      highlightReachable("downstream");
    });
    document.querySelector("#highlight-path").addEventListener("click", highlightPath);
    document.querySelector("#clear-view").addEventListener("click", clearView);
    sizeMetric.addEventListener("change", applyVisualMetrics);
    colorMetric.addEventListener("change", applyVisualMetrics);
    recommendationSelect.addEventListener("click", selectRecommendation);
  }

  function renderAnalysis() {
    const decision = analysis.next;
    const recommendation = decision.recommendation;
    if (!recommendation) {
      recommendationStatus.textContent = "No Issue is executable in this scope.";
      recommendationSelect.hidden = true;
    } else {
      recommendationStatus.textContent = `Do #${recommendation.first_issue.number} ${recommendation.first_issue.title}`;
    }
    const runnerUp = decision.comparison_to_runner_up?.runner_up;
    const onlyCandidate = recommendation?.reasons.find(
      (reason) => reason.reason.code === "only_executable_candidate"
    );
    appendEvidence(
      "Reason",
      decision.comparison_to_runner_up?.message
        || onlyCandidate?.message
        || "No executable recommendation"
    );
    appendEvidence("Runner-up", runnerUp ? `#${runnerUp.number} ${runnerUp.title}` : "None");
    appendEvidence("Search", decision.search_complete ? "Complete" : `Restricted: ${decision.truncated_by.join(", ")}`);
    appendEvidence(
      "Parallel now",
      analysis.plan.parallel_now.map((issue) => `#${issue.number}`).join(", ") || "None"
    );
    appendEvidence("Unresolved", String(analysis.plan.dependency_layers.unresolved.length));
    appendEvidence("Cycles", String(decision.summary.cyclic_issue_count));
    appendEvidence(
      "Unknown External blockers",
      String(graph.nodes.filter((node) => node.readiness === "external_unknown").length)
    );
  }

  function appendEvidence(term, value) {
    const dt = document.createElement("dt");
    dt.textContent = term;
    const dd = document.createElement("dd");
    dd.textContent = value;
    recommendationEvidence.append(dt, dd);
  }

  function selectRecommendation() {
    const recommendation = analysis.next.recommendation;
    if (!recommendation) return;
    view.recommendationSelected = true;
    selectNode(recommendation.first_issue.key);
    applyRecommendationHighlights();
  }

  function applyRecommendationHighlights() {
    const recommendation = analysis.next.recommendation;
    const rolloutKeys = new Set(recommendation.rollout.steps.map((step) => step.issue.key));
    const unlockedKeys = new Set(recommendation.outcome.unlocks.map((unlock) => unlock.issue.key));
    const relevantKeys = new Set([...rolloutKeys, ...unlockedKeys]);
    for (const key of [...relevantKeys]) {
      for (const blocker of query.blockersByKey.get(key) || []) relevantKeys.add(blocker);
    }
    for (const [key, element] of graphElementsByKey) {
      element.classList.toggle("causal-path", rolloutKeys.has(key));
      element.classList.toggle("unlocked-outcome", unlockedKeys.has(key));
      element.classList.toggle("relevant-blocker", relevantKeys.has(key) && !rolloutKeys.has(key) && !unlockedKeys.has(key));
    }
    for (const [key, row] of tableRowsByKey) {
      row.classList.toggle("causal-evidence", relevantKeys.has(key));
    }
    for (const edge of edgeLayer.children) {
      edge.classList.toggle(
        "causal-path",
        relevantKeys.has(edge.dataset.blocker) && relevantKeys.has(edge.dataset.blocked)
      );
    }
  }

  function applyVisualMetrics() {
    const metric = sizeMetric.value;
    const values = graph.nodes.map((node) => Number(node[metric] || 0));
    const maximum = Math.max(1, ...values);
    for (const node of graph.nodes) {
      const element = graphElement(node.key);
      if (!element) continue;
      const circle = element.querySelector("circle");
      const value = Number(node[metric] || 0);
      const radius = metric === "uniform" ? 9 : 7 + 10 * Math.sqrt(value / maximum);
      circle.setAttribute("r", radius.toFixed(2));
      element.dataset.sizeMetric = metric;
      element.dataset.sizeValue = String(value);
      for (const className of [...element.classList]) {
        if (className.startsWith("color-")) element.classList.remove(className);
      }
      element.classList.add(colorClass(node, colorMetric.value));
    }
  }

  function colorClass(node, metric) {
    if (metric === "state") return `color-state-${node.state}`;
    if (metric === "priority") {
      if (!node.priority) return "color-priority-none";
      if (node.priority.state === "declared") return `color-priority-${node.priority.value}`;
      return `color-priority-${node.priority.state}`;
    }
    return `color-readiness-${node.readiness}`;
  }

  function applyView() {
    const searchQuery = search.value.trim().toLocaleLowerCase();
    const numberQuery = searchQuery.match(/^#?(\d+)$/)?.[1] || null;
    const visibleKeys = new Set();
    let visibleNetworkCount = 0;
    for (const node of graph.nodes) {
      const matches = query.matchesSearch(node, searchQuery, numberQuery)
        && query.matchesFilters(node, filterValues)
        && (view.isolatedKeys === null || view.isolatedKeys.has(node.key));
      const graphNode = graphElement(node.key);
      const row = tableRow(node.key);
      if (graphNode) {
        setHidden(graphNode, !matches);
        graphNode.setAttribute("tabindex", matches ? "0" : "-1");
        if (matches) visibleNetworkCount += 1;
      }
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
    const networkSuffix = presentation.mode === "constrained"
      ? `; ${visibleNetworkCount} in the current network view`
      : "";
    searchStatus.textContent = `${visibleKeys.size} of ${graph.nodes.length} nodes visible${networkSuffix}`;
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
    if (presentation.mode === "constrained" && !networkView.contains(key)) {
      renderGraph(networkView.showNeighborhood(key), `neighborhood around ${key}`);
      applyView();
    }
    controls.root.value = key;
    detailPanel.dataset.selectedKey = key;
    for (const element of document.querySelectorAll("[data-node-key]")) {
      const selected = element.dataset.nodeKey === key;
      element.classList.toggle("selected", selected);
      if (element.matches("button, .graph-node")) element.setAttribute("aria-pressed", String(selected));
    }
    renderDetails(node);
  }

  function updateNetworkStatus(description) {
    if (presentation.mode !== "constrained") return;
    networkMode.hidden = false;
    const renderedEdgeCount = [...edgeLayer.children].length;
    networkStatus.textContent = `Showing ${graphElementsByKey.size} of ${graph.nodes.length} nodes and ${renderedEdgeCount} of ${graph.edges.length} edges: ${description}. Positions are precomputed; selecting a table result opens its bounded neighborhood.`;
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
    appendDetail(details, "Layer", node.position.layer === null ? "unresolved / SCC" : String(node.position.layer));
    appendDetail(details, "Assignees", (node.assignees || []).join(", ") || "—");
    appendDetail(details, "Labels", (node.labels || []).join(", ") || "—");
    appendDetail(details, "Declared priority", priorityLabel(node.priority));
    appendDetail(details, "Unlock behavior", node.unlock_count == null ? "—" : String(node.unlock_count));
    appendDetail(details, "PageRank bucket", node.pagerank_bucket == null ? "—" : String(node.pagerank_bucket));
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
    view.highlightedNodes.clear();
    view.highlightedEdges.clear();
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
    view.highlightedNodes.set(key, { className, description });
    graphElement(key)?.classList.add(className);
    const row = tableRow(key);
    row?.classList.add(className);
    const cell = row?.querySelector(".relationship-cell");
    if (cell) cell.textContent = description;
  }

  function markEdge(blocker, blocked, className) {
    const key = query.edgeKey(blocker, blocked);
    view.highlightedEdges.set(key, className);
    edgeElementsByKey.get(key)?.classList.add(className);
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
    view.recommendationSelected = false;
    for (const row of tableRowsByKey.values()) row.classList.remove("causal-evidence");
    renderGraph(networkView.reset(), "initial overview");
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

  function priorityLabel(priority) {
    if (!priority) return "—";
    if (priority.state === "declared") return priority.value;
    return priority.state;
  }

  function svgElement(name) {
    return document.createElementNS(svgNamespace, name);
  }
})();
