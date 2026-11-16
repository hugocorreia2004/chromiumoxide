#!/usr/bin/env node
// Regenerates `oxidewright/src/js/generated/*.js` from a built patchright-core or
// playwright-core npm package.
//
// Usage:
//   node oxidewright/scripts/update-injected.mjs <package.tgz | extracted-package-dir | npm-spec>
//
// Examples:
//   node oxidewright/scripts/update-injected.mjs ./patchright-core-1.64.3.tgz
//   node oxidewright/scripts/update-injected.mjs patchright-core@1.64.3   (runs `npm pack`)
//
// The bundled `lib/coreBundle.js` of those packages contains, for every file in
// `packages/playwright-core/src/generated/`, a `var sourceN = "...";` string literal
// holding the compiled injected script. This script extracts each literal, evaluates
// it as a plain JS string and writes it next to a header naming its origin, version
// and license. It has no dependencies besides Node.js (>= 18).

import fs from 'node:fs';
import path from 'node:path';
import zlib from 'node:zlib';
import os from 'node:os';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const outDir = path.resolve(here, '..', 'src', 'js', 'generated');

function die(message) {
  console.error(message);
  process.exit(1);
}

/** Minimal ustar reader: returns Map<path, Buffer> of regular files. */
function untar(buffer) {
  const files = new Map();
  let offset = 0;
  let longName;
  while (offset + 512 <= buffer.length) {
    const header = buffer.subarray(offset, offset + 512);
    if (header.every(b => b === 0))
      break;
    const field = (start, len) => header.subarray(start, start + len).toString('utf8').replace(/\0.*$/s, '');
    let name = field(0, 100);
    const prefix = field(345, 155);
    if (prefix)
      name = prefix + '/' + name;
    const size = parseInt(field(124, 12).trim() || '0', 8);
    const type = String.fromCharCode(header[156] || 48);
    const dataStart = offset + 512;
    const data = buffer.subarray(dataStart, dataStart + size);
    if (type === 'L') {
      longName = data.toString('utf8').replace(/\0.*$/s, '');
    } else {
      if (longName) {
        name = longName;
        longName = undefined;
      }
      if (type === '0' || type === '\0')
        files.set(name, Buffer.from(data));
    }
    offset = dataStart + Math.ceil(size / 512) * 512;
  }
  return files;
}

function readPackage(input) {
  if (fs.existsSync(input) && fs.statSync(input).isDirectory()) {
    const read = rel => {
      const p = path.join(input, rel);
      return fs.existsSync(p) ? fs.readFileSync(p) : undefined;
    };
    return { read };
  }
  let tarball = input;
  if (!fs.existsSync(input)) {
    // Treat it as an npm package spec.
    const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'ow-injected-'));
    const out = execFileSync('npm', ['pack', input, '--pack-destination', tmp], { encoding: 'utf8', shell: process.platform === 'win32' });
    tarball = path.join(tmp, out.trim().split(/\r?\n/).pop());
  }
  const files = untar(zlib.gunzipSync(fs.readFileSync(tarball)));
  const read = rel => files.get('package/' + rel);
  return { read };
}

const input = process.argv[2];
if (!input)
  die('usage: node update-injected.mjs <package.tgz | package-dir | npm-spec>');

const pkg = readPackage(input);
const packageJson = JSON.parse((pkg.read('package.json') || die('package.json not found')).toString('utf8'));
const bundle = (pkg.read('lib/coreBundle.js') || die('lib/coreBundle.js not found (unsupported package layout)')).toString('utf8');

const marker = /\/\/ packages\/playwright-core\/src\/generated\/(\w+)\.ts\nvar (source\d*);/g;
const extracted = [];
let match;
while ((match = marker.exec(bundle))) {
  const [, name, variable] = match;
  let start = bundle.indexOf(variable + ' = ', match.index);
  if (start < 0)
    die(`cannot find the initializer of ${variable} (${name})`);
  start += variable.length + 3;
  const quote = bundle[start];
  let end = start + 1;
  while (bundle[end] !== quote) {
    if (bundle.charCodeAt(end) === 92 /* backslash */)
      end++;
    end++;
  }
  // The literal is a plain JS string (no interpolation): evaluate it to unescape.
  const source = Function('return ' + bundle.slice(start, end + 1))();
  extracted.push({ name, source });
}
if (!extracted.length)
  die('no generated scripts found in lib/coreBundle.js');
for (const required of ['injectedScriptSource', 'utilityScriptSource']) {
  if (!extracted.some(e => e.name === required))
    die(`missing ${required}`);
}

const project = packageJson.name.startsWith('patchright')
  ? 'Playwright (https://github.com/microsoft/playwright), as patched by Patchright (https://github.com/Kaliiiiiiiiii-Vinyzu/patchright)'
  : 'Playwright (https://github.com/microsoft/playwright)';

fs.mkdirSync(outDir, { recursive: true });
for (const file of fs.readdirSync(outDir)) {
  if (file.endsWith('.js'))
    fs.unlinkSync(path.join(outDir, file));
}
for (const { name, source } of extracted) {
  const header = [
    `// Extracted from ${packageJson.name} ${packageJson.version} (lib/coreBundle.js, packages/playwright-core/src/generated/${name}.ts).`,
    `// Origin: ${project}.`,
    `// Copyright (c) Microsoft Corporation and the Patchright contributors.`,
    `// Licensed under the Apache License, Version 2.0 (${packageJson.license}); see oxidewright/NOTICE.`,
    `// Do not edit: regenerate with \`node oxidewright/scripts/update-injected.mjs <package>\`.`,
    '',
  ].join('\n');
  fs.writeFileSync(path.join(outDir, name + '.js'), header + source.replace(/\r\n/g, '\n'));
  console.log(`${name}.js: ${source.length} chars`);
}
fs.writeFileSync(path.join(outDir, 'VERSION'), `${packageJson.name} ${packageJson.version}\n`);
