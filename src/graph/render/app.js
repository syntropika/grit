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
  const networkView = globalThis.HyfaNetworkView.create(graph, presentation);
  const query = globalThis.HyfaGraphQuery.create(graph);
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
    outcome: "all",
    isolatedKeys: null,
    highlightedNodes: new Map(),
    highlightedEdges: new Map(),
    recommendationSelected: false
  };
  let layoutMode = presentation.network_positions ? "network" : "layers";
  document.body.dataset.layout = layoutMode;
  let zoom = 1;
  let labelZoom = null;
  let mapOrigin = { x: 0, y: 0 };
  const camera = globalThis.HyfaGraphCamera.create(canvas, viewport, updateCameraLabels);

  const initialRenderStartedAt = performance.now();
  renderGraph(networkView.current(), "initial overview");
  const initialRenderDuration = performance.now() - initialRenderStartedAt;
  renderAnalysis();
  populateControls();
  bindControls();
  applyView();
  camera.fit();
  document.body.getBoundingClientRect();
  document.documentElement.dataset.hyfaLoadMs = applicationStartedAt.toFixed(3);
  document.documentElement.dataset.hyfaRenderMs = initialRenderDuration.toFixed(3);
  document.documentElement.dataset.hyfaTimeToInteractiveMs = performance.now().toFixed(3);
  document.documentElement.dataset.hyfaNetworkMode = presentation.mode;

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
    mapOrigin = { x: minX - marginX, y: minY - marginY };
    camera.setBounds({ x: marginX - 30, y: marginY - 50, width: Math.max(80, width + 60), height: Math.max(80, height + 100) });

    for (const edge of graph.edges) {
      if (!keys.has(edge.blocker) || !keys.has(edge.blocked)) continue;
      const blocker = networkView.node(edge.blocker);
      const blocked = networkView.node(edge.blocked);
      if (!blocker || !blocked) continue;
      const line = svgElement("line");
      line.classList.add("graph-edge");
      line.dataset.blocker = edge.blocker;
      line.dataset.blocked = edge.blocked;
      line.setAttribute("x1", displayPosition(blocker).x - minX + marginX);
      line.setAttribute("y1", displayPosition(blocker).y - minY + marginY);
      line.setAttribute("x2", displayPosition(blocked).x - minX + marginX);
      line.setAttribute("y2", displayPosition(blocked).y - minY + marginY);
      edgeElementsByKey.set(query.edgeKey(edge.blocker, edge.blocked), line);
      edgeLayer.append(line);
    }

    const labels = new Map();
    for (const node of renderedNodes) {
      const position = displayPosition(node);
      const labelKey = query.layerKey(node);
      if (!labels.has(labelKey)) {
        labels.set(labelKey, {
          text: node.state === "closed" ? "Closed history" : node.position.layer === null ? "Unresolved / SCC" : `Layer ${node.position.layer}`,
          x: position.x - minX + marginX,
          y: position.y - minY + marginY - 36
        });
      }

      const group = svgElement("g");
      group.classList.add("graph-node", node.readiness);
      if (node.resolution) group.classList.add(`resolution-${node.resolution}`);
      if (node.position.layer === null && node.state !== "closed") group.classList.add("unresolved");
      group.dataset.nodeKey = node.key;
      group.dataset.sourceX = String(node.position.x);
      group.dataset.sourceY = String(node.position.y);
      group.dataset.displayX = String(position.x);
      group.dataset.displayY = String(position.y);
      group.setAttribute(
        "transform",
        `translate(${position.x - minX + marginX} ${position.y - minY + marginY})`
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
      if (renderedNodes.length <= 120) {
        const summary = svgElement("text");
        summary.classList.add("node-summary");
        summary.setAttribute("x", "17");
        summary.setAttribute("y", "4");
        const shortTitle = (node.title || "External blocker").slice(0, 31);
        summary.textContent = `${node.number == null ? "Draft" : `#${node.number}`} ${shortTitle}${(node.title || "").length > 31 ? "…" : ""}`;
        summary.dataset.fullLabel = summary.textContent;
        summary.dataset.shortLabel = node.number == null ? "Draft" : `#${node.number}`;
        group.append(summary);
      }
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
      text.setAttribute("y", String(label.y));
      text.textContent = label.text;
      layerLabels.append(text);
    }
    document.documentElement.dataset.hyfaRenderedNodes = String(renderedNodes.length);
    document.documentElement.dataset.hyfaRenderedEdges = String(edgeLayer.children.length);
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
    highlightConnections(detailPanel.dataset.selectedKey);
    updateNetworkStatus(description);
    camera.fit();
  }

  function displayPosition(node) {
    return layoutMode === "network" ? (presentation.network_positions?.[node.key] || node.position) : node.position;
  }

  function positionBounds(nodes) {
    if (nodes.length === 0) return { minX: 0, minY: 0, width: 0, height: 0 };
    let minX = displayPosition(nodes[0]).x;
    let minY = displayPosition(nodes[0]).y;
    let maxX = minX;
    let maxY = minY;
    for (let index = 1; index < nodes.length; index += 1) {
      const node = nodes[index];
      minX = Math.min(minX, displayPosition(node).x);
      minY = Math.min(minY, displayPosition(node).y);
      maxX = Math.max(maxX, displayPosition(node).x);
      maxY = Math.max(maxY, displayPosition(node).y);
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
    const layoutControl = document.querySelector("#graph-layout");
    layoutControl.value = layoutMode;
    layoutControl.disabled = !presentation.network_positions;
    layoutControl.addEventListener("change", () => {
      layoutMode = layoutControl.value;
      document.body.dataset.layout = layoutMode;
      renderGraph(networkView.current(), "layout changed");
      applyView();
      fitVisibleGraph();
    });
    document.querySelector("#expand-map").addEventListener("click", () => expandMap());
    document.addEventListener("keydown", (event) => {
      if (event.key === "Escape") {
        if (detailPanel.dataset.selectedKey) {
          clearSelection();
          if (document.body.dataset.mapExpanded === "true") canvas.focus({ preventScroll: true });
        }
        else if (document.body.dataset.mapExpanded === "true") expandMap(false);
      }
      if (event.key === "/" && !event.target.matches("input, select, textarea") && document.body.dataset.mapExpanded !== "true") {
        event.preventDefault();
        search.focus();
      }
      if (event.key === "Tab" && document.body.dataset.mapExpanded === "true") {
        const targets = [...document.querySelector(".explorer").querySelectorAll("button, a, select, [tabindex='0']")]
          .filter((target) => target.getBoundingClientRect().width && !target.disabled);
        const first = targets[0];
        const last = targets[targets.length - 1];
        if (event.shiftKey && (document.activeElement === first || !targets.includes(document.activeElement))) {
          event.preventDefault();
          last?.focus();
        } else if (!event.shiftKey && (document.activeElement === last || !targets.includes(document.activeElement))) {
          event.preventDefault();
          first?.focus();
        }
      }
    });
    for (const button of document.querySelectorAll("[data-work-view]")) {
      button.addEventListener("click", () => showOutcome(button.dataset.workView));
    }
    document.querySelector("#show-completed").addEventListener("click", () => showOutcome("completed"));
    document.querySelector("#reset-filters").addEventListener("click", clearView);
    document.querySelector("#completion-list").addEventListener("click", (event) => {
      const button = event.target.closest("button[data-node-key]");
      if (!button) return;
      showOutcome("all");
      selectNode(button.dataset.nodeKey);
      detailPanel.scrollIntoView({ block: "nearest" });
    });
    search.addEventListener("input", applyView);
    search.addEventListener("keydown", (event) => {
      if (event.key !== "Enter") return;
      const first = visibleTableButtons()[0];
      if (first) {
        event.preventDefault();
        selectNode(first.dataset.nodeKey);
        graphElement(first.dataset.nodeKey)?.focus();
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
    document.querySelector("#zoom-in").addEventListener("click", () => setZoom(zoom * 1.25));
    document.querySelector("#zoom-out").addEventListener("click", () => setZoom(zoom / 1.25));
    document.querySelector("#zoom-reset").addEventListener("click", () => setZoom(1));
    document.querySelector("#zoom-fit").addEventListener("click", fitVisibleGraph);
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

  function issueLabel(issue) {
    return issue.number == null ? issue.key : `#${issue.number}`;
  }

  function renderAnalysis() {
    const decision = analysis.next;
    const recommendation = decision.recommendation;
    if (!recommendation) {
      recommendationStatus.textContent = "No Issue is executable in this scope.";
      recommendationSelect.hidden = true;
    } else {
      recommendationStatus.textContent = `Do ${issueLabel(recommendation.first_issue)} ${recommendation.first_issue.title}${recommendation.pending ? " [pending]" : ""}`;
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
    appendEvidence("Runner-up", runnerUp ? `${issueLabel(runnerUp)} ${runnerUp.title}` : "None");
    appendEvidence("Search", decision.search_complete ? "Complete" : `Restricted: ${decision.truncated_by.join(", ")}`);
    appendEvidence(
      "Parallel now",
      analysis.plan.parallel_now.map(issueLabel).join(", ") || "None"
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
    showOutcome("all");
    document.querySelector("#graph-region").scrollIntoView({ block: "start" });
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
      element.dataset.baseRadius = radius.toFixed(2);
      for (const className of [...element.classList]) {
        if (className.startsWith("color-")) element.classList.remove(className);
      }
      element.classList.add(colorClass(node, colorMetric.value));
    }
    const legends = {
      readiness: [["ready", "Ready"], ["blocked", "Blocked"], ["completed", "Completed"], ["closed", "Other closed"], ["unknown", "Unresolved / cycle"]],
      state: [["open", "Open"], ["closed", "Closed"], ["unknown", "Unknown"]],
      priority: [["p0", "P0"], ["p1", "P1"], ["p2", "P2"], ["p3", "P3"], ["p4", "P4"], ["none", "Unspecified"], ["unknown", "Conflict"]]
    };
    document.querySelector(".graph-legend").replaceChildren(
      ...legends[colorMetric.value].map(([state, label]) => {
        const item = document.createElement("span");
        item.className = `legend-${state}`;
        item.textContent = label;
        return item;
      })
    );
    updateCameraLabels({ zoom }, true);
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
        && matchesOutcome(node)
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
    document.querySelector("#graph-empty").hidden = visibleKeys.size !== 0;
    for (const button of document.querySelectorAll("[data-work-view]")) {
      button.setAttribute("aria-pressed", String(button.dataset.workView === view.outcome));
    }
    if (detailPanel.dataset.selectedKey && !visibleKeys.has(detailPanel.dataset.selectedKey)) {
      clearSelection();
    }
    document.querySelector("#map-node-count").textContent = `${visibleNetworkCount} nodes · ${[...edgeLayer.children].filter((edge) => !edge.hasAttribute("hidden")).length} connections`;
  }

  function matchesOutcome(node) {
    if (view.outcome === "all") return true;
    if (node.kind !== "issue") return false;
    if (view.outcome === "open") return node.state === "open";
    return node.state === "closed" && (node.resolution || "other") === view.outcome;
  }

  function showOutcome(outcome) {
    clearView();
    view.outcome = outcome;
    if (presentation.mode === "constrained"
      && !graph.nodes.some((node) => networkView.contains(node.key) && matchesOutcome(node))) {
      const first = graph.nodes.find(matchesOutcome);
      if (first) renderGraph(networkView.showNeighborhood(first.key), `${outcome} Issue neighborhood`);
    }
    applyView();
    const label = outcome === "not_planned" ? "not planned" : outcome === "other" ? "other closed" : outcome;
    viewStatus.textContent = `Showing ${label} Issues. Recommendations still use open work only.`;
    fitVisibleGraph();
  }

  function stateLabel(node) {
    if (node.state !== "closed" || node.kind !== "issue") return node.state;
    if (node.resolution === "completed") return "Completed";
    if (node.resolution === "not_planned") return "Not planned";
    return "Closed · unspecified";
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
    camera.focus({ x: displayPosition(node).x - mapOrigin.x, y: displayPosition(node).y - mapOrigin.y });
    document.body.dataset.detailsOpen = "true";
    highlightConnections(key);
  }

  function updateNetworkStatus(description) {
    if (presentation.mode !== "constrained") return;
    networkMode.hidden = false;
    const renderedEdgeCount = [...edgeLayer.children].length;
    networkStatus.textContent = `Showing ${graphElementsByKey.size} of ${graph.nodes.length} nodes and ${renderedEdgeCount} of ${graph.edges.length} edges: ${description}. Positions are precomputed; selecting a table result opens its bounded neighborhood.`;
  }

  function clearSelection() {
    delete detailPanel.dataset.selectedKey;
    document.body.dataset.detailsOpen = "false";
    highlightConnections(null);
    for (const element of document.querySelectorAll("[data-node-key]")) {
      element.classList.remove("selected");
      if (element.matches("button, .graph-node")) element.setAttribute("aria-pressed", "false");
    }
    detailPanel.replaceChildren();
    const heading = document.createElement("h2");
    heading.id = "detail-heading";
    heading.textContent = "Issue details";
    const prompt = document.createElement("p");
    prompt.className = "detail-prompt";
    prompt.textContent = "Select an Issue to see its status, blockers, and the work it unlocks.";
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
    badge.textContent = node.state === "closed" ? stateLabel(node) : node.readiness;
    const details = document.createElement("dl");
    appendDetail(details, "State", stateLabel(node));
    appendDetail(details, "Layer", node.state === "closed" ? "History (not operational)" : node.position.layer === null ? "unresolved / SCC" : String(node.position.layer));
    appendDetail(details, "Assignees", (node.assignees || []).join(", ") || "—");
    appendDetail(details, "Labels", (node.labels || []).join(", ") || "—");
    appendDetail(details, "Declared priority", priorityLabel(node.priority));
    appendDetail(details, "PageRank bucket", node.pagerank_bucket == null ? "—" : String(node.pagerank_bucket));
    const projects = query.projectsFor(node);
    if (projects.length > 0) appendDetail(details, "Projects", projects.join(", "));
    const close = document.createElement("button");
    close.type = "button";
    close.className = "detail-close";
    close.textContent = "Close";
    close.setAttribute("aria-label", "Close Issue details");
    close.addEventListener("click", () => { clearSelection(); canvas.focus({ preventScroll: true }); });
    detailPanel.append(close, heading, key, badge, details);
    appendImpact(node.impact);
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

  function appendImpact(impact) {
    if (!impact) return;
    const section = document.createElement("section");
    section.className = "dependency-impact";
    section.setAttribute("aria-label", "Dependency impact");
    const heading = document.createElement("h3");
    heading.textContent = "Dependency impact";
    const explanation = document.createElement("p");
    explanation.textContent = impact.explanation;
    const metrics = document.createElement("dl");
    appendDetail(metrics, "Direct open dependents", String(impact.direct_dependents));
    appendDetail(metrics, "Longest downstream chain", impact.chain_depth.state === "finite"
      ? `${impact.chain_depth.edges} dependency steps (not a duration)`
      : "Unavailable: a downstream cycle is reachable");
    section.append(heading, explanation, metrics);
    if (impact.pending) {
      const pending = document.createElement("p");
      pending.className = "detail-prompt";
      pending.textContent = "Includes pending local changes.";
      section.append(pending);
    }
    if (!impact.downstream.complete) {
      const partial = document.createElement("p");
      partial.textContent = "Downstream counts are lower bounds; some work was not inspected.";
      section.append(partial);
    }
    detailPanel.append(section);
    if (impact.immediate_unlocks?.count > 0) {
      appendRelations(`Would become ready (showing ${impact.immediate_unlocks.examples.length} of ${impact.immediate_unlocks.count})`, impact.immediate_unlocks.examples);
    }
    if (impact.still_blocked?.examples.length > 0) {
      const title = document.createElement("h3");
      title.textContent = "Why other work would remain blocked";
      const list = document.createElement("ul");
      for (const outcome of impact.still_blocked.examples) {
        const item = document.createElement("li");
        const button = relationButton(outcome.key);
        const reason = document.createElement("p");
        const blockers = outcome.blockers.map(blocker =>
          `${blocker.key || "Unknown internal Issue"}${blocker.external ? " (external)" : ""}${blocker.unknown ? " (unknown state)" : ""}`);
        reason.textContent = `Still needs: ${blockers.join(", ")}.${outcome.blocker_count > blockers.length ? ` Showing ${blockers.length} of ${outcome.blocker_count} blockers.` : ""}${outcome.cyclic ? " Part of a dependency cycle." : ""}`;
        item.append(button, reason);
        list.append(item);
      }
      const shown = document.createElement("p");
      shown.className = "detail-prompt";
      shown.textContent = `Showing ${impact.still_blocked.examples.length} of ${impact.still_blocked.total.complete ? "" : "at least "}${impact.still_blocked.total.count} remaining blocked Issues.`;
      detailPanel.append(title, shown, list);
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
        item.append(relationButton(key));
        list.append(item);
      }
    }
    detailPanel.append(heading, list);
  }

  function relationButton(key) {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "relation-button";
    button.textContent = key;
    button.addEventListener("click", () => selectNode(key));
    return button;
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
    view.outcome = "all";
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
    applyView();
    fitVisibleGraph();
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
    camera.zoomTo(value);
  }

  function updateCameraLabels(state, force = false) {
    zoom = state.zoom;
    if (!force && zoom === labelZoom) return;
    labelZoom = zoom;
    document.querySelector("#zoom-reset").textContent = `${Math.round(zoom * 100)}%`;
    const arrow = document.querySelector("#arrow");
    arrow.setAttribute("markerWidth", String(7 / zoom));
    arrow.setAttribute("markerHeight", String(7 / zoom));
    for (const edge of edgeLayer.children) {
      const target = query.nodesByKey.get(edge.dataset.blocked);
      const end = displayPosition(target);
      const startX = Number(edge.getAttribute("x1"));
      const startY = Number(edge.getAttribute("y1"));
      const dx = end.x - mapOrigin.x - startX;
      const dy = end.y - mapOrigin.y - startY;
      const distance = Math.hypot(dx, dy) || 1;
      const radius = Math.max(4 / zoom, Number(graphElement(target.key)?.dataset.baseRadius || 9));
      const remaining = Math.max(0, distance - radius - 2 / zoom) / distance;
      edge.setAttribute("x2", String(startX + dx * remaining));
      edge.setAttribute("y2", String(startY + dy * remaining));
    }
    for (const element of graphElementsByKey.values()) {
      element.querySelector("circle").setAttribute("r", String(Math.max(4 / zoom, Number(element.dataset.baseRadius || 9))));
      for (const label of element.querySelectorAll(".node-summary, .node-label")) {
        label.setAttribute("font-size", String(12 / zoom));
        label.setAttribute("x", String(16 / zoom));
        label.setAttribute("y", String(4 / zoom));
        label.setAttribute("stroke-width", String(3 / zoom));
        if (label.classList.contains("node-summary")) label.textContent = layoutMode === "network" || zoom < 0.65 ? label.dataset.shortLabel : label.dataset.fullLabel;
      }
    }
  }

  function fitVisibleGraph() {
    const nodes = graph.nodes.filter((node) => {
      const element = graphElement(node.key);
      return element && !element.hasAttribute("hidden");
    });
    const bounds = positionBounds(nodes);
    camera.setBounds({ x: bounds.minX - mapOrigin.x - 30, y: bounds.minY - mapOrigin.y - 50, width: Math.max(80, bounds.width + 60), height: Math.max(80, bounds.height + 100) });
    camera.fit();
  }

  function highlightConnections(key) {
    const connected = new Set([...(query.blockersByKey.get(key) || []), ...(query.dependentsByKey.get(key) || [])]);
    for (const [nodeKey, element] of graphElementsByKey) element.classList.toggle("connected", connected.has(nodeKey));
    for (const edge of edgeLayer.children) edge.classList.toggle("selected-connection", edge.dataset.blocker === key || edge.dataset.blocked === key);
  }

  function expandMap(expanded = document.body.dataset.mapExpanded !== "true") {
    document.body.dataset.mapExpanded = String(expanded);
    const button = document.querySelector("#expand-map");
    button.textContent = expanded ? "Exit expanded map" : "Expand map";
    button.setAttribute("aria-expanded", String(expanded));
    const explorer = document.querySelector(".explorer");
    for (const element of document.querySelectorAll("main > :not(.explorer), .site-header, .site-footer, .skip-link")) element.inert = expanded;
    if (expanded) {
      explorer.setAttribute("role", "dialog");
      explorer.setAttribute("aria-modal", "true");
      explorer.setAttribute("aria-label", "Expanded dependency map");
      canvas.focus({ preventScroll: true });
    } else {
      explorer.removeAttribute("role");
      explorer.removeAttribute("aria-modal");
      explorer.removeAttribute("aria-label");
      button.focus({ preventScroll: true });
    }
    fitVisibleGraph();
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
