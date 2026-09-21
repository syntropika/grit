const frame = document.querySelector("#app");
const constrainedFrame = document.querySelector("#constrained-app");
const projectFrame = document.querySelector("#project-app");
const result = document.querySelector("#result");

const exercise = () => {
  try {
    const doc = frame.contentDocument;
    const data = readGraph(doc);
    const harness = createHarness(doc, data);
    const projectDoc = projectFrame.contentDocument;
    const projectData = readGraph(projectDoc);
    const projectHarness = createHarness(projectDoc, projectData);
    const presentation = JSON.parse(doc.querySelector("#graph-presentation-data").textContent);
    const checks = {
      full_network_within_validated_range: presentation.mode === "full"
        && doc.querySelector("#network-mode").hidden,
      performance_markers: Number(doc.documentElement.dataset.gritLoadMs) >= 0
        && Number(doc.documentElement.dataset.gritRenderMs) >= 0
        && Number(doc.documentElement.dataset.gritTimeToInteractiveMs)
          >= Number(doc.documentElement.dataset.gritLoadMs),
      ...exerciseConstrainedMode(constrainedFrame.contentDocument),
      ...exerciseOutcomes(harness),
      ...exerciseSearchAndSelection(harness),
      ...exerciseFilters(harness),
      ...exerciseIsolation(harness),
      ...exerciseRelationships(harness),
      ...exerciseKeyboardAndZoom(harness),
      ...exerciseHostileText(harness),
      ...exerciseRecommendation(harness),
      ...exerciseProjectFilter(projectHarness),
      accessible_table_matches_visible_graph: harness.tableMatchesGraph(),
      project_filter_absent_without_data: !doc.querySelector("#project-filter")
    };
    result.textContent = JSON.stringify({ checks });
  } catch (error) {
    result.textContent = JSON.stringify({ error: String(error), stack: error.stack });
  }
};

function exerciseConstrainedMode(doc) {
  const search = doc.querySelector("#graph-search");
  const visibleGraphKeys = () => [...doc.querySelectorAll(".graph-node:not([hidden])")]
    .map((node) => node.dataset.nodeKey)
    .sort();
  const visibleTableKeys = () => [...doc.querySelectorAll("tbody tr:not([hidden])")]
    .map((row) => row.dataset.nodeKey)
    .sort();
  const initial = !doc.querySelector("#network-mode").hidden
    && visibleGraphKeys().join(",") === "acme/widgets#1"
    && visibleTableKeys().length === 8;

  search.value = "#5";
  search.dispatchEvent(new Event("input", { bubbles: true }));
  const searchable = visibleTableKeys().join(",") === "acme/widgets#5";
  doc.querySelector('tr[data-node-key="acme/widgets#5"] button').click();
  search.value = "";
  search.dispatchEvent(new Event("input", { bubbles: true }));
  const neighborhood = visibleGraphKeys().join(",") === "acme/widgets#4,acme/widgets#5";
  const node = doc.querySelector('.graph-node[data-node-key="acme/widgets#5"]');
  const data = readGraph(doc);
  const source = data.nodes.find((candidate) => candidate.key === "acme/widgets#5");
  const precomputed = node.dataset.sourceX === String(source.position.x)
    && node.dataset.sourceY === String(source.position.y);

  doc.querySelector("#show-initial-network").click();
  const reset = visibleGraphKeys().join(",") === "acme/widgets#1";
  doc.querySelector("#show-full-network").click();
  const expanded = visibleGraphKeys().length === data.nodes.length;

  doc.querySelector("#clear-view").click();
  const priority = doc.querySelector("#priority-filter");
  priority.value = "value:p0";
  priority.dispatchEvent(new Event("change", { bubbles: true }));
  const completeFilteredTable = visibleGraphKeys().length === 0
    && visibleTableKeys().join(",") === "acme/widgets#2";
  doc.querySelector('tr[data-node-key="acme/widgets#2"] button').click();
  const filteredSelection = visibleGraphKeys().join(",") === "acme/widgets#2"
    && visibleTableKeys().join(",") === "acme/widgets#2";

  doc.querySelector("#clear-view").click();
  doc.querySelector("#root-node").value = "acme/widgets#2";
  doc.querySelector("#highlight-upstream").click();
  const boundedHighlight = doc.querySelector('.graph-node[data-node-key="acme/widgets#1"]')
    .classList.contains("relationship-upstream")
    && doc.querySelector('.graph-node[data-node-key="acme/widgets#2"]')
      .classList.contains("relationship-root");
  doc.querySelector("#show-full-network").click();
  const expandedHighlight = doc.querySelector('.graph-node[data-node-key="partners/platform#42"]')
    .classList.contains("relationship-upstream")
    && doc.querySelectorAll(".graph-edge.relationship-upstream").length === 2;

  doc.querySelector("#clear-view").click();
  doc.querySelector("#root-node").value = "acme/widgets#4";
  doc.querySelector("#root-depth").value = "1";
  doc.querySelector("#isolate-root").click();
  const isolated = visibleGraphKeys().join(",") === "acme/widgets#4,acme/widgets#5"
    && visibleTableKeys().join(",") === "acme/widgets#4,acme/widgets#5";
  doc.querySelector("#clear-view").click();
  const cleared = visibleGraphKeys().join(",") === "acme/widgets#1"
    && visibleTableKeys().length === data.nodes.length
    && doc.querySelectorAll(".relationship-upstream, .relationship-root").length === 0;
  doc.querySelector('tr[data-node-key="acme/widgets#5"] button').click();
  const sizeControl = doc.querySelector("#node-size-metric");
  sizeControl.value = "pagerank_bucket";
  sizeControl.dispatchEvent(new Event("change", { bubbles: true }));
  const colorControl = doc.querySelector("#node-color-metric");
  colorControl.value = "priority";
  colorControl.dispatchEvent(new Event("change", { bubbles: true }));
  doc.querySelector("#recommendation-select").click();
  const recommendation = doc.querySelector('.graph-node[data-node-key="acme/widgets#1"]');
  const recommendationOpens = recommendation.classList.contains("causal-path")
    && recommendation.classList.contains("color-priority-p1")
    && recommendation.dataset.sizeMetric === "pagerank_bucket"
    && doc.querySelector('tr[data-node-key="acme/widgets#7"]').classList.contains("causal-evidence");
  doc.querySelector("#show-full-network").click();
  const outcomeAppears = doc.querySelector('.graph-node[data-node-key="acme/widgets#7"]')
    .classList.contains("unlocked-outcome");
  doc.querySelector("#clear-view").click();
  const clearedRecommendation = doc.querySelectorAll(".causal-path, .unlocked-outcome, .causal-evidence").length === 0;
  return {
    constrained_recommendation_preserves_metrics_and_evidence: recommendationOpens && outcomeAppears && clearedRecommendation,
    constrained_mode_opens_bounded: initial,
    constrained_search_keeps_table: searchable,
    selected_result_opens_neighborhood: neighborhood && precomputed,
    constrained_reset_and_explicit_expand: reset && expanded,
    constrained_filters_keep_complete_table: completeFilteredTable && filteredSelection,
    constrained_highlights_survive_window_changes: boundedHighlight && expandedHighlight,
    constrained_isolation_and_clear: isolated && cleared
  };
}

function createHarness(doc, data) {
  const visibleKeys = (selector) => [...doc.querySelectorAll(selector)]
    .filter((element) => !element.hasAttribute("hidden"))
    .map((element) => element.dataset.nodeKey)
    .sort();
  const visibleGraphKeys = () => visibleKeys("#graph-nodes .graph-node");
  const visibleTableKeys = () => visibleKeys("tbody tr[data-node-key]");
  const relationshipText = (key, role, visible) => {
    const related = data.edges
      .filter((edge) => edge[role] === key && visible.has(edge.blocked) && visible.has(edge.blocker))
      .map((edge) => edge[role === "blocked" ? "blocker" : "blocked"]);
    return related.length === 0 ? "\u2014" : related.join(", ");
  };
  const tableRelationshipsMatchGraph = () => {
    const visible = new Set(visibleGraphKeys());
    return visibleTableKeys().every((key) => {
      const row = doc.querySelector(`tr[data-node-key="${key}"]`);
      const actualBlockers = row.querySelector(".blockers-cell").textContent;
      const actualDependents = row.querySelector(".dependents-cell").textContent;
      const expectedBlockers = relationshipText(key, "blocked", visible);
      const expectedDependents = relationshipText(key, "blocker", visible);
      return actualBlockers === expectedBlockers && actualDependents === expectedDependents;
    });
  };
  return {
    doc,
    data,
    canonicalArtifact: JSON.stringify(data),
    visibleGraphKeys,
    tableMatchesGraph: () => JSON.stringify(visibleGraphKeys())
      === JSON.stringify(visibleTableKeys())
      && tableRelationshipsMatchGraph(),
    select: (id, value) => {
      const control = doc.querySelector(id);
      control.value = value;
      control.dispatchEvent(new Event("change", { bubbles: true }));
    },
    clear: () => doc.querySelector("#clear-view").click()
  };
}

function exerciseSearchAndSelection(harness) {
  const { doc, data } = harness;
  const search = doc.querySelector("#graph-search");
  const labelsInitiallyHidden = [...doc.querySelectorAll(".node-label")]
    .every((label) => doc.defaultView.getComputedStyle(label).opacity === "0");
  search.value = "Dependent";
  search.dispatchEvent(new Event("input", { bubbles: true }));
  const visibleRows = [...doc.querySelectorAll("tbody tr:not([hidden])")];

  const graphNode = doc.querySelector('[data-node-key="acme/widgets#2"]');
  graphNode.dispatchEvent(new MouseEvent("click", { bubbles: true }));
  const selectedLabels = doc.querySelectorAll(".graph-node.selected .node-label");
  const panel = doc.querySelector("#issue-details");
  const panelText = panel.textContent;
  const canonicalLink = panel.querySelector("a");
  const sourceNode = data.nodes.find((node) => node.key === "acme/widgets#2");
  const cycleNode = data.nodes.find((node) => node.key === "acme/widgets#4");
  const cycleElement = doc.querySelector('#graph-nodes [data-node-key="acme/widgets#4"]');
  const usesSccPosition = cycleNode.position.layer === null
    && cycleElement.classList.contains("unresolved")
    && cycleElement.dataset.sourceX === String(cycleNode.position.x)
    && cycleElement.dataset.sourceY === String(cycleNode.position.y)
    && [...doc.querySelectorAll(".layer-label")]
      .some((label) => label.textContent === "Unresolved / SCC");

  search.value = "#4";
  search.dispatchEvent(new Event("input", { bubbles: true }));
  const numberRows = [...doc.querySelectorAll("tbody tr:not([hidden])")];
  search.value = "";
  search.dispatchEvent(new Event("input", { bubbles: true }));

  return {
    title_search: visibleRows.length === 1
      && visibleRows[0].dataset.nodeKey === "acme/widgets#2",
    number_search: numberRows.length === 1
      && numberRows[0].dataset.nodeKey === "acme/widgets#4",
    side_panel: panelText.includes("acme/widgets#1")
      && panelText.includes("partners/platform#42"),
    canonical_link: canonicalLink.href === "https://github.com/acme/widgets/issues/2",
    precomputed_position: graphNode.dataset.sourceX === String(sourceNode.position.x)
      && graphNode.dataset.sourceY === String(sourceNode.position.y),
    scc_position: usesSccPosition,
    labels_hidden_by_default: labelsInitiallyHidden,
    selected_label_only: selectedLabels.length === 1
  };
}

function exerciseFilters(harness) {
  const { select, visibleGraphKeys, tableMatchesGraph } = harness;
  select("#readiness-filter", "value:ready");
  const readiness = visibleGraphKeys().join(",") === "acme/widgets#1,acme/widgets#6" && tableMatchesGraph();
  select("#readiness-filter", "sentinel:all");
  select("#state-filter", "value:closed");
  const state = visibleGraphKeys().join(",") === "acme/widgets#3" && tableMatchesGraph();
  select("#state-filter", "sentinel:all");
  select("#priority-filter", "value:p0");
  const priority = visibleGraphKeys().join(",") === "acme/widgets#2" && tableMatchesGraph();
  select("#priority-filter", "value:conflict");
  const priorityConflict = visibleGraphKeys().join(",") === "acme/widgets#5"
    && tableMatchesGraph();
  select("#priority-filter", "sentinel:all");
  select("#area-filter", "value:area:core");
  const area = visibleGraphKeys().join(",")
    === "acme/widgets#1,acme/widgets#4,acme/widgets#5"
    && tableMatchesGraph();
  select("#area-filter", "sentinel:all");
  select("#assignee-filter", "value:alice");
  const assignee = visibleGraphKeys().join(",")
    === "acme/widgets#2,acme/widgets#4"
    && tableMatchesGraph();
  select("#area-filter", "value:area:core");
  const composed = visibleGraphKeys().join(",") === "acme/widgets#4" && tableMatchesGraph();
  select("#area-filter", "sentinel:all");
  select("#assignee-filter", "value:unassigned");
  const assigneeNamedUnassigned = visibleGraphKeys().join(",") === "acme/widgets#5"
    && tableMatchesGraph();
  select("#assignee-filter", "value:all");
  const assigneeNamedAll = visibleGraphKeys().join(",") === "acme/widgets#5"
    && tableMatchesGraph();
  select("#assignee-filter", "sentinel:unassigned");
  const unassigned = visibleGraphKeys().join(",") === "acme/widgets#1,acme/widgets#3,acme/widgets#6,acme/widgets#7"
    && tableMatchesGraph();
  select("#assignee-filter", "sentinel:all");
  select("#component-filter", "value:component-3");
  const component = visibleGraphKeys().join(",")
    === "acme/widgets#4,acme/widgets#5"
    && tableMatchesGraph();
  select("#component-filter", "sentinel:all");

  return {
    readiness_filter: readiness,
    state_filter: state,
    priority_filter: priority,
    priority_conflict_filter: priorityConflict,
    area_filter: area,
    assignee_filter: assignee,
    assignee_named_unassigned_filter: assigneeNamedUnassigned,
    assignee_named_all_filter: assigneeNamedAll,
    unassigned_filter: unassigned,
    filters_compose: composed,
    disconnected_component_filter: component
  };
}

function exerciseProjectFilter(harness) {
  const { doc, select, visibleGraphKeys, tableMatchesGraph } = harness;
  const projectFilter = doc.querySelector("#project-filter");
  const optionValues = [...projectFilter.options].map((option) => option.value);
  const distinctSentinel = optionValues.includes("sentinel:all")
    && optionValues.includes("value:all");
  select("#project-filter", "value:all");
  const filtered = visibleGraphKeys().join(",") === "acme/widgets#1" && tableMatchesGraph();
  select("#project-filter", "sentinel:all");
  const restored = visibleGraphKeys().length === harness.data.nodes.length && tableMatchesGraph();
  return {
    project_filter_dom_interface: distinctSentinel && filtered && restored
  };
}

function exerciseIsolation(harness) {
  const { doc, data, canonicalArtifact, select, visibleGraphKeys, tableMatchesGraph } = harness;
  const graphNode = doc.querySelector('#graph-nodes [data-node-key="acme/widgets#2"]');
  const sourceNode = data.nodes.find((node) => node.key === "acme/widgets#2");
  select("#root-node", "acme/widgets#1");
  doc.querySelector("#root-depth").value = "1";
  doc.querySelector("#isolate-root").click();
  const isolated = visibleGraphKeys().join(",") === "acme/widgets#1,acme/widgets#2,acme/widgets#7"
    && tableMatchesGraph()
    && JSON.stringify(data) === canonicalArtifact
    && graphNode.dataset.sourceX === String(sourceNode.position.x)
    && graphNode.dataset.sourceY === String(sourceNode.position.y);
  harness.clear();
  const restored = visibleGraphKeys().length === data.nodes.length && tableMatchesGraph();
  return {
    root_depth_isolation: isolated,
    clear_restores_canonical_graph: restored
  };
}

function exerciseRelationships(harness) {
  const { doc, select } = harness;
  select("#root-node", "acme/widgets#2");
  doc.querySelector("#highlight-upstream").click();
  const upstreamKeys = [...doc.querySelectorAll(".graph-node.relationship-upstream")]
    .map((element) => element.dataset.nodeKey)
    .sort()
    .join(",");
  const upstream = upstreamKeys === "acme/widgets#1,partners/platform#42"
    && doc.querySelector('tr[data-node-key="acme/widgets#1"] .relationship-cell')
      .textContent === "upstream blocker";

  select("#root-node", "acme/widgets#1");
  doc.querySelector("#highlight-downstream").click();
  const downstream = doc.querySelector('.graph-node[data-node-key="acme/widgets#2"]')
    .classList.contains("relationship-downstream")
    && doc.querySelector('tr[data-node-key="acme/widgets#2"] .relationship-cell')
      .textContent === "downstream dependent";

  select("#path-target", "acme/widgets#2");
  select("#root-node", "acme/widgets#1");
  doc.querySelector("#highlight-path").click();
  const path = doc.querySelectorAll(".graph-node.relationship-path").length === 2
    && doc.querySelectorAll(".graph-edge.relationship-path").length === 1
    && [...doc.querySelectorAll("tbody .relationship-cell")]
      .filter((cell) => cell.textContent === "selected path").length === 2;
  return {
    upstream_highlight: upstream,
    downstream_highlight: downstream,
    selected_path_highlight: path
  };
}

function exerciseKeyboardAndZoom(harness) {
  const { doc } = harness;
  const firstRowButton = doc.querySelector('tr[data-node-key="acme/widgets#1"] button');
  firstRowButton.focus();
  firstRowButton.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }));
  const keyboardMoved = doc.activeElement.dataset.nodeKey === "acme/widgets#2";
  doc.activeElement.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
  const keyboardSelected = doc.querySelector("#issue-details").dataset.selectedKey
    === "acme/widgets#2";
  doc.querySelector("#zoom-in").click();
  return {
    keyboard_navigation: keyboardMoved && keyboardSelected,
    zoom: Number(doc.querySelector("#graph-canvas").dataset.zoom) > 1
  };
}

function exerciseHostileText(harness) {
  const { doc } = harness;
  doc.querySelector('#graph-nodes [data-node-key="acme/widgets#1"]')
    .dispatchEvent(new MouseEvent("click", { bubbles: true }));
  return {
    hostile_text_is_literal: doc.querySelector("#detail-heading").textContent
      .includes("<script>alert(1)</script>")
      && doc.querySelector("#issue-details").textContent
        .includes("area:<img src=x onerror=alert(2)>"),
    no_injected_elements: doc.querySelectorAll("script").length === 5
      && doc.querySelectorAll("img").length === 0
  };
}

function exerciseWhenReady() {
  const mainReady = frame.contentDocument?.documentElement?.dataset.gritTimeToInteractiveMs;
  const constrainedReady = constrainedFrame.contentDocument
    ?.documentElement?.dataset.gritTimeToInteractiveMs;
  const projectReady = projectFrame.contentDocument
    ?.documentElement?.dataset.gritTimeToInteractiveMs;
  if (mainReady && constrainedReady && projectReady) {
    setTimeout(exercise, 0);
    return;
  }
  setTimeout(exerciseWhenReady, 10);
}

exerciseWhenReady();

function readGraph(doc) {
  const artifact = JSON.parse(doc.querySelector("#graph-data").textContent);
  return { ...artifact, nodes: artifact.nodes.map((node) => ({ ...node, ...node.common })) };
}

function exerciseRecommendation({ doc, data }) {
    const recommendation = data.analysis.next.recommendation;
    const recommendationText = doc.querySelector("#recommendation-status").textContent;
    const evidenceText = doc.querySelector("#recommendation-evidence").textContent;
    const summaryMatches = recommendation.first_issue.number === 1
      && recommendationText.includes("#1")
      && data.analysis.next.comparison_to_runner_up.message.length > 0
      && evidenceText.includes(`Reason${data.analysis.next.comparison_to_runner_up.message}`)
      && evidenceText.includes("SearchComplete")
      && evidenceText.includes("Parallel now#1, #6");
    const distinctRunnerUp = data.analysis.next.comparison_to_runner_up.runner_up.number === 6
      && evidenceText.includes("Runner-up#6 Runner-up");
    const operationalDiagnostics = evidenceText.includes("Unresolved3")
      && evidenceText.includes("Cycles2")
      && evidenceText.includes("Unknown External blockers1");

    doc.querySelector("#recommendation-select").click();
    const causalPath = doc.querySelector('[data-node-key="acme/widgets#1"]')
      .classList.contains("causal-path")
      && doc.querySelector('[data-node-key="acme/widgets#7"]')
        .classList.contains("unlocked-outcome")
      && doc.querySelector('.graph-edge[data-blocker="acme/widgets#1"][data-blocked="acme/widgets#7"]')
        .classList.contains("causal-path")
      && doc.querySelectorAll("tbody tr.causal-evidence").length >= 2;

    const sizeControl = doc.querySelector("#node-size-metric");
    sizeControl.value = "unlock_count";
    sizeControl.dispatchEvent(new Event("change", { bubbles: true }));
    const recommendationNode = doc.querySelector('[data-node-key="acme/widgets#1"]');
    const unlockSizeControl = recommendationNode.dataset.sizeMetric === "unlock_count"
      && recommendationNode.dataset.sizeValue === String(
        data.nodes.find((node) => node.number === 1).unlock_count
      );
    sizeControl.value = "pagerank_bucket";
    sizeControl.dispatchEvent(new Event("change", { bubbles: true }));
    const pagerankSizeControl = recommendationNode.dataset.sizeMetric === "pagerank_bucket"
      && recommendationNode.dataset.sizeValue === String(
        data.nodes.find((node) => node.number === 1).pagerank_bucket
      );

    const colorControl = doc.querySelector("#node-color-metric");
    colorControl.value = "priority";
    colorControl.dispatchEvent(new Event("change", { bubbles: true }));
    const priorityColorControl = recommendationNode.classList.contains("color-priority-p1");

  return {
    recommendation_summary: summaryMatches,
    distinct_runner_up: distinctRunnerUp,
    operational_diagnostics: operationalDiagnostics,
    causal_path: causalPath,
    unlock_size_control: unlockSizeControl,
    pagerank_size_control: pagerankSizeControl,
    priority_color_control: priorityColorControl
  };
}


function exerciseOutcomes({ doc, data }) {
  const before = JSON.stringify(data.analysis);
  const colorControl = doc.querySelector("#node-color-metric");
  const closedCircle = doc.querySelector('.graph-node[data-node-key="acme/widgets#3"] circle');
  colorControl.value = "priority";
  colorControl.dispatchEvent(new Event("change", { bubbles: true }));
  const closedPriority = doc.defaultView.getComputedStyle(closedCircle).stroke === "rgb(152, 162, 179)"
    && doc.querySelector(".graph-legend").textContent.includes("P0");
  colorControl.value = "state";
  colorControl.dispatchEvent(new Event("change", { bubbles: true }));
  const closedState = doc.defaultView.getComputedStyle(closedCircle).stroke === "rgb(102, 112, 133)";
  colorControl.value = "readiness";
  colorControl.dispatchEvent(new Event("change", { bubbles: true }));
  const visibleKeys = () => [...doc.querySelectorAll("tbody tr:not([hidden])")]
    .map((row) => row.dataset.nodeKey).sort();
  const completed = data.nodes.filter((node) => node.kind === "issue" && node.resolution === "completed");
  const summaryVisible = doc.querySelector("#count-completed").textContent === String(completed.length)
    && doc.querySelector("#completion-list").textContent.includes("Historical")
    && !doc.querySelector("#completion-empty").hidden === (completed.length === 0);
  doc.querySelector('[data-work-view="completed"]').click();
  const filtered = visibleKeys().join(",") === "acme/widgets#3"
    && doc.querySelector('[data-work-view="completed"]').getAttribute("aria-pressed") === "true";
  doc.querySelector('tbody tr[data-node-key="acme/widgets#3"] button').click();
  const inspected = doc.querySelector("#issue-details").textContent.includes("Completed");
  const historyNode = doc.querySelector('.graph-node[data-node-key="acme/widgets#3"]');
  const historyReadable = !historyNode.classList.contains("unresolved")
    && historyNode.querySelector(".node-summary").textContent.includes("#3")
    && historyNode.querySelector("circle").getBoundingClientRect().width >= 15
    && doc.querySelector("#issue-details").textContent.includes("History (not operational)");
  const constrainedDoc = constrainedFrame.contentDocument;
  constrainedDoc.querySelector('[data-work-view="completed"]').click();
  const constrainedCompleted = [...constrainedDoc.querySelectorAll('.graph-node:not([hidden])')]
    .some((node) => node.dataset.nodeKey === "acme/widgets#3")
    && Number(constrainedDoc.documentElement.dataset.gritRenderedNodes) <= 500;
  constrainedDoc.querySelector("#clear-view").click();
  doc.querySelector('[data-work-view="not_planned"]').click();
  const empty = visibleKeys().length === 0 && !doc.querySelector("#graph-empty").hidden;
  doc.querySelector("#recommendation-select").click();
  const resumed = visibleKeys().length === data.nodes.length
    && doc.querySelector("#issue-details").dataset.selectedKey === "acme/widgets#1"
    && JSON.stringify(readGraph(doc).analysis) === before;
  doc.querySelector("#clear-view").click();
  return {
    completed_summary_visible: summaryVisible,
    closed_nodes_honor_visual_encoding: closedPriority && closedState,
    history_map_is_readable_and_not_unresolved: historyReadable,
    constrained_completed_window: constrainedCompleted,
    completed_filter_and_details: filtered && inspected,
    closed_empty_state: empty,
    completion_to_recommendation_preserves_analysis: resumed
  };
}
