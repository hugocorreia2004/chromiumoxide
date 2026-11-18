// Extracted from playwright-core 1.65.1 with the patchright-core 1.64.3 patches merged in (lib/coreBundle.js, packages/playwright-core/src/generated/bidiInsertTextSource.ts).
// Origin: Playwright (https://github.com/microsoft/playwright), as patched by Patchright (https://github.com/Kaliiiiiiiiii-Vinyzu/patchright).
// Copyright (c) Microsoft Corporation and the Patchright contributors.
// Licensed under the Apache License, Version 2.0 (Apache-2.0); see oxidewright/NOTICE.
// Do not edit: regenerate with `cargo run -p oxidewright-xtask -- merge-patchright <base> <patched> <latest>`.

var __commonJS = obj => {
  let required = false;
  let result;
  return function __require() {
    if (!required) {
      required = true;
      let fn;
      for (const name in obj) { fn = obj[name]; break; }
      const module = { exports: {} };
      fn(module.exports, module);
      result = module.exports;
    }
    return result;
  }
};
var __export = (target, all) => {for (var name in all) target[name] = all[name];};
var __toESM = mod => ({ ...mod, 'default': mod });
var __toCommonJS = mod => ({ ...mod, __esModule: true });


// packages/injected/src/bidiInsertText.ts
var bidiInsertText_exports = {};
__export(bidiInsertText_exports, {
  BidiInsertTextInstaller: () => BidiInsertTextInstaller,
  default: () => bidiInsertText_default
});
module.exports = __toCommonJS(bidiInsertText_exports);
function bidiInsertText(window, text) {
  let element = window.document.activeElement;
  while (element == null ? void 0 : element.shadowRoot)
    element = element.shadowRoot.activeElement;
  if (!element)
    return;
  const elementType = element.nodeName.toLocaleLowerCase();
  if (elementType === "iframe" || elementType === "frame") {
    return element;
  } else if (elementType === "input" || elementType === "textarea") {
    const inputElement = element;
    const start = inputElement.selectionStart;
    if (start === null) {
      inputElement.value += text;
    } else {
      let value = inputElement.value;
      value = value.substring(0, start) + text + value.substring(inputElement.selectionEnd);
      inputElement.value = value;
      const caretPosition = start + text.length;
      inputElement.setSelectionRange(caretPosition, caretPosition);
    }
    inputElement.dispatchEvent(new InputEvent("input", { data: text, bubbles: true, composed: true }));
  } else if (element instanceof HTMLElement && element.isContentEditable) {
    const selection = window.getSelection();
    let range;
    if (selection.rangeCount)
      range = selection.getRangeAt(0);
    if (!range || !element.contains(range.commonAncestorContainer)) {
      range = window.document.createRange();
      range.selectNodeContents(element);
      range.collapse(true);
    }
    range.deleteContents();
    const lines = text.split("\n");
    for (let i = lines.length - 1; i >= 0; i--) {
      range.insertNode(window.document.createTextNode(lines[i]));
      if (i > 0)
        range.insertNode(window.document.createElement("br"));
    }
    range.collapse();
    selection.removeAllRanges();
    selection.addRange(range);
    element.dispatchEvent(new InputEvent("input", { data: text, bubbles: true, composed: true }));
  }
}
var BidiInsertTextInstaller = class {
  constructor(injectedScript) {
    const window = injectedScript.window;
    window.__pw_bidiInsertText = (text) => bidiInsertText(window, text);
  }
};
var bidiInsertText_default = BidiInsertTextInstaller;
