const test = require("node:test");
const assert = require("node:assert/strict");

require("../src/graph/render/network-view.js");

const graph = {
  nodes: ["#1", "#2", "#3", "#4"].map((key) => ({ key })),
  edges: [
    { blocker: "#1", blocked: "#2" },
    { blocker: "#2", blocked: "#3" }
  ]
};

test("builds every relationship index in one network model", () => {
  const view = createConstrainedView();

  assert.deepEqual(view.blockers("#2"), ["#1"]);
  assert.deepEqual(view.dependents("#2"), ["#3"]);
  assert.equal(view.node("#4").key, "#4");
  assert.throws(() => view.blockers("#2").push("corruption"), TypeError);
  assert.deepEqual(view.blockers("#2"), ["#1"]);
});

test("owns reset, full, and bounded-neighborhood window state", () => {
  const view = createConstrainedView();
  assert.deepEqual([...view.current()], ["#1"]);

  const neighborhood = view.showNeighborhood("#2");
  assert.deepEqual([...neighborhood].sort(), ["#1", "#2"]);
  assert.equal(view.contains("#2"), true);
  neighborhood.add("unowned");
  assert.equal(view.contains("unowned"), false);

  assert.deepEqual([...view.reset()], ["#1"]);
  assert.deepEqual([...view.showFull()], ["#1", "#2", "#3", "#4"]);
  assert.deepEqual([...view.showNeighborhood("#4")], ["#4"]);
});

test("uses the complete network as the initial full-mode window", () => {
  const view = globalThis.GritNetworkView.create(graph, {
    mode: "full",
    initial_network_node_limit: 1,
    initial_node_keys: []
  });

  assert.deepEqual([...view.current()], ["#1", "#2", "#3", "#4"]);
});

function createConstrainedView() {
  return globalThis.GritNetworkView.create(graph, {
    mode: "constrained",
    initial_network_node_limit: 2,
    initial_node_keys: ["#1"]
  });
}
