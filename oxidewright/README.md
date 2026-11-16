# oxidewright

A Rust port of the [Playwright](https://playwright.dev) API for Chromium, built on top of
[chromiumoxide](../README.md), with [Patchright](https://github.com/Kaliiiiiiiiii-Vinyzu/patchright)-level
stealth: the protocol traces that page scripts can observe are never left behind.

> **Disclaimer.** oxidewright is an independent project. It is inspired by Playwright and
> Patchright and bundles their injected scripts (Apache-2.0, see [NOTICE](NOTICE)), but it is
> **not affiliated with, endorsed by or sponsored by Microsoft** or the Playwright/Patchright
> projects.

## Install

oxidewright is not published on crates.io. Depend on it from git:

```toml
[dependencies]
oxidewright = { git = "https://github.com/hugocorreia2004/chromiumoxide" }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

* MSRV: Rust 1.85 (edition 2024). Async runtime: tokio.
* A Chrome or Chromium binary must be installed (it is detected, or pass
  `.executable_path(..)` to `launch()`).
* Cargo feature `stealth` (on by default) builds chromiumoxide with its own `stealth` feature.

## Quick start

```rust,no_run
use oxidewright::{chromium, LoadState};

#[tokio::main]
async fn main() -> oxidewright::Result<()> {
    let browser = chromium().launch().headless(true).await?;
    let context = browser.new_context().await?;
    let page = context.new_page().await?;

    let response = page.goto("https://example.com").await?;
    println!("status: {:?}", response.map(|r| r.status()));

    // Locators auto-wait and are strict, like Playwright's.
    page.get_by_role("link").first().click().timeout(5_000).await?;

    // Expressions or functions, called with the argument:
    let title: String = page.evaluate("() => document.title").await?;
    let sum: i32 = page.evaluate("([a, b]) => a + b").arg((1, 2)).await?;
    println!("{title} {sum}");

    page.wait_for_load_state().state(LoadState::NetworkIdle).await?;
    page.screenshot().path("page.png").full_page(true).await?;
    browser.close().await?;
    Ok(())
}
```

Coming from Playwright JS/TS? See [docs/from-playwright.md](docs/from-playwright.md) for the
conventions and a method-by-method mapping of every class.

## Examples

Runnable examples live in [`examples/`](examples). Except `connect_over_cdp`, they serve their
pages from routes on a made-up origin, so they need no network:

| Example | Shows |
|---|---|
| [`screenshot`](examples/screenshot.rs) | launch, viewport, page/element/full-page screenshots |
| [`connect_over_cdp`](examples/connect_over_cdp.rs) | attach to a Chrome started with `--remote-debugging-port` |
| [`locators_forms`](examples/locators_forms.rs) | `get_by_*` locators, filters, filling a form |
| [`network_mocking`](examples/network_mocking.rs) | `route` with `fulfill`/`abort`/`fallback`, request events |
| [`wait_for_events`](examples/wait_for_events.rs) | `wait_for_response`, `expect_popup`, dialogs, timeouts |
| [`storage_state`](examples/storage_state.rs) | save and restore cookies and `localStorage` |
| [`har`](examples/har.rs) | record a HAR and replay it with `route_from_har` |
| [`tracing`](examples/tracing.rs) | record a trace for `npx playwright show-trace` |

```sh
cargo run -p oxidewright --example locators_forms
```

## Conventions

* Same classes and methods as Playwright, in `snake_case`: `page.wait_for_load_state()`,
  `locator.press_sequentially(..)`, `handle.json_value()`.
* Option objects become builder methods chained before `.await` (the builders implement
  `IntoFuture`): `page.goto(url).wait_until(LoadState::Commit).timeout(5_000).await` is
  `await page.goto(url, { waitUntil: 'commit', timeout: 5000 })`. Option names and defaults
  are Playwright's.
* Reserved words: `r#type` (alias `type_text`), `r#move` (alias `move_to`), `continue_`,
  `and_`, `or_`.
* `evaluate` takes the JavaScript source as a string; if it is a function it is called with
  `.arg(..)` (any `serde::Serialize` value, may contain `JSHandle`s). Results deserialize into
  any `DeserializeOwned` type, through Playwright's own serializer, so `undefined`, `NaN`, `-0`,
  `Date`, `RegExp`, `BigInt`, `Map`/`Set` and handles round-trip; use `oxidewright::JsValue` to
  keep those distinctions.
* Errors are the `oxidewright::Error` enum (`Timeout`, `TargetClosed`, `Navigation`,
  `Evaluation`, `StrictModeViolation`, ...). Messages mirror Playwright's, e.g.
  `locator.click: Timeout 30000ms exceeded.` followed by a call log.
* Events: closures (`page.on_request(|req| ..)`, `page.on(EventKind::Popup, ..)`), waiters
  that subscribe when called (`page.wait_for_response("**/api")`) and `expect_*(action)`
  helpers (`page.expect_popup(action)`).
* `Browser`, `BrowserContext`, `Page`, `Frame`, `Locator` and handles are cheap `Clone` values.

## Stealth

What oxidewright never sends:

* `Runtime.enable`, `Log.enable`, `Performance.enable` (also not by chromiumoxide with the
  `stealth` feature), and no `Runtime.addBinding` (no `expose_function`).
* No utility world created at attach. Execution contexts are created on demand with
  `Page.createIsolatedWorld` (random name), cached per frame and dropped on navigation or
  detach. The selector engine and actions run in that isolated world, which shares the DOM
  with the page but not its JavaScript globals.
* Main-world evaluation (`page.evaluate`) goes through the `globalThis` object of the page
  (Patchright's technique), never through context ids learned from `Runtime` events. Page
  scripts can observe what your code does there, not how it got there.
* `Fetch.enable` is only sent while routes (or HTTP credentials) exist;
  `Page.setInterceptFileChooserDialog` only while a file chooser is awaited or listened for.
* Tracing and video use `Page.startScreencast`, which pages cannot observe. Screenshots never
  bring the page to front.

Opt-in features that **are** detectable when you enable them:

* `console` / `pageerror` events need `Runtime.enable`: create the context with
  `.console_events(true)` to get them.
* JS coverage enables the `Debugger` and `Profiler` domains (CSS coverage `DOM`/`CSS`).
* Every emulation override (viewport, user agent, locale, timezone, media features,
  geolocation, ...) changes what the page sees, so it is only sent when the option is set.
  Contexts from `browser.new_context()` keep Playwright's 1280x720 default viewport (use
  `.viewport(None)` to keep the window size); the default context of `connect_over_cdp`
  overrides nothing; unset media features are not forced to `light`/`no-preference`.
* CDP sessions (`new_cdp_session`) share the page session: whatever you send is your call.
* `route_web_socket` is deliberately absent: Playwright implements it with a script injected
  into the page.

## Feature matrix

| Area | Status |
|---|---|
| `chromium().launch()` / `connect_over_cdp()` | Implemented. Missing: `launch_persistent_context` (use `.user_data_dir(..)`), `connect`, `launch_server` |
| Browser, BrowserContext, Page, Frame | Implemented (navigation, lifecycle incl. `networkidle`, frames incl. out-of-process iframes, init scripts, tags, `close` with reason) |
| Locators, `get_by_*`, FrameLocator, filters, strict mode | Implemented |
| Actions (click, fill, press, check, select, upload, drag, ...) with actionability checks | Implemented |
| ElementHandle / JSHandle | Implemented |
| Keyboard, Mouse, Touchscreen | Implemented |
| Network: Request, Response, `route`, `unroute`, `route.fetch` | Implemented (`route.fetch` goes through the browser network stack) |
| Events and `wait_for_*` / `expect_*` | Implemented: popup, page, close, crash, dialog, download, filechooser, load, domcontentloaded, frame events, request events, websocket, worker; console/pageerror opt-in |
| Emulation and context options, `devices()` | Implemented (out-of-process iframes are not emulated separately yet) |
| Cookies, storage state | Implemented (Playwright-compatible JSON) |
| Screenshots, PDF | Implemented |
| HAR: `record_har`, `route_from_har` | Implemented; zipped HAR files not supported |
| APIRequestContext (`request()`, `context.request()`) | Implemented; missing `timing()`, `server_addr()`, `security_details()`, `fetch(request)`; requests use rustls, not Chrome's fingerprint |
| Tracing | Partial: screencast frames and groups; no per-action events, DOM snapshots, network or console entries |
| Video | Implemented with an external `ffmpeg` (`OXIDEWRIGHT_FFMPEG_PATH` or `PATH`) |
| Clock, coverage, CDP sessions, selectors (`set_test_id_attribute`) | Implemented (`selectors().register` missing) |
| Workers | Partial: `evaluate` (JSON values), no `evaluate_handle` |
| Assertions (`expect`) | Coming |
| Missing | `wait_for_function`, `wait_for_timeout`, `expose_function`/`expose_binding`, `add_locator_handler`, `route_web_socket`, service workers, `pause`, `page.aria_snapshot`, intercepting the very first request of a popup or out-of-process iframe |

## Updating the bundled scripts

```sh
node oxidewright/scripts/update-injected.mjs patchright-core@<version>   # or a .tgz / directory
```

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT)
at your option. The bundled JavaScript (`src/js/generated`) and the device descriptors
(`src/devices.json`) are Apache-2.0, see [NOTICE](NOTICE).
