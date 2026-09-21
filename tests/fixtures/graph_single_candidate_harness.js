const frame = document.querySelector("#app");
const result = document.querySelector("#result");

const exercise = () => {
  try {
    const doc = frame.contentDocument;
    const data = JSON.parse(doc.querySelector("#graph-data").textContent);
    const reason = data.analysis.next.recommendation.reasons
      .find((entry) => entry.reason.code === "only_executable_candidate");
    const evidence = doc.querySelector("#recommendation-evidence").textContent;
    result.textContent = JSON.stringify({
      canonical_reason: reason.message === "it is the only executable candidate"
        && evidence.includes(`Reason${reason.message}`)
    });
  } catch (error) {
    result.textContent = JSON.stringify({ error: String(error) });
  }
};

frame.addEventListener("load", () => setTimeout(exercise, 0));
setTimeout(() => {
  if (result.textContent === "pending") exercise();
}, 1500);
