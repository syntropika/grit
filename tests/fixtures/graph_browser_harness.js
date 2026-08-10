const frame = document.querySelector("#app");
const result = document.querySelector("#result");

const exercise = () => {
  try {
    const doc = frame.contentDocument;
    const data = JSON.parse(doc.querySelector("#graph-data").textContent);
    const search = doc.querySelector("#graph-search");
    search.value = "Dependent";
    search.dispatchEvent(new Event("input", { bubbles: true }));
    const visibleRows = [...doc.querySelectorAll("tbody tr:not([hidden])")];
    const labelsInitiallyHidden = [...doc.querySelectorAll(".node-label")]
      .every((label) => doc.defaultView.getComputedStyle(label).opacity === "0");

    const graphNode = doc.querySelector('[data-node-key="acme/widgets#2"]');
    graphNode.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    const selectedLabels = doc.querySelectorAll(".graph-node.selected .node-label");
    const panel = doc.querySelector("#issue-details");
    const panelText = panel.textContent;
    const canonicalLink = panel.querySelector("a");
    const sourceNode = data.nodes.find((node) => node.common.key === "acme/widgets#2");
    const cycleNode = data.nodes.find((node) => node.common.key === "acme/widgets#4");
    const cycleElement = doc.querySelector('#graph-nodes [data-node-key="acme/widgets#4"]');
    const usesSccPosition = cycleNode.common.position.layer === null
      && cycleElement.classList.contains("unresolved")
      && cycleElement.dataset.sourceX === String(cycleNode.common.position.x)
      && cycleElement.dataset.sourceY === String(cycleNode.common.position.y)
      && [...doc.querySelectorAll(".layer-label")]
        .some((label) => label.textContent === "Unresolved / SCC");
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
        data.nodes.find((node) => node.common.number === 1).unlock_count
      );
    sizeControl.value = "pagerank_bucket";
    sizeControl.dispatchEvent(new Event("change", { bubbles: true }));
    const pagerankSizeControl = recommendationNode.dataset.sizeMetric === "pagerank_bucket"
      && recommendationNode.dataset.sizeValue === String(
        data.nodes.find((node) => node.common.number === 1).pagerank_bucket
      );

    const colorControl = doc.querySelector("#node-color-metric");
    colorControl.value = "priority";
    colorControl.dispatchEvent(new Event("change", { bubbles: true }));
    const priorityColorControl = recommendationNode.classList.contains("color-priority-p1");

    search.value = "#4";
    search.dispatchEvent(new Event("input", { bubbles: true }));
    const numberRows = [...doc.querySelectorAll("tbody tr:not([hidden])")];
    search.value = "";
    search.dispatchEvent(new Event("input", { bubbles: true }));
    const firstRowButton = doc.querySelector('tr[data-node-key="acme/widgets#1"] button');
    firstRowButton.focus();
    firstRowButton.dispatchEvent(new KeyboardEvent("keydown", {
      key: "ArrowDown",
      bubbles: true
    }));
    const keyboardMoved = doc.activeElement.dataset.nodeKey === "acme/widgets#2";
    doc.activeElement.dispatchEvent(new KeyboardEvent("keydown", {
      key: "Enter",
      bubbles: true
    }));

    doc.querySelector("#zoom-in").click();
    const zoomed = Number(doc.querySelector("#graph-canvas").dataset.zoom) > 1;

    doc.querySelector('[data-node-key="acme/widgets#1"]')
      .dispatchEvent(new MouseEvent("click", { bubbles: true }));
    const literalTitle = doc.querySelector("#detail-heading").textContent
      .includes("<script>alert(1)</script>");
    const literalLabel = doc.querySelector("#issue-details").textContent
      .includes("area:<img src=x onerror=alert(2)>");

    result.textContent = JSON.stringify({
      checks: {
        title_search: visibleRows.length === 1
          && visibleRows[0].dataset.nodeKey === "acme/widgets#2",
        number_search: numberRows.length === 1
          && numberRows[0].dataset.nodeKey === "acme/widgets#4",
        side_panel: panelText.includes("acme/widgets#1")
          && panelText.includes("partners/platform#42"),
        canonical_link: canonicalLink.href === "https://github.com/acme/widgets/issues/2",
        precomputed_position: graphNode.dataset.sourceX === String(sourceNode.common.position.x)
          && graphNode.dataset.sourceY === String(sourceNode.common.position.y),
        scc_position: usesSccPosition,
        labels_hidden_by_default: labelsInitiallyHidden,
        selected_label_only: selectedLabels.length === 1,
        keyboard_navigation: keyboardMoved
          && doc.querySelector("#issue-details").dataset.selectedKey === "acme/widgets#1",
        zoom: zoomed,
        recommendation_summary: summaryMatches,
        distinct_runner_up: distinctRunnerUp,
        operational_diagnostics: operationalDiagnostics,
        causal_path: causalPath,
        unlock_size_control: unlockSizeControl,
        pagerank_size_control: pagerankSizeControl,
        priority_color_control: priorityColorControl,
        hostile_text_is_literal: literalTitle && literalLabel,
        no_injected_elements: doc.querySelectorAll("script").length === 2
          && doc.querySelectorAll("img").length === 0
      }
    });
  } catch (error) {
    result.textContent = JSON.stringify({ error: String(error), stack: error.stack });
  }
};

if (frame.contentDocument && frame.contentDocument.querySelector("#graph-data")) {
  setTimeout(exercise, 0);
} else {
  frame.addEventListener("load", exercise, { once: true });
}
