# oxidewright

A Rust port of the [Playwright](https://playwright.dev) API for Chromium, built on top of
[chromiumoxide](../README.md).

> **Disclaimer.** oxidewright is an independent project. It is inspired by Playwright and
> [Patchright](https://github.com/Kaliiiiiiiiii-Vinyzu/patchright) and bundles their injected
> scripts (Apache-2.0, see [NOTICE](NOTICE)), but it is **not affiliated with, endorsed by or
> sponsored by Microsoft** or the Playwright/Patchright projects.

## Usage

```rust,no_run
use oxidewright::{chromium, LoadState};

#[tokio::main]
async fn main() -> oxidewright::Result<()> {
    let browser = chromium().launch().headless(true).await?;
    let context = browser.new_context().await?;
    let page = context.new_page().await?;

    let response = page.goto("https://example.com").await?;
    println!("status: {:?}", response.map(|r| r.status()));

    // Expressions or functions, called with the argument:
    let title: String = page.evaluate("() => document.title").await?;
    let sum: i32 = page.evaluate("([a, b]) => a + b").arg((1, 2)).await?;
    println!("{title} {sum}");

    page.wait_for_load_state().state(LoadState::NetworkIdle).await?;
    browser.close().await?;
    Ok(())
}
```

## Conventions

* Same classes and methods as Playwright, in `snake_case`: `page.wait_for_load_state()`,
  `frame.child_frames()`, `handle.json_value()`.
* Option objects become builder methods chained before `.await`:
  `page.goto(url).wait_until(LoadState::Commit).timeout(5_000).await` is
  `await page.goto(url, { waitUntil: 'commit', timeout: 5000 })`.
* `evaluate` takes the JavaScript source as a string. If it evaluates to a function, the
  function is called with the argument (`.arg(..)`, any `serde::Serialize` value, which may
  contain `JSHandle`s). Results are deserialized into any `serde::de::DeserializeOwned` type.
  Values use Playwright's own serializer, so `undefined`, `NaN`, `-0`, `Date`, `RegExp`,
  `BigInt`, `Map`/`Set` and handles round-trip like in Playwright; use `oxidewright::JsValue`
  to keep those distinctions.
* Errors are `oxidewright::Error` and their messages mirror Playwright's, e.g.
  `page.goto: Timeout 30000ms exceeded.` followed by a call log.
* `Page`, `Frame`, `BrowserContext`, `Browser` and handles are cheap `Clone` values.

## How it talks to the browser

* Execution contexts are created on demand (`Page.createIsolatedWorld` for the utility
  world, the `globalThis` object of the main world for page-world evaluation), so
  `Runtime.enable` is never needed.
* The injected `InjectedScript` / `UtilityScript` bundles come from Patchright's build of
  Playwright and are instantiated exactly like Playwright's server does.
* Frame lifecycle (`commit`, `domcontentloaded`, `load`, `networkidle`) and navigation
  waiting follow Playwright's `server/frames.ts`; out-of-process iframes are handled through
  chromiumoxide's per-frame sessions.

## Status

Implemented: `chromium()`, `BrowserType::{launch, connect_over_cdp}`, `Browser`,
`BrowserContext`, `Page` and `Frame` navigation/evaluation/hierarchy, `JSHandle` and
`ElementHandle` basics, `Request`/`Response` of navigations.

Planned: locators, actions and selectors; network routing and events; keyboard, mouse and
touch; dialogs, downloads, workers and the rest of the Playwright API.

### Tracing and video

* `context.tracing()` writes zips in Playwright's trace format (open them with
  `npx playwright show-trace`). They contain the context header, screencast frames
  (`screenshots(true)`) and `group`/`group_end` entries. Per-action events, DOM
  snapshots, network entries, console messages and sources are not recorded yet.
* `record_video_dir(..)` records a WebM video per page (`page.video()`), encoded by an
  external `ffmpeg` with the `libvpx` encoder: set `OXIDEWRIGHT_FFMPEG_PATH` or put
  `ffmpeg` on `PATH` (Playwright bundles its own build; oxidewright does not).
* Both use `Page.startScreencast`, which page scripts cannot observe.

## Updating the bundled scripts

```sh
node oxidewright/scripts/update-injected.mjs patchright-core@<version>   # or a .tgz / directory
```

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT)
at your option. The bundled JavaScript (`src/js/generated`) is Apache-2.0, see [NOTICE](NOTICE).
