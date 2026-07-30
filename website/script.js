const codeByLanguage = {
  ts: `import { python } from "poly";

const answer = await python.call(
  "./math.py",
  "add",
  [20, 22],
);

console.log(answer); // 42`,
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

tabs.forEach((tab) => {
  tab.addEventListener("click", () => selectTab(tab.dataset.tab));
});

copyButton?.addEventListener("click", async () => {
  const language = document.querySelector(".tab.is-active")?.dataset.tab ?? "ts";
  await navigator.clipboard.writeText(codeByLanguage[language]);
  copyButton.textContent = "Copied";
  window.setTimeout(() => {
    copyButton.textContent = "Copy";
  }, 1400);
});

const buildCommands = `git clone https://github.com/liooil/poly
cd poly
pwsh ./scripts/bootstrap-bun.ps1`;

const copyInstall = document.querySelector(".copy-install");
copyInstall?.addEventListener("click", async () => {
  await navigator.clipboard.writeText(buildCommands);
  copyInstall.textContent = "Copied";
  window.setTimeout(() => {
    copyInstall.textContent = "Copy commands";
  }, 1400);
});
