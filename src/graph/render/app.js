(() => {
  "use strict";

  const svgNamespace = "http://www.w3.org/2000/svg";
  const artifact = JSON.parse(document.querySelector("#graph-data").textContent);
  const graph = {
    ...artifact,
    nodes: artifact.nodes.map((node) => ({
      ...node,
      ...node.common,
      state: lifecycleForStatus(node.status),
      readiness: node.status
    }))
  };
  const analysis = graph.analysis;
  const nodesByKey = new Map(graph.nodes.map((node) => [node.key, node]));
  const blockersByKey = relationIndex("blocked", "blocker");
  const dependentsByKey = relationIndex("blocker", "blocked");
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
  const detailPanel = document.querySelector("#issue-details");
  const tableBody = document.querySelector("tbody");
  const graphElementsByKey = new Map();
  const tableRowsByKey = new Map(
    [...tableBody.querySelectorAll("tr[data-node-key]")].map((row) => [row.dataset.nodeKey, row])
  );
  let zoom = 1;

  renderGraph();
  renderAnalysis();
  bindControls();

  function lifecycleForStatus(status) {
    if (status === "ready" || status === "blocked" || status === "external_open") return "open";
    if (status === "closed" || status === "external_closed") return "closed";
    return "unknown";
  }
  applyVisualMetrics();
  applySearch("");

  function relationIndex(sourceField, targetField) {
    const index = new Map();
    for (const edge of graph.edges) {
      const values = index.get(edge[sourceField]) || [];
      values.push(edge[targetField]);
      index.set(edge[sourceField], values);
    }
    return index;
  }

  function renderGraph() {
    const marginX = 80;
    const marginY = 70;
    const maxX = Math.max(0, ...graph.nodes.map((node) => node.position.x));
    const maxY = Math.max(0, ...graph.nodes.map((node) => node.position.y));
    canvas.setAttribute("viewBox", `0 0 ${Math.max(520, maxX + 260)} ${Math.max(380, maxY + 150)}`);

    for (const edge of graph.edges) {
      const blocker = nodesByKey.get(edge.blocker);
      const blocked = nodesByKey.get(edge.blocked);
      if (!blocker || !blocked) continue;
      const line = svgElement("line");
      line.classList.add("graph-edge");
      line.dataset.blocker = edge.blocker;
      line.dataset.blocked = edge.blocked;
      line.setAttribute("x1", blocker.position.x + marginX);
      line.setAttribute("y1", blocker.position.y + marginY);
      line.setAttribute("x2", blocked.position.x + marginX);
      line.setAttribute("y2", blocked.position.y + marginY);
      edgeLayer.append(line);
    }

    const labels = new Map();
    for (const node of graph.nodes) {
      const labelKey = node.position.layer === null ? "unresolved" : `layer-${node.position.layer}`;
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

    for (const label of labels.values()) {
      const text = svgElement("text");
      text.classList.add("layer-label");
      text.setAttribute("x", label.x - 18);
      text.setAttribute("y", "24");
      text.textContent = label.text;
      layerLabels.append(text);
    }
  }

  function bindControls() {
    search.addEventListener("input", () => applySearch(search.value));
    search.addEventListener("keydown", (event) => {
      if (event.key !== "Enter") return;
      const first = visibleTableButtons()[0];
      if (first) {
        event.preventDefault();
        selectNode(first.dataset.nodeKey);
        first.focus();
      }
    });
    tableBody.addEventListener("click", (event) => {
      const button = event.target.closest("button[data-node-key]");
      if (button) selectNode(button.dataset.nodeKey);
    });
    tableBody.addEventListener("keydown", handleTableKeyboard);
    document.querySelector("#zoom-in").addEventListener("click", () => setZoom(zoom + 0.2));
    document.querySelector("#zoom-out").addEventListener("click", () => setZoom(zoom - 0.2));
    document.querySelector("#zoom-reset").addEventListener("click", () => setZoom(1));
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
    const rolloutKeys = new Set(recommendation.rollout.steps.map((step) => step.issue.key));
    const unlockedKeys = new Set(recommendation.outcome.unlocks.map((unlock) => unlock.issue.key));
    const relevantKeys = new Set([...rolloutKeys, ...unlockedKeys]);
    for (const key of [...relevantKeys]) {
      for (const blocker of blockersByKey.get(key) || []) relevantKeys.add(blocker);
    }
    for (const [key, element] of graphElementsByKey) {
      element.classList.toggle("causal-path", rolloutKeys.has(key));
      element.classList.toggle("unlocked-outcome", unlockedKeys.has(key));
      element.classList.toggle("relevant-blocker", relevantKeys.has(key) && !rolloutKeys.has(key) && !unlockedKeys.has(key));
      tableRow(key).classList.toggle("causal-evidence", relevantKeys.has(key));
    }
    for (const edge of edgeLayer.children) {
      edge.classList.toggle(
        "causal-path",
        relevantKeys.has(edge.dataset.blocker) && relevantKeys.has(edge.dataset.blocked)
      );
    }
    selectNode(recommendation.first_issue.key);
  }

  function applyVisualMetrics() {
    const metric = sizeMetric.value;
    const values = graph.nodes.map((node) => Number(node[metric] || 0));
    const maximum = Math.max(1, ...values);
    for (const node of graph.nodes) {
      const element = graphElement(node.key);
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

  function applySearch(rawQuery) {
    const query = rawQuery.trim().toLocaleLowerCase();
    const numberQuery = query.match(/^#?(\d+)$/)?.[1] || null;
    let visibleCount = 0;
    for (const node of graph.nodes) {
      const matches = !query
        || (numberQuery !== null && String(node.number) === numberQuery)
        || (node.title || "").toLocaleLowerCase().includes(query);
      const graphNode = graphElement(node.key);
      const row = tableRow(node.key);
      setHidden(graphNode, !matches);
      graphNode.setAttribute("tabindex", matches ? "0" : "-1");
      setHidden(row, !matches);
      if (matches) visibleCount += 1;
    }
    for (const edge of edgeLayer.children) {
      setHidden(
        edge,
        isHidden(graphElement(edge.dataset.blocker)) || isHidden(graphElement(edge.dataset.blocked))
      );
    }
    searchStatus.textContent = `${visibleCount} ${visibleCount === 1 ? "result" : "results"}`;
  }

  function selectNode(key) {
    const node = nodesByKey.get(key);
    if (!node) return;
    detailPanel.dataset.selectedKey = key;
    for (const element of document.querySelectorAll("[data-node-key]")) {
      const selected = element.dataset.nodeKey === key;
      element.classList.toggle("selected", selected);
      if (element.matches("button, .graph-node")) element.setAttribute("aria-pressed", String(selected));
    }
    renderDetails(node);
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
    detailPanel.append(heading, key, badge, details);
    appendRelations("Blockers", blockersByKey.get(node.key) || []);
    appendRelations("Dependents", dependentsByKey.get(node.key) || []);
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

  function isHidden(element) {
    return element.hasAttribute("hidden");
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
