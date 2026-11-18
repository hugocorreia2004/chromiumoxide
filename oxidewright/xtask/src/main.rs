//! Maintenance tasks of oxidewright, in Rust (no Node.js needed).
//!
//! ```text
//! cargo run -p oxidewright-xtask -- update-injected <package>
//! cargo run -p oxidewright-xtask -- merge-patchright <playwright-core-BASE> <patchright-core-BASE> <playwright-core-NEW>
//! ```
//!
//! A `<package>` is a `.tgz`, an extracted package directory or an npm spec
//! (`patchright-core@1.64.3`, downloaded from registry.npmjs.org).
//!
//! `update-injected` regenerates `oxidewright/src/js/generated/*.js` from a
//! built patchright-core or playwright-core package: its `lib/coreBundle.js`
//! holds, for every file of `packages/playwright-core/src/generated/`, a
//! `var sourceN = "...";` string literal with the compiled script.
//!
//! `merge-patchright` is for a Playwright release Patchright has not caught up
//! with yet: a three-way merge (`git merge-file`) of the Patchright patches
//! onto the newer Playwright scripts. BASE is the Playwright release the
//! Patchright one is built on. It fails if a file does not merge cleanly.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

type Result<T> = std::result::Result<T, String>;

const PATCHRIGHT_ORIGIN: &str = "Playwright (https://github.com/microsoft/playwright), as patched by Patchright (https://github.com/Kaliiiiiiiiii-Vinyzu/patchright)";
const PLAYWRIGHT_ORIGIN: &str = "Playwright (https://github.com/microsoft/playwright)";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["update-injected", package] => update_injected(package),
        ["merge-patchright", base, patched, latest] => merge_patchright(base, patched, latest),
        _ => Err(
            "usage:\n  oxidewright-xtask update-injected <package.tgz | package-dir | npm-spec>\n  oxidewright-xtask merge-patchright <playwright-core-BASE> <patchright-core-BASE> <playwright-core-NEW>"
                .to_string(),
        ),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{err}");
            ExitCode::FAILURE
        }
    }
}

/// `oxidewright/src/js/generated`.
fn out_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/js/generated")
}

// ----------------------------------------------------------------------
// Packages
// ----------------------------------------------------------------------

/// The scripts of a package.
struct Extracted {
    name: String,
    version: String,
    license: String,
    /// (script name, source), in bundle order.
    scripts: Vec<(String, String)>,
}

impl Extracted {
    fn label(&self) -> String {
        format!("{} {}", self.name, self.version)
    }

    fn origin(&self) -> &'static str {
        if self.name.starts_with("patchright") {
            PATCHRIGHT_ORIGIN
        } else {
            PLAYWRIGHT_ORIGIN
        }
    }

    fn script(&self, name: &str) -> Option<&str> {
        self.scripts
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, s)| s.as_str())
    }
}

/// Reads `package.json` and `lib/coreBundle.js` of a package.
fn read_package(input: &str) -> Result<(Vec<u8>, Vec<u8>)> {
    let path = Path::new(input);
    if path.is_dir() {
        let read = |rel: &str| std::fs::read(path.join(rel)).map_err(|e| format!("{rel}: {e}"));
        return Ok((read("package.json")?, read("lib/coreBundle.js")?));
    }
    let tarball = if path.is_file() {
        std::fs::read(path).map_err(|e| format!("{input}: {e}"))?
    } else {
        download(input)?
    };
    let mut tar = Vec::new();
    flate2::read::GzDecoder::new(tarball.as_slice())
        .read_to_end(&mut tar)
        .map_err(|e| format!("{input}: not a gzipped tarball: {e}"))?;
    let mut files = untar(&tar);
    let mut take = |rel: &str| {
        files
            .remove(&format!("package/{rel}"))
            .ok_or_else(|| format!("{rel} not found (unsupported package layout)"))
    };
    Ok((take("package.json")?, take("lib/coreBundle.js")?))
}

/// Downloads the tarball of an npm spec (`name@version`, `name` = latest).
fn download(spec: &str) -> Result<Vec<u8>> {
    // A scoped name starts with '@': the version separator is a later '@'.
    let (name, version) = match spec[1..].rfind('@') {
        Some(i) => (&spec[..i + 1], &spec[i + 2..]),
        None => (spec, "latest"),
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    runtime.block_on(async {
        let client = reqwest::Client::new();
        let meta_url = format!("https://registry.npmjs.org/{name}/{version}");
        let meta = client
            .get(&meta_url)
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|e| format!("{spec}: {e}"))?
            .bytes()
            .await
            .map_err(|e| format!("{spec}: {e}"))?;
        let meta: serde_json::Value =
            serde_json::from_slice(&meta).map_err(|e| format!("{meta_url}: {e}"))?;
        let tarball = meta["dist"]["tarball"]
            .as_str()
            .ok_or_else(|| format!("{spec}: no dist.tarball in {meta_url}"))?;
        eprintln!("downloading {tarball}");
        let bytes = client
            .get(tarball)
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|e| format!("{tarball}: {e}"))?
            .bytes()
            .await
            .map_err(|e| format!("{tarball}: {e}"))?;
        Ok(bytes.to_vec())
    })
}

/// Minimal ustar reader: the regular files, by path.
fn untar(buffer: &[u8]) -> HashMap<String, Vec<u8>> {
    let mut files = HashMap::new();
    let mut offset = 0;
    let mut long_name: Option<String> = None;
    while offset + 512 <= buffer.len() {
        let header = &buffer[offset..offset + 512];
        if header.iter().all(|b| *b == 0) {
            break;
        }
        let field = |start: usize, len: usize| {
            let raw = &header[start..start + len];
            let end = raw.iter().position(|b| *b == 0).unwrap_or(len);
            String::from_utf8_lossy(&raw[..end]).into_owned()
        };
        let mut name = field(0, 100);
        let prefix = field(345, 155);
        if !prefix.is_empty() {
            name = format!("{prefix}/{name}");
        }
        let size = usize::from_str_radix(field(124, 12).trim(), 8).unwrap_or(0);
        let kind = header[156];
        let data_start = offset + 512;
        let data = &buffer[data_start..(data_start + size).min(buffer.len())];
        if kind == b'L' {
            let end = data.iter().position(|b| *b == 0).unwrap_or(data.len());
            long_name = Some(String::from_utf8_lossy(&data[..end]).into_owned());
        } else {
            if let Some(long) = long_name.take() {
                name = long;
            }
            if kind == b'0' || kind == 0 {
                files.insert(name, data.to_vec());
            }
        }
        offset = data_start + size.div_ceil(512) * 512;
    }
    files
}

/// Extracts the generated scripts of a package.
fn extract(input: &str) -> Result<Extracted> {
    let (package_json, bundle) = read_package(input)?;
    let package: serde_json::Value =
        serde_json::from_slice(&package_json).map_err(|e| format!("package.json: {e}"))?;
    let field = |key: &str| package[key].as_str().unwrap_or_default().to_string();
    let bundle = String::from_utf8(bundle).map_err(|e| format!("lib/coreBundle.js: {e}"))?;
    let scripts = extract_scripts(&bundle)?;
    for required in ["injectedScriptSource", "utilityScriptSource"] {
        if !scripts.iter().any(|(n, _)| n == required) {
            return Err(format!("missing {required}"));
        }
    }
    Ok(Extracted {
        name: field("name"),
        version: field("version"),
        license: field("license"),
        scripts,
    })
}

/// Finds `// packages/playwright-core/src/generated/<name>.ts\nvar sourceN;`
/// and the string literal later assigned to `sourceN`.
fn extract_scripts(bundle: &str) -> Result<Vec<(String, String)>> {
    const MARKER: &str = "// packages/playwright-core/src/generated/";
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(found) = bundle[from..].find(MARKER) {
        let at = from + found;
        from = at + MARKER.len();
        let rest = &bundle[from..];
        let Some(name_end) = rest.find(".ts\nvar ") else {
            continue;
        };
        let name = &rest[..name_end];
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            continue;
        }
        let after = &rest[name_end + ".ts\nvar ".len()..];
        let Some(semi) = after.find(';') else {
            continue;
        };
        let variable = &after[..semi];
        if !variable.starts_with("source")
            || !variable["source".len()..]
                .chars()
                .all(|c| c.is_ascii_digit())
        {
            continue;
        }
        let assignment = format!("{variable} = ");
        let start = bundle[at..]
            .find(&assignment)
            .map(|i| at + i + assignment.len())
            .ok_or_else(|| format!("cannot find the initializer of {variable} ({name})"))?;
        let source = unescape_js_string(&bundle[start..]).map_err(|e| format!("{name}: {e}"))?;
        out.push((name.to_string(), source));
    }
    if out.is_empty() {
        return Err("no generated scripts found in lib/coreBundle.js".to_string());
    }
    Ok(out)
}

/// Evaluates the JavaScript string literal at the start of `text`.
fn unescape_js_string(text: &str) -> Result<String> {
    let mut chars = text.chars();
    let quote = chars.next().ok_or("empty literal")?;
    if !matches!(quote, '"' | '\'' | '`') {
        return Err(format!("expected a string literal, found {quote:?}"));
    }
    let mut units: Vec<u16> = Vec::new();
    let push = |units: &mut Vec<u16>, c: char| {
        let mut buf = [0u16; 2];
        units.extend_from_slice(c.encode_utf16(&mut buf));
    };
    let hex = |chars: &mut std::str::Chars, n: usize| -> Result<u32> {
        let digits: String = chars.by_ref().take(n).collect();
        u32::from_str_radix(&digits, 16).map_err(|_| format!("bad escape \\{digits}"))
    };
    loop {
        let c = chars.next().ok_or("unterminated string literal")?;
        if c == quote {
            break;
        }
        if quote == '`' && c == '$' && chars.as_str().starts_with('{') {
            return Err("template literal with interpolation".to_string());
        }
        if c != '\\' {
            push(&mut units, c);
            continue;
        }
        let e = chars.next().ok_or("unterminated escape")?;
        match e {
            'n' => units.push(0x0a),
            'r' => units.push(0x0d),
            't' => units.push(0x09),
            'b' => units.push(0x08),
            'f' => units.push(0x0c),
            'v' => units.push(0x0b),
            '0' if !chars.as_str().starts_with(|c: char| c.is_ascii_digit()) => units.push(0),
            'x' => units.push(hex(&mut chars, 2)? as u16),
            'u' => {
                if chars.as_str().starts_with('{') {
                    chars.next();
                    let digits: String = chars.by_ref().take_while(|c| *c != '}').collect();
                    let code = u32::from_str_radix(&digits, 16)
                        .ok()
                        .and_then(char::from_u32)
                        .ok_or_else(|| format!("bad escape \\u{{{digits}}}"))?;
                    push(&mut units, code);
                } else {
                    units.push(hex(&mut chars, 4)? as u16);
                }
            }
            // Line continuations.
            '\r' => {
                if chars.as_str().starts_with('\n') {
                    chars.next();
                }
            }
            '\n' | '\u{2028}' | '\u{2029}' => {}
            other => push(&mut units, other),
        }
    }
    Ok(String::from_utf16_lossy(&units))
}

// ----------------------------------------------------------------------
// Writing
// ----------------------------------------------------------------------

/// The 5 comment lines and the blank line before every script.
fn header(from: &str, origin: &str, name: &str, license: &str, command: &str) -> String {
    [
        format!(
            "// Extracted from {from} (lib/coreBundle.js, packages/playwright-core/src/generated/{name}.ts)."
        ),
        format!("// Origin: {origin}."),
        "// Copyright (c) Microsoft Corporation and the Patchright contributors.".to_string(),
        format!(
            "// Licensed under the Apache License, Version 2.0 ({license}); see oxidewright/NOTICE."
        ),
        format!("// Do not edit: regenerate with `cargo run -p oxidewright-xtask -- {command}`."),
        String::new(),
    ]
    .join("\n")
}

fn clear_out_dir(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    for entry in std::fs::read_dir(dir).map_err(|e| e.to_string())? {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.extension().is_some_and(|e| e == "js") {
            std::fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        }
    }
    Ok(())
}

fn write(path: &Path, contents: &str) -> Result<()> {
    std::fs::write(path, contents).map_err(|e| format!("{}: {e}", path.display()))
}

fn update_injected(input: &str) -> Result<()> {
    let package = extract(input)?;
    let dir = out_dir();
    clear_out_dir(&dir)?;
    for (name, source) in &package.scripts {
        let head = header(
            &package.label(),
            package.origin(),
            name,
            &package.license,
            "update-injected <package>",
        );
        write(
            &dir.join(format!("{name}.js")),
            &(head + &source.replace("\r\n", "\n")),
        )?;
        println!("{name}.js: {} chars", source.encode_utf16().count());
    }
    write(&dir.join("VERSION"), &format!("{}\n", package.label()))
}

fn merge_patchright(base: &str, patched: &str, latest: &str) -> Result<()> {
    let base = extract(base)?;
    let patched = extract(patched)?;
    let latest = extract(latest)?;
    let work = std::env::temp_dir().join(format!("oxw-merge-{}", std::process::id()));
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let result = (|| {
        let mut merged = Vec::new();
        for (name, theirs) in &latest.scripts {
            // A file Patchright removed stays removed.
            let Some(ours) = patched.script(name) else {
                println!("{name}.js: skipped (not in {})", patched.label());
                continue;
            };
            let old = base.script(name).unwrap_or_default();
            let normalize = |s: &str| s.replace("\r\n", "\n");
            let files = [("ours", ours), ("base", old), ("theirs", theirs.as_str())]
                .map(|(file, text)| (work.join(file), normalize(text)));
            for (path, text) in &files {
                write(path, text)?;
            }
            let output = Command::new("git")
                .arg("merge-file")
                .arg("-p")
                .args(files.iter().map(|(p, _)| p))
                .output()
                .map_err(|e| format!("git merge-file: {e}"))?;
            match output.status.code() {
                Some(0) => {}
                Some(n) if n > 0 => {
                    return Err(format!("{name}.js: {n} conflict(s), fix by hand"));
                }
                _ => {
                    return Err(format!(
                        "git merge-file failed: {}",
                        String::from_utf8_lossy(&output.stderr)
                    ));
                }
            }
            let text = String::from_utf8(output.stdout).map_err(|e| e.to_string())?;
            merged.push((name.clone(), text));
        }
        Ok(merged)
    })();
    let _ = std::fs::remove_dir_all(&work);
    let merged = result?;
    let from = format!(
        "{} with the {} patches merged in",
        latest.label(),
        patched.label()
    );
    let dir = out_dir();
    clear_out_dir(&dir)?;
    for (name, text) in &merged {
        let head = header(
            &from,
            PATCHRIGHT_ORIGIN,
            name,
            &latest.license,
            "merge-patchright <base> <patched> <latest>",
        );
        write(&dir.join(format!("{name}.js")), &(head + text))?;
        println!("{name}.js: merged");
    }
    write(
        &dir.join("VERSION"),
        &format!("{} + {} patches\n", latest.label(), patched.label()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unescapes_js_literals() {
        assert_eq!(
            unescape_js_string(r#""a\nb\"c\\d";"#).unwrap(),
            "a\nb\"c\\d"
        );
        assert_eq!(
            unescape_js_string(r"'\x41B\u{1F600}'").unwrap(),
            "AB\u{1F600}"
        );
        assert_eq!(unescape_js_string(r"'😀'").unwrap(), "\u{1F600}");
        assert_eq!(unescape_js_string("'a\\\nb'").unwrap(), "ab");
        assert_eq!(unescape_js_string(r"'\0\q'").unwrap(), "\0q");
        assert!(unescape_js_string("`${x}`").is_err());
    }

    #[test]
    fn finds_generated_scripts() {
        let bundle = "x\n// packages/playwright-core/src/generated/fooSource.ts\nvar source3;\nvar init = () => { source3 = \"hi\\n\"; };";
        assert_eq!(
            extract_scripts(bundle).unwrap(),
            vec![("fooSource".to_string(), "hi\n".to_string())]
        );
    }
}
