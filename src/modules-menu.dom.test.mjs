// Lesson 985 follow-up: DOM-level regression test for terminal modules
// dropdown click flow.
//
// Background: Lesson 980 verified the click handler structure by reading
// source. Lesson 985 taught us static analysis isn't enough — runtime
// issues can hide in capture-phase handlers, sync throws in
// openModuleModal, etc. This test boots a JSDOM, sets up the globals
// that modules-menu.js expects, imports it, and clicks through the
// "trigger → dropdown → item → run modal" flow exactly as it happens
// in the Terminal page. If the run modal does NOT appear, that's
// David Bug 4 ("nothing happens, you are back to the terminal chat").
//
// Run via: `node --test src/modules-menu.dom.test.mjs`
import { test } from "node:test";
import assert from "node:assert/strict";
import { JSDOM } from "jsdom";

const FAKE_MODULES = [
  {
    module_id: "firecrawl",
    name: "Firecrawl",
    icon: "🔥",
    client_manifest: {
      name: "firecrawl",
      version: "1.0",
      description: "Scraper",
      tools: [
        {
          name: "scrape",
          description: "Scrape a URL",
          form_schema: {
            type: "object",
            title: "Scrape",
            properties: {
              url: { type: "string", title: "URL", minLength: 1 },
            },
            required: ["url"],
          },
        },
      ],
    },
  },
  {
    module_id: "diff",
    name: "Diff",
    icon: "🔁",
    client_manifest: {
      name: "diff",
      version: "0.2",
      description: "Diff",
      tools: [
        {
          name: "diff_text",
          description: "Diff two texts",
          form_schema: {
            type: "object",
            title: "Diff",
            properties: {
              a: { type: "string", title: "A" },
              b: { type: "string", title: "B" },
            },
            required: ["a", "b"],
          },
        },
      ],
    },
  },
];

function setupJsdom() {
  const dom = new JSDOM('<!DOCTYPE html><html><body><button id="trigger">Modules</button></body></html>', {
    pretendToBeVisual: true,
  });
  const win = dom.window;
  global.window = win;
  global.document = win.document;
  global.HTMLElement = win.HTMLElement;
  global.HTMLInputElement = win.HTMLInputElement;
  global.HTMLTextAreaElement = win.HTMLTextAreaElement;
  global.HTMLFormElement = win.HTMLFormElement;
  global.Node = win.Node;
  global.Element = win.Element;
  global.Event = win.Event;
  global.CSS = win.CSS;
  global.getComputedStyle = win.getComputedStyle;
  return win;
}

test("terminal: clicking a module in the dropdown opens the run modal (Lesson 985 runtime)", async () => {
  const win = setupJsdom();
  const errors = [];
  win.addEventListener("error", (ev) => errors.push({ where: "win.error", message: ev.message, stack: ev.error?.stack }));
  // Modern Node: listen on process for unhandled rejections
  const origUnhandled = process.listeners("unhandledRejection");
  const onRej = (reason) => errors.push({ where: "unhandledRejection", message: String(reason?.message ?? reason), stack: reason?.stack });
  process.on("unhandledRejection", onRej);

  // Mock invoke before importing modules-menu
  let listCallCount = 0;
  let runCallCount = 0;
  const invokeImpl = {
    invoke: async (cmd, args) => {
      if (cmd === "mc_list_installed_modules") {
        listCallCount++;
        return { modules: FAKE_MODULES };
      }
      if (cmd === "mc_run_module") {
        runCallCount++;
        return { outputs: "scraped http://example.com — OK", duration_ms: 42, run_id: "r-test-1" };
      }
      return null;
    },
  };
  // Tauri 2 reads invoke from window.__TAURI_INTERNALS__.invoke
  win.__TAURI_INTERNALS__ = invokeImpl;
  // Legacy fallback
  win.__TAURI__ = { core: invokeImpl, invoke: invokeImpl.invoke };

  // Dynamic import AFTER globals are set
  const url = new URL("./modules-menu.js", import.meta.url).href;
  const menuModule = await import(url);

  // Trigger to open dropdown
  const trigger = document.getElementById("trigger");
  menuModule.mountModulesMenu(trigger, { surface: "terminal" });
  trigger.click();
  await new Promise((r) => setTimeout(r, 80));

  const menu = document.querySelector(".mc-modules-menu");
  assert.ok(menu, "Dropdown menu should appear after clicking Modules button");
  const items = menu.querySelectorAll("[data-module-id]");
  assert.equal(items.length, 2, `Expected 2 module items, got ${items.length}`);

  // Click first module
  const firstItem = items[0];
  assert.equal(firstItem.dataset.moduleId, "firecrawl");
  firstItem.click();
  await new Promise((r) => setTimeout(r, 100));

  const modal = document.querySelector(".mc-modal");
  const menuStill = document.querySelector(".mc-modules-menu");
  assert.equal(menuStill, null, "Menu should be removed after selection");

  // KEY ASSERTION: modal must exist. If not, that's Lesson 985 Bug 4.
  assert.ok(
    modal,
    `Expected run modal to open after clicking a module in the dropdown. ` +
      `This is the Lesson 985 Terminal "nothing happens" bug. ` +
      `Errors during the flow: ${JSON.stringify(errors, null, 2)}`
  );

  // Verify modal has form with correct data attrs
  const form = modal.querySelector("form.mc-modal-form");
  assert.ok(form, "Modal should contain a form");
  assert.equal(form.dataset.module, "firecrawl");
  assert.equal(form.dataset.tool, "scrape");

  // Verify the form actually has the URL input rendered (form_schema → input)
  const urlInput = modal.querySelector("input[name='url']");
  assert.ok(urlInput, "Modal should render a URL input from form_schema. Modal HTML: " + modal.outerHTML.slice(0, 1000));

  // Inspect the form_schema path that the launchpad uses:
  // the Lesson 980 fallback builds a fake object when
  // getInstalledModulesMerged didn't expose _raw.
  console.error("DEBUG: form rendered with fields, fields count is whatever the parser produced from", JSON.stringify(FAKE_MODULES[0].client_manifest.tools[0].form_schema));

  // No errors during the click flow
  assert.equal(errors.length, 0, "Unexpected errors during click flow: " + JSON.stringify(errors));

  process.off("unhandledRejection", onRej);
});

test("terminal: clicking a module with NO tools shows info modal (not silently nothing)", async () => {
  const win = setupJsdom();
  const errors = [];
  win.addEventListener("error", (ev) => errors.push({ where: "win.error", message: ev.message, stack: ev.error?.stack }));
  process.on("unhandledRejection", (reason) => errors.push({ where: "unhandledRejection", message: String(reason?.message ?? reason), stack: reason?.stack }));

  // Module WITHOUT tools
  const FAKE_NO_TOOLS_MODULES = [
    {
      module_id: "broken_mod",
      name: "Broken Mod",
      description: "Has no client_manifest tools (Lesson 980 fallback path)",
      client_manifest: { name: "broken_mod", version: "0.1", tools: [] },
    },
  ];

  const invokeImpl = {
    invoke: async (cmd) => {
      if (cmd === "mc_list_installed_modules") return { modules: FAKE_NO_TOOLS_MODULES };
      return null;
    },
  };
  win.__TAURI_INTERNALS__ = invokeImpl;

  const url = new URL("./modules-menu.js", import.meta.url).href;
  const menuModule = await import(url + "?nopetools=" + Date.now());

  const trigger = document.createElement("button");
  trigger.id = "trigger2";
  document.body.appendChild(trigger);
  menuModule.mountModulesMenu(trigger, { surface: "terminal" });
  trigger.click();
  await new Promise((r) => setTimeout(r, 80));

  const items = document.querySelectorAll(".mc-modules-menu [data-module-id]");
  assert.ok(items.length === 1, `Expected 1 module item, got ${items.length}`);
  items[0].click();
  await new Promise((r) => setTimeout(r, 80));

  // Should get info modal
  const modal = document.querySelector(".mc-modal");
  assert.ok(modal, "Info modal should open for module with no tools. Errors: " + JSON.stringify(errors));
  const header = modal.querySelector("h3");
  assert.ok(header && header.textContent.includes("Broken Mod"), "Info modal should show module name. Got: " + (header?.textContent));
});
