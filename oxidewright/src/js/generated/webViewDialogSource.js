// Extracted from patchright-core 1.64.3 (lib/coreBundle.js, packages/playwright-core/src/generated/webViewDialogSource.ts).
// Origin: Playwright (https://github.com/microsoft/playwright), as patched by Patchright (https://github.com/Kaliiiiiiiiii-Vinyzu/patchright).
// Copyright (c) Microsoft Corporation and the Patchright contributors.
// Licensed under the Apache License, Version 2.0 (Apache-2.0); see oxidewright/NOTICE.
// Do not edit: regenerate with `node oxidewright/scripts/update-injected.mjs <package>`.

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


// packages/injected/src/webview/webViewDialog.ts
var webViewDialog_exports = {};
__export(webViewDialog_exports, {
  installDialogBridge: () => installDialogBridge
});
module.exports = __toCommonJS(webViewDialog_exports);
function toMessage(message) {
  return String(message === void 0 || message === null ? "" : message);
}
function installDialogBridge(window, endpoint) {
  function post(type, message, defaultValue) {
    const xhr = new window.XMLHttpRequest();
    try {
      xhr.open("POST", endpoint, false);
    } catch (e) {
      return null;
    }
    try {
      xhr.send(JSON.stringify({ type, message, defaultValue }));
    } catch (e) {
      return null;
    }
    if (xhr.status !== 200)
      return null;
    try {
      return JSON.parse(xhr.responseText);
    } catch (e) {
      return null;
    }
  }
  Object.defineProperty(window, "alert", {
    configurable: true,
    writable: false,
    value: function(message) {
      post("alert", toMessage(message), "");
    }
  });
  Object.defineProperty(window, "confirm", {
    configurable: true,
    writable: false,
    value: function(message) {
      const r = post("confirm", toMessage(message), "");
      return !!(r && r.accept);
    }
  });
  Object.defineProperty(window, "prompt", {
    configurable: true,
    writable: false,
    value: function(message, defaultValue) {
      const def = toMessage(defaultValue);
      const r = post("prompt", toMessage(message), def);
      if (!r || !r.accept)
        return null;
      return typeof r.promptText === "string" ? r.promptText : def;
    }
  });
}
