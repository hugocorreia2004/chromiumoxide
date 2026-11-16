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
`ElementHandle` basics, and the network and events API:

* `Request`/`Response` (headers, raw headers, post data, timing, sizes, redirect chains,
  bodies) and the `request`, `response`, `requestfinished`, `requestfailed` events.
* Routing: `page.route`/`context.route` (glob, regex, predicate, `times`), `unroute`,
  `unroute_all`, `Route::{fulfill, continue_, fallback, abort, fetch}`, `route_from_har`.
  `Fetch.enable` is only sent while routes (or HTTP credentials) exist.
* Events with closure handlers (`page.on_request(..)`, `page.on(EventKind::Popup, ..)`) and
  waiters (`page.wait_for_request(..)`, `page.expect_popup(action)`, ...): popup, page, close,
  crash, dialog (auto-dismissed without listeners), download, filechooser, load,
  domcontentloaded, frame events, websocket, worker.
* `record_har`, `set_extra_http_headers`, `set_offline`, `http_credentials`,
  `ignore_https_errors`, `accept_downloads`.

```rust,ignore
page.route("**/*.png", |route: Route| async move { route.abort().await }).await?;

// The subscription starts when `wait_for_response` is called.
let response = page.wait_for_response("**/api/data");
page.evaluate::<()>("() => { fetch('/api/data') }").await?;
let data: serde_json::Value = response.await?.json().await?;

let popup = page.expect_popup(page.evaluate::<()>("() => { window.open('/x') }")).await?;
```

### Network and events: differences with Playwright

* `console` and `pageerror` events need `Runtime.enable`, which page scripts can detect
  (Patchright drops them entirely). They are **opt-in**: create the context with
  `.console_events(true)`.
* `route.fetch()` performs the request through the browser itself (the request is continued
  with response interception), so it keeps the browser's network fingerprint; it can be called
  once per route and returns redirects as-is.
* Workers are evaluated with `Runtime.evaluate` in their session (no `Runtime.enable`): values
  cross as JSON and `evaluate_handle` is not available.
* Not implemented: `route_web_socket` (Playwright implements it with a script injected in the
  page, which is detectable), service workers (`serviceworker` event, `fromServiceWorker`
  routing), zipped HAR files, intercepting the very first request of a popup or of an
  out-of-process iframe (they are adopted once the browser reports them).

Emulation and capture:

* every context option of `browser.new_context()` that changes what pages see: `viewport`
  (`None` = no override), `screen`, `device_scale_factor`, `is_mobile`, `has_touch`,
  `user_agent`, `locale`, `timezone_id`, `geolocation`, `permissions`, `color_scheme`,
  `reduced_motion`, `forced_colors`, `contrast`, `java_script_enabled`, `bypass_csp`,
  `ignore_https_errors`, `accept_downloads`, `base_url`, `strict_selectors`,
  `service_workers`, `storage_state`, and `.device(&devices()["iPhone 13"])`;
* `page.set_viewport_size`, `page.emulate_media`, `context.set_geolocation`,
  `grant_permissions`/`clear_permissions`;
* cookies (`cookies`, `add_cookies`, `clear_cookies` with filters) and storage state
  (`storage_state()`, the `storage_state` option, `set_storage_state`), JSON-compatible
  with Playwright's files;
* `page.screenshot()` / `element_handle.screenshot()` with every option (`path`, `type`,
  `quality`, `full_page`, `clip`, `omit_background`, `animations`, `caret`, `scale`,
  `mask`, `mask_color`, `style`, `timeout`); the page is never brought to front;
* `page.pdf()`, `page.clock()` (Playwright's fake timers), JS/CSS coverage,
  `add_script_tag`/`add_style_tag`, `opener`, `close().reason(..)` /
  `.run_before_unload(true)`, `context.new_cdp_session` and
  `browser.new_browser_cdp_session`.

### Emulation and stealth

Every emulation override is observable by pages, so oxidewright only sends one when the
corresponding option is set. Contexts created with `browser.new_context()` keep Playwright's
default 1280x720 viewport (use `.viewport(None)` to keep the window size); the default
context of `connect_over_cdp` overrides nothing. Unlike Playwright, media features are not
forced to `light`/`no-preference` when unset, so pages see the browser's real preferences.
Context options are applied to the page's own session; out-of-process iframes run in other
renderers and are not emulated separately yet. JS coverage enables the `Debugger` and
`Profiler` domains and CDP sessions share the page session: both are visible to a page that
looks for them.

### Assertions (`expect`)

`oxidewright::expect` ports Playwright Test's web-first assertions (without the test runner).
They retry until they pass or the timeout expires (5 s by default, see
`expect::set_default_timeout`), running the injected script's own `expect()` like Playwright does:

```rust,ignore
use oxidewright::expect::expect;
use oxidewright::regex;

expect(&page.get_by_role("button")).to_be_visible().await?;
expect(&page.locator("#status")).not().to_have_text("Loading").timeout(1_000).await?;
expect(&page.locator("li")).to_have_text(["one", "two"]).await?;   // one entry per element
expect(&page.locator("h1")).to_contain_text(regex("welcome", "i")).await?;
expect(&page).to_have_url("/done").await?;                          // resolved against base_url
expect(&page).message("should be on the home page").to_have_title("Home").await?;
expect(&page.request().get(url).await?).to_be_ok().await?;
```

* Locators: `to_be_attached`, `to_be_checked` (`.checked(..)`, `.indeterminate(..)`),
  `to_be_disabled`, `to_be_editable`, `to_be_empty`, `to_be_enabled`, `to_be_focused`,
  `to_be_hidden`, `to_be_in_viewport` (`.ratio(..)`), `to_be_visible`, `to_contain_text`,
  `to_have_text` (`.use_inner_text(..)`, `.ignore_case(..)`; a string, a regex or a list),
  `to_have_accessible_name` / `_description` / `_error_message`, `to_have_attribute(name, value)`,
  `to_have_attribute_present(name)`, `to_have_class`, `to_contain_class`, `to_have_count`,
  `to_have_css` (`.pseudo(..)`), `to_have_id`, `to_have_js_property`, `to_have_role`,
  `to_have_value`, `to_have_values`, `to_match_aria_snapshot`, `to_have_screenshot`.
* Pages: `to_have_title`, `to_have_url` (string or regex, `.ignore_case(..)`),
  `to_have_url_matching(predicate)`, `to_match_aria_snapshot`, `to_have_screenshot`.
* API responses (`page.request()` / `route.fetch()` responses): `to_be_ok`.
* Failures are `Error::Expect(MatcherResult)` (name, pass, expected, actual, log, timeout, ARIA
  snapshot) and the message mirrors Playwright's, diffs included:

  ```text
  expect(locator).toHaveText(expected) failed

  Locator:  locator('#node')
  Expected: "hey"
  Received: "Text content"
  Timeout:  1000ms

  Call log:
    - Expect "toHaveText" locator('#node') with timeout 1000ms
    - waiting for locator('#node')
    -   locator resolved to <div id="node">Text content</div>
    -   unexpected value "Text content"
  ```

Differences with Playwright Test: no soft assertions, `expect.poll`, `expect.extend`, `toPass`,
`AbortSignal` (drop the future instead) or `URLPattern`. ARIA snapshot templates are parsed with
`yaml-rust2`, so YAML syntax errors are worded differently. `to_have_screenshot` compares
with a PNG at an explicit path using Playwright's `pixelmatch` comparator (`threshold`,
`max_diff_pixels`, `max_diff_pixel_ratio`); a missing file is written and the assertion fails,
and `-actual.png` / `-diff.png` files are written next to it on mismatch. The `ssim-cie94`
comparator, snapshot path templates and `--update-snapshots` modes are not ported.

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
