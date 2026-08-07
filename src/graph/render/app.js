(() => {
  "use strict";

  const applicationStartedAt = performance.now();
  const svgNamespace = "http://www.w3.org/2000/svg";
  const graph = JSON.parse(document.querySelector("#graph-data").textContent);
  const presentation = JSON.parse(
    document.querySelector("#graph-presentation-data").textContent
  );
  const networkView = globalThis.GritNetworkView.create(graph, presentation);
  const canvas = document.querySelector("#graph-canvas");
  const viewport = document.querySelector("#graph-viewport");
  const nodeLayer = document.querySelector("#graph-nodes");
  const edgeLayer = document.querySelector("#graph-edges");
  const layerLabels = document.querySelector("#layer-labels");
  const search = document.querySelector("#graph-search");
  const searchStatus = document.querySelector("#search-status");
  const detailPanel = document.querySelector("#issue-details");
  const tableBody = document.querySelector("tbody");
  const networkMode = document.querySelector("#network-mode");
  const networkStatus = document.querySelector("#network-mode-status");
  const graphElementsByKey = new Map();
  const tableRowsByKey = new Map(
    [...tableBody.querySelectorAll("tr[data-node-key]")].map((row) => [row.dataset.nodeKey, row])
  );
  let zoom = 1;

  const initialRenderStartedAt = performance.now();
  renderGraph(networkView.current(), "initial overview");
  const initialRenderDuration = performance.now() - initialRenderStartedAt;
  bindControls();
  applySearch("");
  document.body.getBoundingClientRect();
  document.documentElement.dataset.gritLoadMs = applicationStartedAt.toFixed(3);
  document.documentElement.dataset.gritRenderMs = initialRenderDuration.toFixed(3);
  document.documentElement.dataset.gritTimeToInteractiveMs = performance.now().toFixed(3);
  document.documentElement.dataset.gritNetworkMode = presentation.mode;

  function renderGraph(keys, description) {
    graphElementsByKey.clear();
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
      edgeLayer.append(line);
    }

    const labels = new Map();
    for (const node of renderedNodes) {
      const labelKey = node.position.layer === null ? "unresolved" : `layer-${node.position.layer}`;
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

    for (const label of labels.values()) {
      const text = svgElement("text");
      text.classList.add("layer-label");
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
    document.querySelector("#show-initial-network").addEventListener("click", () => {
      renderGraph(networkView.reset(), "initial overview");
      applySearch(search.value);
    });
    document.querySelector("#show-full-network").addEventListener("click", () => {
      renderGraph(networkView.showFull(), "full network requested by visitor");
      applySearch(search.value);
    });
  }

  function applySearch(rawQuery) {
    const query = rawQuery.trim().toLocaleLowerCase();
    const numberQuery = query.match(/^#?(\d+)$/)?.[1] || null;
    let visibleCount = 0;
    let visibleNetworkCount = 0;
    for (const node of graph.nodes) {
      const matches = !query
        || (numberQuery !== null && String(node.number) === numberQuery)
        || (node.title || "").toLocaleLowerCase().includes(query);
      const graphNode = graphElement(node.key);
      const row = tableRow(node.key);
      if (graphNode) {
        setHidden(graphNode, !matches);
        graphNode.setAttribute("tabindex", matches ? "0" : "-1");
        if (matches) visibleNetworkCount += 1;
      }
      setHidden(row, !matches);
      if (matches) visibleCount += 1;
    }
    for (const edge of edgeLayer.children) {
      setHidden(
        edge,
        isHidden(graphElement(edge.dataset.blocker)) || isHidden(graphElement(edge.dataset.blocked))
      );
    }
    const networkSuffix = presentation.mode === "constrained"
      ? `; ${visibleNetworkCount} in the current network view`
      : "";
    searchStatus.textContent = `${visibleCount} ${visibleCount === 1 ? "result" : "results"}${networkSuffix}`;
  }

  function selectNode(key) {
    const node = networkView.node(key);
    if (!node) return;
    if (presentation.mode === "constrained" && !networkView.contains(key)) {
      renderGraph(networkView.showNeighborhood(key), `neighborhood around ${key}`);
      applySearch(search.value);
    }
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
    appendDetail(details, "Assignees", node.assignees.join(", ") || "—");
    appendDetail(details, "Labels", node.labels.join(", ") || "—");
    detailPanel.append(heading, key, badge, details);
    appendRelations("Blockers", networkView.blockers(node.key));
    appendRelations("Dependents", networkView.dependents(node.key));
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

  function svgElement(name) {
    return document.createElementNS(svgNamespace, name);
  }
})();
