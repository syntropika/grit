const frame = document.querySelector("#app");
const result = document.querySelector("#result");

function exercise() {
  try {
    const doc = frame.contentDocument;
    const graph = JSON.parse(doc.querySelector("#graph-data").textContent);
    const draft = graph.nodes.find(node => node.common.temporary_id && !node.common.number);
    const key = draft.common.key;
    const node = () => doc.querySelector(`.graph-node[data-node-key="${key}"]`);
    const rows = () => [...doc.querySelectorAll("tbody tr:not([hidden])")].map(row => row.dataset.nodeKey);
    const select = (id, value) => {
      const control = doc.querySelector(id);
      control.value = value;
      control.dispatchEvent(new Event("change", { bubbles: true }));
    };
    const search = value => {
      const control = doc.querySelector("#graph-search");
      control.value = value;
      control.dispatchEvent(new Event("input", { bubbles: true }));
    };
    const clear = () => doc.querySelector("#clear-view").click();
    const checks = {};
    checks.draft_identity_is_lossless = key.includes("#draft:") && !draft.common.number && draft.url === "";
    checks.constrained_table_keeps_pending_draft = rows().includes(key) && !node();
    checks.recommendation_names_draft = doc.querySelector("#recommendation-status").textContent.includes(key)
      && !doc.querySelector("#recommendation-status").textContent.includes("undefined");

    search(key);
    checks.draft_key_is_searchable = rows().length === 1 && rows()[0] === key;
    doc.querySelector(`tr[data-node-key="${key}"] button`).click();
    checks.pending_title_and_no_github_link = doc.querySelector("#detail-heading").textContent === "Edited local Draft"
      && doc.querySelector("#issue-details .detail-key").textContent === key
      && !doc.querySelector("#issue-details a[href^='https://github.com/']");
    search("Edited local Draft");
    checks.edited_title_is_searchable = rows().length === 1 && rows()[0] === key;
    search("");
    select("#priority-filter", "value:p0");
    checks.pending_priority_filter = rows().length === 1 && rows()[0] === key;
    clear();
    select("#root-node", key);
    select("#root-depth", "1");
    doc.querySelector("#isolate-root").click();
    checks.pending_dependency_isolation = rows().sort().join(",") === [key, "acme/widgets#2"].sort().join(",");
    clear();
    select("#node-size-metric", "pagerank_bucket");
    select("#node-color-metric", "priority");
    doc.querySelector("#recommendation-select").click();
    checks.recommendation_opens_draft_and_keeps_metrics = node()?.classList.contains("causal-path")
      && node().classList.contains("color-priority-p0") && node().dataset.sizeMetric === "pagerank_bucket";
    doc.querySelector("#show-full-network").click();
    checks.expansion_keeps_causal_evidence = node()?.classList.contains("causal-path")
      && doc.querySelector('.graph-node[data-node-key="acme/widgets#2"]').classList.contains("unlocked-outcome")
      && doc.querySelector(`.graph-edge[data-blocker="${key}"][data-blocked="acme/widgets#2"]`).classList.contains("causal-path");
    clear();
    checks.clear_resets_view = !node() && rows().length === graph.nodes.length
      && doc.querySelectorAll(".causal-path, .unlocked-outcome, .causal-evidence").length === 0;
    checks.embedded_working_input_unchanged = JSON.stringify(graph) === doc.querySelector("#graph-data").textContent;
    result.textContent = JSON.stringify({ checks });
  } catch (error) {
    result.textContent = JSON.stringify({ error: String(error), stack: error.stack });
  }
}

function waitForApp() {
  if (frame.contentDocument?.documentElement?.dataset.gritTimeToInteractiveMs !== undefined) exercise();
  else setTimeout(waitForApp, 10);
}
waitForApp();
