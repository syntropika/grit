const frame = document.querySelector("#app");
const constrainedFrame = document.querySelector("#constrained-app");
const result = document.querySelector("#result");

const exercise = () => {
  try {
    const doc = frame.contentDocument;
    const data = JSON.parse(doc.querySelector("#graph-data").textContent);
    const presentation = JSON.parse(
      doc.querySelector("#graph-presentation-data").textContent
    );
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
    const keyboardSelected = doc.querySelector("#issue-details").dataset.selectedKey
      === "acme/widgets#2";

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
        precomputed_position: graphNode.dataset.sourceX === String(sourceNode.position.x)
          && graphNode.dataset.sourceY === String(sourceNode.position.y),
        scc_position: usesSccPosition,
        labels_hidden_by_default: labelsInitiallyHidden,
        full_network_within_validated_range: presentation.mode === "full"
          && doc.querySelector("#network-mode").hidden,
        performance_markers: Number(doc.documentElement.dataset.gritLoadMs) >= 0
          && Number(doc.documentElement.dataset.gritRenderMs) >= 0
          && Number(doc.documentElement.dataset.gritTimeToInteractiveMs)
            >= Number(doc.documentElement.dataset.gritLoadMs),
        selected_label_only: selectedLabels.length === 1,
        keyboard_navigation: keyboardMoved && keyboardSelected,
        zoom: zoomed,
        hostile_text_is_literal: literalTitle && literalLabel,
        ...exerciseConstrainedMode(constrainedFrame.contentDocument),
        no_injected_elements: doc.querySelectorAll("script").length === 4
          && doc.querySelectorAll("img").length === 0
      }
    });
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
    && visibleTableKeys().length === 6;

  search.value = "#5";
  search.dispatchEvent(new Event("input", { bubbles: true }));
  const searchable = visibleTableKeys().join(",") === "acme/widgets#5";
  doc.querySelector('tr[data-node-key="acme/widgets#5"] button').click();
  search.value = "";
  search.dispatchEvent(new Event("input", { bubbles: true }));
  const neighborhood = visibleGraphKeys().join(",") === "acme/widgets#4,acme/widgets#5";
  const node = doc.querySelector('.graph-node[data-node-key="acme/widgets#5"]');
  const data = JSON.parse(doc.querySelector("#graph-data").textContent);
  const source = data.nodes.find((candidate) => candidate.key === "acme/widgets#5");
  const precomputed = node.dataset.sourceX === String(source.position.x)
    && node.dataset.sourceY === String(source.position.y);

  doc.querySelector("#show-initial-network").click();
  const reset = visibleGraphKeys().join(",") === "acme/widgets#1";
  doc.querySelector("#show-full-network").click();
  const expanded = visibleGraphKeys().length === data.nodes.length;
  return {
    constrained_mode_opens_bounded: initial,
    constrained_search_keeps_table: searchable,
    selected_result_opens_neighborhood: neighborhood && precomputed,
    constrained_reset_and_explicit_expand: reset && expanded
  };
}

function exerciseWhenReady() {
  const mainReady = frame.contentDocument?.documentElement.dataset.gritTimeToInteractiveMs;
  const constrainedReady = constrainedFrame.contentDocument
    ?.documentElement.dataset.gritTimeToInteractiveMs;
  if (mainReady && constrainedReady) {
    setTimeout(exercise, 0);
    return;
  }
  setTimeout(exerciseWhenReady, 10);
}

exerciseWhenReady();
