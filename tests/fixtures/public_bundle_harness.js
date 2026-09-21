const frame = document.querySelector("#app");
const result = document.querySelector("#result");

const exercise = () => {
  try {
    const doc = frame.contentDocument;
    const resources = [
      ...performance.getEntriesByType("resource"),
      ...frame.contentWindow.performance.getEntriesByType("resource")
    ];
    result.textContent = JSON.stringify({
      hostile_text_is_literal: doc.body.textContent.includes(
        "Root <script>alert('title')</script>"
      ) && doc.body.textContent.includes("area:backend<img src=x onerror=alert(2)>"),
      no_injected_elements: doc.querySelectorAll("script, img").length === 0,
      resource_urls_are_local: resources.every((entry) => entry.name.startsWith("file://"))
    });
  } catch (error) {
    result.textContent = JSON.stringify({ error: String(error), stack: error.stack });
  }
};

if (frame.contentDocument && frame.contentDocument.querySelector("table")) {
  setTimeout(exercise, 0);
} else {
  frame.addEventListener("load", exercise, { once: true });
}
