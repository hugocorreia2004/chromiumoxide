// Extracted from patchright-core 1.64.3 (lib/coreBundle.js, packages/playwright-core/src/generated/webMCPSource.ts).
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


// packages/injected/src/webMCP.ts
var webMCP_exports = {};
__export(webMCP_exports, {
  WebMCPScript: () => WebMCPScript
});
module.exports = __toCommonJS(webMCP_exports);
var WebMCPScript = class {
  constructor(global) {
    var _a, _b, _c;
    this._global = global;
    this._modelContext = (_c = (_a = global.document) == null ? void 0 : _a.modelContext) != null ? _c : (_b = global.navigator) == null ? void 0 : _b.modelContext;
  }
  async tools() {
    return (await this._ownTools()).map((tool) => this._describe(tool));
  }
  async callTool(name, input) {
    const modelContext = this._modelContext;
    if (!modelContext)
      throw new Error("WebMCP is not available on this page");
    if (modelContext.invokeTool)
      return await modelContext.invokeTool(name, input);
    const tool = (await this._ownTools()).find((tool2) => tool2.name === name);
    if (!tool || !modelContext.executeTool)
      throw new Error(`WebMCP tool "${name}" is not registered in this frame`);
    return this._parseResult(await this._executeTool(modelContext, tool, input));
  }
  async _executeTool(modelContext, tool, input) {
    try {
      return await modelContext.executeTool(tool, input);
    } catch (e) {
      if (!String(e == null ? void 0 : e.message).includes("Failed to parse input"))
        throw e;
      return await modelContext.executeTool(tool, JSON.stringify(input));
    }
  }
  _parseResult(result) {
    if (typeof result !== "string")
      return result;
    if (result === "undefined")
      return void 0;
    try {
      return JSON.parse(result);
    } catch {
      return result;
    }
  }
  async _ownTools() {
    var _a, _b, _c;
    const tools = (_c = await ((_b = (_a = this._modelContext) == null ? void 0 : _a.getTools) == null ? void 0 : _b.call(_a))) != null ? _c : [];
    return tools.filter((tool) => !("window" in tool) || tool.window === this._global.window);
  }
  _describe(tool) {
    var _a;
    const annotations = tool.annotations;
    return {
      name: tool.name,
      description: (_a = tool.description) != null ? _a : "",
      inputSchema: this._parseInputSchema(tool.inputSchema),
      annotations: annotations ? {
        readOnly: annotations.readOnlyHint,
        untrustedContent: annotations.untrustedContentHint,
        consequential: annotations.consequentialHint
      } : void 0
    };
  }
  _parseInputSchema(inputSchema) {
    if (typeof inputSchema !== "string")
      return inputSchema;
    try {
      return JSON.parse(inputSchema);
    } catch {
      return void 0;
    }
  }
};
