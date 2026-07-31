const codeByLanguage = {
  ts: `// target API
import { add } from "./math.py"
  with { type: "python" };

console.log(add(20, 22)); // 42`,
  py: `def add(left, right):
    return left + right`,
};

const tabs = [...document.querySelectorAll(".tab")];
const panels = [...document.querySelectorAll(".code-panel")];
const copyButton = document.querySelector(".copy-button");

function selectTab(language) {
  tabs.forEach((tab) => {
    const active = tab.dataset.tab === language;
    tab.classList.toggle("is-active", active);
    tab.setAttribute("aria-selected", String(active));
  });

  panels.forEach((panel) => {
    panel.classList.toggle("is-active", panel.dataset.panel === language);
  });
}

function showCopiedState(button) {
  const idleLabel = button.dataset.idleLabel ?? button.textContent;
  button.textContent = button.dataset.copiedLabel ?? "Copied";
  window.setTimeout(() => {
    button.textContent = idleLabel;
  }, 1400);
}

tabs.forEach((tab) => {
  tab.addEventListener("click", () => selectTab(tab.dataset.tab));
});

copyButton?.addEventListener("click", async () => {
  const language = document.querySelector(".tab.is-active")?.dataset.tab ?? "ts";
  await navigator.clipboard.writeText(codeByLanguage[language]);
  showCopiedState(copyButton);
});

const buildCommands = `git clone https://github.com/liooil/poly
cd poly
pwsh ./scripts/bootstrap-bun.ps1 -Configuration Release`;

const copyInstall = document.querySelector(".copy-install");
copyInstall?.addEventListener("click", async () => {
  await navigator.clipboard.writeText(buildCommands);
  showCopiedState(copyInstall);
});
