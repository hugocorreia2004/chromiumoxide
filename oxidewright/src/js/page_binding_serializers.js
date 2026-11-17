// Extracted from patchright-core 1.64.3 (lib/coreBundle.js, packages/playwright-core/src/utils/isomorphic/oldUtilityScriptSerializers.ts: source()).
// Origin: Playwright (https://github.com/microsoft/playwright), as patched by Patchright (https://github.com/Kaliiiiiiiiii-Vinyzu/patchright).
// Copyright (c) Microsoft Corporation and the Patchright contributors.
// Licensed under the Apache License, Version 2.0 (Apache-2.0); see oxidewright/NOTICE.
function source6() {
  function isRegExp7(obj) {
    try {
      return obj instanceof RegExp || Object.prototype.toString.call(obj) === "[object RegExp]";
    } catch (error) {
      return false;
    }
  }
  function isDate4(obj) {
    try {
      return obj instanceof Date || Object.prototype.toString.call(obj) === "[object Date]";
    } catch (error) {
      return false;
    }
  }
  function isURL3(obj) {
    try {
      return obj instanceof URL || Object.prototype.toString.call(obj) === "[object URL]";
    } catch (error) {
      return false;
    }
  }
  function isError4(obj) {
    try {
      return obj instanceof Error || obj && Object.getPrototypeOf(obj)?.name === "Error";
    } catch (error) {
      return false;
    }
  }
  function isTypedArray2(obj, constructor) {
    try {
      return obj instanceof constructor || Object.prototype.toString.call(obj) === `[object ${constructor.name}]`;
    } catch (error) {
      return false;
    }
  }
  const typedArrayConstructors2 = {
    i8: Int8Array,
    ui8: Uint8Array,
    ui8c: Uint8ClampedArray,
    i16: Int16Array,
    ui16: Uint16Array,
    i32: Int32Array,
    ui32: Uint32Array,
    // TODO: add Float16Array once it's in baseline
    f32: Float32Array,
    f64: Float64Array,
    bi64: BigInt64Array,
    bui64: BigUint64Array
  };
  function typedArrayToBase642(array) {
    if ("toBase64" in array) return array.toBase64();
    const binary = Array.from(new Uint8Array(array.buffer, array.byteOffset, array.byteLength)).map((b) => String.fromCharCode(b)).join("");
    return btoa(binary);
  }
  function base64ToTypedArray2(base64, TypedArrayConstructor) {
    const binary = atob(base64);
    const bytes = new Uint8Array(binary.length);
    for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
    return new TypedArrayConstructor(bytes.buffer);
  }
  function parseEvaluationResultValue2(value2, handles = [], refs = /* @__PURE__ */ new Map()) {
    if (Object.is(value2, void 0)) return void 0;
    if (typeof value2 === "object" && value2) {
      if ("ref" in value2) return refs.get(value2.ref);
      if ("v" in value2) {
        if (value2.v === "undefined") return void 0;
        if (value2.v === "null") return null;
        if (value2.v === "NaN") return NaN;
        if (value2.v === "Infinity") return Infinity;
        if (value2.v === "-Infinity") return -Infinity;
        if (value2.v === "-0") return -0;
        return void 0;
      }
      if ("d" in value2) return new Date(value2.d);
      if ("u" in value2) return new URL(value2.u);
      if ("bi" in value2) return BigInt(value2.bi);
      if ("e" in value2) {
        const error = new Error(value2.e.m);
        error.name = value2.e.n;
        error.stack = value2.e.s;
        return error;
      }
      if ("r" in value2) return new RegExp(value2.r.p, value2.r.f);
      if ("a" in value2) {
        const result2 = [];
        refs.set(value2.id, result2);
        for (const a of value2.a) result2.push(parseEvaluationResultValue2(a, handles, refs));
        return result2;
      }
      if ("o" in value2) {
        const result2 = {};
        refs.set(value2.id, result2);
        for (const { k, v } of value2.o) result2[k] = parseEvaluationResultValue2(v, handles, refs);
        return result2;
      }
      if ("h" in value2) return handles[value2.h];
      if ("ta" in value2) return base64ToTypedArray2(value2.ta.b, typedArrayConstructors2[value2.ta.k]);
    }
    return value2;
  }
  function serializeAsCallArgument2(value2, handleSerializer) {
    return serialize2(value2, handleSerializer, { visited: /* @__PURE__ */ new Map(), lastId: 0 });
  }
  function serialize2(value2, handleSerializer, visitorInfo) {
    if (value2 && typeof value2 === "object") {
      if (typeof globalThis.Window === "function" && value2 instanceof globalThis.Window) return "ref: <Window>";
      if (typeof globalThis.Document === "function" && value2 instanceof globalThis.Document) return "ref: <Document>";
      if (typeof globalThis.Node === "function" && value2 instanceof globalThis.Node) return "ref: <Node>";
    }
    return innerSerialize2(value2, handleSerializer, visitorInfo);
  }
  function innerSerialize2(value2, handleSerializer, visitorInfo) {
    const result2 = handleSerializer(value2);
    if ("fallThrough" in result2) value2 = result2.fallThrough;
    else return result2;
    if (typeof value2 === "symbol") return { v: "undefined" };
    if (Object.is(value2, void 0)) return { v: "undefined" };
    if (Object.is(value2, null)) return { v: "null" };
    if (Object.is(value2, NaN)) return { v: "NaN" };
    if (Object.is(value2, Infinity)) return { v: "Infinity" };
    if (Object.is(value2, -Infinity)) return { v: "-Infinity" };
    if (Object.is(value2, -0)) return { v: "-0" };
    if (typeof value2 === "boolean") return value2;
    if (typeof value2 === "number") return value2;
    if (typeof value2 === "string") return value2;
    if (typeof value2 === "bigint") return { bi: value2.toString() };
    if (isError4(value2)) {
      let stack;
      if (value2.stack?.startsWith(value2.name + ": " + value2.message)) {
        stack = value2.stack;
      } else {
        stack = `${value2.name}: ${value2.message}
${value2.stack}`;
      }
      return { e: { n: value2.name, m: value2.message, s: stack } };
    }
    if (isDate4(value2)) return { d: value2.toJSON() };
    if (isURL3(value2)) return { u: value2.toJSON() };
    if (isRegExp7(value2)) return { r: { p: value2.source, f: value2.flags } };
    for (const [k, ctor] of Object.entries(typedArrayConstructors2)) {
      if (isTypedArray2(value2, ctor)) return { ta: { b: typedArrayToBase642(value2), k } };
    }
    const id = visitorInfo.visited.get(value2);
    if (id) return { ref: id };
    if (Array.isArray(value2)) {
      const a = [];
      const id2 = ++visitorInfo.lastId;
      visitorInfo.visited.set(value2, id2);
      for (let i = 0; i < value2.length; ++i) a.push(serialize2(value2[i], handleSerializer, visitorInfo));
      return { a, id: id2 };
    }
    if (typeof value2 === "object") {
      const o = [];
      const id2 = ++visitorInfo.lastId;
      visitorInfo.visited.set(value2, id2);
      for (const name of Object.keys(value2)) {
        let item;
        try {
          item = value2[name];
        } catch (e) {
          continue;
        }
        if (name === "toJSON" && typeof item === "function") o.push({ k: name, v: { o: [], id: 0 } });
        else o.push({ k: name, v: serialize2(item, handleSerializer, visitorInfo) });
      }
      let jsonWrapper;
      try {
        if (o.length === 0 && value2.toJSON && typeof value2.toJSON === "function")
          jsonWrapper = { value: value2.toJSON() };
      } catch (e) {
      }
      if (jsonWrapper) return innerSerialize2(jsonWrapper.value, handleSerializer, visitorInfo);
      return { o, id: id2 };
    }
  }
  return { parseEvaluationResultValue: parseEvaluationResultValue2, serializeAsCallArgument: serializeAsCallArgument2 };
}
