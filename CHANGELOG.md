# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Breaking Changes

- `Page::screenshot` no longer brings the page to the front before the capture;
  set `ScreenshotParams::activate` (or `ScreenshotParamsBuilder::activate(true)`)
  to keep the old behavior. `ScreenshotParams` gained the public field
  `activate` (mattsse/chromiumoxide#308)
- Operations on a page whose target was closed or destroyed now fail with the new
  `CdpError::TargetClosed` instead of `NoResponse`, `FrameNotReady` or a channel
  send error (mattsse/chromiumoxide#14)
- `Page::wait_for_navigation` and `Page::wait_for_navigation_response` fail with
  `CdpError::Timeout` after the configured request timeout instead of waiting
  forever
- `goto` (and `Browser::new_page(url)`) now resolve on the navigated frame's own
  `load`, as in Playwright: they no longer wait for subframes the browser does not
  fold into that event, such as out-of-process iframes. Use
  `Page::wait_for_navigation_until(LifecycleEvent::NetworkIdle)` or the frame APIs
  to wait for them
- Generated protocol types: every struct and enum now implements `Default`,
  enums implement `Deserialize` by hand, and `NavigationError` has a new
  `Failed` variant

### Added

- `Page::wait_for_navigation_until(LifecycleEvent)`, e.g. to wait for
  `LifecycleEvent::NetworkIdle` (mattsse/chromiumoxide#36); `LifecycleEvent` is
  re-exported at the crate root

### Fixed

- Protocol deserialization is tolerant: missing fields fall back to their
  default (mattsse/chromiumoxide#335), enum values unknown to this protocol
  revision map to the enum's default (or `None` for optional fields), and an
  event whose params cannot be read arrives as `CdpEvent::Other` instead of
  failing the whole message (mattsse/chromiumoxide#266). The approach is ported
  from [spider-rs/chromey](https://github.com/spider-rs/chromey) (Apache-2.0)
- Mandatory arrays are serialized even when empty, so `Browser::set_cookies(vec![])`
  is accepted by the browser; browser-level requests (`Browser::execute`,
  `clear_cookies`, `pages`, `close`, ...) fail with `CdpError::Timeout` instead of
  hanging when no response arrives, and the browser cookie methods use the
  browser's context (mattsse/chromiumoxide#293)
- `goto` completes on the navigated frame's own `load` (a late, lazy or never
  finishing subframe no longer holds it back), fails as soon as the navigation
  request fails (`Network.loadingFailed`) or the frame stops loading without
  committing, and `Frame::goto` uses the configured request timeout
  (mattsse/chromiumoxide#125, mattsse/chromiumoxide#191). `Browser::new_page`
  hands the page over after the request timeout even if its first document never
  fires `load`
- `Page::close` resolves once the target is gone; afterwards pending waiters and
  commands of the page, its frames and elements fail with
  `CdpError::TargetClosed` and its event streams end (mattsse/chromiumoxide#14)
- When the websocket connection ends, the handler settles every pending request,
  refuses new ones and keeps returning `None`; a `Browser::close` whose
  connection drops before the reply succeeds, and the end of the connection after
  `Browser::close` is no longer logged as an error (mattsse/chromiumoxide#65,
  mattsse/chromiumoxide#287)

- Add `add_init_script` to `Page` for scripts before navigation
### Added

- `Page::add_event_listener`, which registers a caller-built `EventListenerRequest` and so lets
  several event methods be received through one channel, in order
### Breaking Changes

- Change `Element::node_id` from `NodeId` to `Option<NodeId>` because
  session-scoped Runtime queries can produce valid elements without a frontend
  node id
- Mark `CdpError` as non-exhaustive and add frame- and interception-specific
  error variants

### Added

- Add session-aware cross-origin iframe (OOPIF) support, including public frame
  APIs, recursive child-session attachment, frame navigation and evaluation,
  DOM element interaction, request interception, preload replay, and
  deterministic detach/teardown handling

### Changed

- Derive `Clone` for `Element`, `ScreenshotParams` and `ScreenshotParamsBuilder`
### Added

- Add `Element::call_js_fn_with_args` for puppeteer-style argument passing via CDP `arguments`

### Fixed

- Events of different methods subscribed through one channel are no longer regrouped by method, so
  they arrive in the order the session delivered them in

## [0.9.1] 2026-02-25

### Fixed

- Invalid import when using zip0

## [0.9.0] 2026-02-20

### Breaking Changes

- Removed async-std support

### Changed

- Updated edition to 2024
- Bump fetcher chromium to `r1585606`
- Bump CDP to `r1566079`
- Use a struct `Arg` for arguments to combine flags automatically
- Browser process no longer inherits stdout
- Update `reqwest` to v0.13
- Update `thiserror` to v2
- Update `heck` to v0.5
- Add support for `zip` v8 and make it default

### Added

- Add `Page::element_from_node_id` to create an `Element` from a DOM `NodeId`
- Add `click_with` to `Page` and `Element` for custom click behavior

## [0.8.0] 2025-11-28

### Breaking Changes

Due to the support of new browsers in the fetcher, we changed the API.
The changes are mainly related to how you can request a specific version of the browser.
Previously you could only request a chromium revision, now we support chrome versions, channel and milestone.

```rust
BrowserFetcherOptions::builder()
  .with_kind(BrowserKind::Chrome)
  .with_version(BrowserVersion::Channel(Channel::Beta))
  .build()
```

We also change the output format of the `fetch`. It is now called a `BrowserFetcherInstallation`
and contains a `BuildInfo`. We garantee that at least the version or revision will be present
in that struct, but not always both.

```rust
let installation = chromiumoxide_fetcher::BrowserFetcher::new(options).fetch().await?;
println!("Executable path: {}", installation.executable_path.display());
```

Finally, we switched the async runtime tokio by default. We will remove for support for async-std in the next release.
If you want to part of the discussion on other runtime support, see [#273](https://github.com/mattsse/chromiumoxide/issues/273).

### Changed

- Bumped MSRV to 1.85 to support edition 2024
- Update `async-tungstenite` to 0.32.0
- Update `which` to 8.0.0
- Replace `winreg` by `windows-registry`
- Updated PDL to r1519099 (Chromium 142.0.7431.0)
- Updated fetcher to r1520176 (Chromium 142.0.7435.0)
- Fetch now supports `Chrome for testing` and `Chrome Headless Shell`
- Now uses tokio by default

### Added

- Add option to disable automation detection
- Expose the `cmd` module for access to `CommandChain`

### Fixed

- Fixed typo in feature `_fetcher-rustls-tokio`
- More resilient message parsing, it should now not crash on unknown events coming from the browser
- Extensions should only be disabled when no extensions are provided

[Unreleased]: https://github.com/mattsse/chromiumoxide/compare/v0.7.0...HEAD
