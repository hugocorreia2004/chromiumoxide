//! Ported from Playwright's `tests/library/browser.spec.ts`,
//! `browsertype-basic.spec.ts`, `browsertype-launch.spec.ts`,
//! `defaultbrowsercontext-{1,2}.spec.ts`, `browsercontext-events.spec.ts`,
//! `slowmo.spec.ts` and `tests/page/page-dialog.spec.ts` (the Browser,
//! BrowserType and BrowserContext APIs).

use std::future::IntoFuture;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use oxidewright::{Browser, BrowserContext, EventKind, LaunchOptions, ViewportSize};

use crate::common::run;

fn launch() -> oxidewright::Launch {
    let mut launch = oxidewright::chromium().launch();
    if let Ok(path) = std::env::var("OXIDEWRIGHT_CHROME") {
        launch = launch.executable_path(path);
    }
    launch
}

fn launch_persistent(dir: &std::path::Path) -> oxidewright::LaunchPersistentContext {
    let mut launch = oxidewright::chromium().launch_persistent_context(dir);
    if let Ok(path) = std::env::var("OXIDEWRIGHT_CHROME") {
        launch = launch.executable_path(path);
    }
    launch
}

/// A fresh, empty profile directory (removed when dropped).
struct UserDataDir(std::path::PathBuf);

impl UserDataDir {
    fn new() -> Self {
        let name: String = (0..12).map(|_| (b'a' + rand_u8() % 26) as char).collect();
        let dir = std::env::temp_dir().join(format!("oxidewright-test-profile-{name}"));
        std::fs::create_dir_all(&dir).unwrap();
        UserDataDir(dir)
    }
}

fn rand_u8() -> u8 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64,
    );
    h.finish() as u8
}

impl Drop for UserDataDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// ----------------------------------------------------------------------
// browser.spec.ts
// ----------------------------------------------------------------------

#[test]
fn br_001_should_return_browser_type() {
    run(|t| async move {
        assert_eq!(t.browser.browser_type(), oxidewright::chromium());
        assert_eq!(t.browser.browser_type().name(), "chromium");
    });
}

#[test]
fn br_002_should_create_new_page() {
    run(|t| async move {
        // The harness context is already open.
        let base = t.browser.contexts().len();
        let page1 = t.browser.new_page().await.unwrap();
        assert_eq!(t.browser.contexts().len(), base + 1);
        let page2 = t.browser.new_page().await.unwrap();
        assert_eq!(t.browser.contexts().len(), base + 2);
        page1.close().await.unwrap();
        assert_eq!(t.browser.contexts().len(), base + 1);
        page2.close().await.unwrap();
        assert_eq!(t.browser.contexts().len(), base);
    });
}

#[test]
fn br_003_should_throw_upon_second_create_new_page() {
    run(|t| async move {
        let page = t.browser.new_page().await.unwrap();
        let error = page.context().unwrap().new_page().await.unwrap_err();
        page.close().await.unwrap();
        assert!(
            error
                .to_string()
                .contains("Please use browser.newContext()"),
            "{error}"
        );
    });
}

#[test]
fn br_005_should_dispatch_page_on_close_upon_browser_close_and_reject_evaluate() {
    run(|_t| async move {
        let browser = launch().await.unwrap();
        let page = browser.new_page().await.unwrap();
        let closed = Arc::new(Mutex::new(false));
        let c = closed.clone();
        page.on_close(move |_| *c.lock().unwrap() = true);
        let pending = {
            let page = page.clone();
            tokio::spawn(async move {
                page.evaluate::<()>("() => new Promise(() => {})")
                    .await
                    .unwrap_err()
            })
        };
        tokio::time::sleep(Duration::from_millis(100)).await;
        browser.close().await.unwrap();
        let error = pending.await.unwrap();
        assert!(error.to_string().contains("closed"), "{error}");
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(*closed.lock().unwrap());
    });
}

#[test]
fn br_006_should_fire_context_event_on_new_context() {
    run(|t| async move {
        let events: Arc<Mutex<Vec<BrowserContext>>> = Arc::default();
        let e = events.clone();
        t.browser
            .on_context(move |context| e.lock().unwrap().push(context));
        let context = t.browser.new_context().await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(*events.lock().unwrap(), vec![context.clone()]);
        context.close().await.unwrap();
    });
}

#[test]
fn new_page_should_accept_context_options() {
    run(|t| async move {
        let page = t
            .browser
            .new_page()
            .viewport(Some(ViewportSize {
                width: 456,
                height: 345,
            }))
            .user_agent("foobar")
            .await
            .unwrap();
        assert_eq!(page.evaluate::<i64>("() => innerWidth").await.unwrap(), 456);
        assert_eq!(
            page.evaluate::<String>("() => navigator.userAgent")
                .await
                .unwrap(),
            "foobar"
        );
        page.close().await.unwrap();
    });
}

#[test]
fn browser_close_should_accept_a_reason() {
    run(|_t| async move {
        let browser = launch().await.unwrap();
        let context = browser.new_context().await.unwrap();
        browser.close().reason("Shutting down").await.unwrap();
        assert!(context.is_closed());
        assert_eq!(context.close_reason().as_deref(), Some("Shutting down"));
    });
}

#[test]
fn browser_should_emit_disconnected_and_remove_listeners() {
    run(|_t| async move {
        let browser = launch().await.unwrap();
        let fired = Arc::new(Mutex::new(0));
        let f = fired.clone();
        browser.on_disconnected(move |b: Browser| {
            assert!(!b.is_connected());
            *f.lock().unwrap() += 1;
        });
        let removed = Arc::new(Mutex::new(0));
        let r = removed.clone();
        browser.on_context(move |_| *r.lock().unwrap() += 1);
        browser.remove_all_listeners(Some(EventKind::Context));
        let disconnected = browser.wait_for_event(EventKind::Disconnected);
        let _ = browser.new_context().await.unwrap();
        browser.close().await.unwrap();
        disconnected.await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(*fired.lock().unwrap(), 1);
        assert_eq!(*removed.lock().unwrap(), 0);
    });
}

// ----------------------------------------------------------------------
// browsertype-basic.spec.ts / browsertype-launch.spec.ts
// ----------------------------------------------------------------------

#[test]
fn bt_001_browser_type_executable_path_should_work() {
    if std::env::var("OXIDEWRIGHT_CHROME").is_ok() {
        return; // Skip with custom executable path, like Playwright.
    }
    let path = oxidewright::chromium().executable_path().unwrap();
    assert!(path.exists(), "{}", path.display());
}

#[test]
fn bt_002_browser_type_name_should_work() {
    assert_eq!(oxidewright::chromium().name(), "chromium");
}

#[test]
fn bt_006_should_throw_if_user_data_dir_is_passed_as_an_argument() {
    run(|_t| async move {
        let error = launch()
            .args(["--user-data-dir=random-path", "--profile=random-path"])
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Pass userDataDir parameter to 'browserType.launchPersistentContext"),
            "{error}"
        );
    });
}

#[test]
fn bt_009_should_throw_if_page_argument_is_passed() {
    run(|_t| async move {
        let error = launch().args(["http://example.com"]).await.unwrap_err();
        assert!(
            error.to_string().contains("can not specify page"),
            "{error}"
        );
    });
}

#[test]
fn bt_011_should_reject_if_executable_path_is_invalid() {
    run(|_t| async move {
        let error = oxidewright::chromium()
            .launch()
            .executable_path("random-invalid-path")
            .await
            .unwrap_err();
        assert!(error.to_string().contains("Failed to launch"), "{error}");
    });
}

#[test]
fn bt_015_should_fire_close_event_for_all_contexts() {
    run(|_t| async move {
        let browser = launch().await.unwrap();
        let context = browser.new_context().await.unwrap();
        let closed = Arc::new(Mutex::new(false));
        let c = closed.clone();
        context.on_close(move |_| *c.lock().unwrap() = true);
        browser.close().await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(*closed.lock().unwrap());
    });
}

#[test]
fn bt_016_should_be_callable_twice() {
    run(|_t| async move {
        let browser = launch().await.unwrap();
        let (a, b) = tokio::join!(browser.close().into_future(), browser.close().into_future());
        a.unwrap();
        b.unwrap();
        browser.close().await.unwrap();
    });
}

#[test]
fn launch_should_reject_unknown_channels() {
    run(|_t| async move {
        let error = oxidewright::chromium()
            .launch()
            .channel("firefox")
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("Unsupported chromium channel"),
            "{error}"
        );
    });
}

#[test]
fn launch_should_ignore_listed_default_args() {
    run(|t| async move {
        // `--mute-audio` and `--hide-scrollbars` are default headless switches;
        // without `--hide-scrollbars` a scrollable page shows scrollbars.
        let browser = launch()
            .ignore_default_args(["--hide-scrollbars"])
            .downloads_path(std::env::temp_dir().join("oxidewright-test-downloads"))
            .handle_sigint(false)
            .await
            .unwrap();
        let page = browser.new_page().await.unwrap();
        page.goto(&t.server.empty_page).await.unwrap();
        let width: i64 = page
            .evaluate(
                "() => { document.body.style.height = '5000px'; \
                 return window.innerWidth - document.documentElement.clientWidth; }",
            )
            .await
            .unwrap();
        assert!(width > 0, "no scrollbar");
        browser.close().await.unwrap();
    });
}

// ----------------------------------------------------------------------
// slowmo.spec.ts
// ----------------------------------------------------------------------

#[test]
fn sl_002_page_slow_mo_click() {
    run(|_t| async move {
        let browser = launch().slow_mo(400).await.unwrap();
        let page = browser.new_page().await.unwrap();
        page.set_content("<button>a</button>").await.unwrap();
        let start = Instant::now();
        page.click("button").await.unwrap();
        assert!(start.elapsed() >= Duration::from_millis(400));
        let start = Instant::now();
        page.mouse().r#move(1, 1).await.unwrap();
        assert!(start.elapsed() >= Duration::from_millis(400));
        browser.close().await.unwrap();
    });
}

#[test]
fn sl_007_page_slow_mo_goto() {
    run(|t| async move {
        let browser = launch().slow_mo(400).await.unwrap();
        let page = browser.new_page().await.unwrap();
        let start = Instant::now();
        page.goto(&t.server.empty_page).await.unwrap();
        assert!(start.elapsed() >= Duration::from_millis(400));
        // Queries are not slowed down.
        let start = Instant::now();
        page.title().await.unwrap();
        assert!(start.elapsed() < Duration::from_millis(400));
        browser.close().await.unwrap();
    });
}

// ----------------------------------------------------------------------
// defaultbrowsercontext-{1,2}.spec.ts (launch_persistent_context)
// ----------------------------------------------------------------------

#[test]
fn dc_004_should_support_viewport_option() {
    run(|_t| async move {
        let dir = UserDataDir::new();
        let context = launch_persistent(&dir.0)
            .viewport(Some(ViewportSize {
                width: 456,
                height: 789,
            }))
            .await
            .unwrap();
        let page = context.pages()[0].clone();
        assert_eq!(page.evaluate::<i64>("() => innerWidth").await.unwrap(), 456);
        assert_eq!(
            page.evaluate::<i64>("() => innerHeight").await.unwrap(),
            789
        );
        let page = context.new_page().await.unwrap();
        assert_eq!(page.evaluate::<i64>("() => innerWidth").await.unwrap(), 456);
        context.close().await.unwrap();
    });
}

#[test]
fn dc_006_should_support_user_agent_option() {
    run(|t| async move {
        let dir = UserDataDir::new();
        let context = launch_persistent(&dir.0)
            .user_agent("foobar")
            .await
            .unwrap();
        let page = context.pages()[0].clone();
        assert_eq!(
            page.evaluate::<String>("() => navigator.userAgent")
                .await
                .unwrap(),
            "foobar"
        );
        let request = t.server.wait_for_request("/empty.html");
        let (request, _) = tokio::join!(request, page.goto(&t.server.empty_page));
        assert_eq!(request.header("user-agent").as_deref(), Some("foobar"));
        context.close().await.unwrap();
    });
}

#[test]
fn dc_019_should_support_locale_option() {
    run(|_t| async move {
        let dir = UserDataDir::new();
        let context = launch_persistent(&dir.0).locale("fr-FR").await.unwrap();
        let page = context.pages()[0].clone();
        assert_eq!(
            page.evaluate::<String>("() => navigator.language")
                .await
                .unwrap(),
            "fr-FR"
        );
        context.close().await.unwrap();
    });
}

#[test]
fn dc_023_should_accept_user_data_dir() {
    run(|_t| async move {
        let dir = UserDataDir::new();
        let context = launch_persistent(&dir.0).await.unwrap();
        assert!(std::fs::read_dir(&dir.0).unwrap().count() > 0);
        context.close().await.unwrap();
        assert!(std::fs::read_dir(&dir.0).unwrap().count() > 0);
    });
}

#[test]
fn dc_025_should_restore_state_from_user_data_dir() {
    run(|t| async move {
        let dir = UserDataDir::new();
        let context = launch_persistent(&dir.0).await.unwrap();
        let page = context.new_page().await.unwrap();
        page.goto(&t.server.empty_page).await.unwrap();
        page.evaluate::<()>("() => { localStorage.hey = 'hello' }")
            .await
            .unwrap();
        page.goto(&t.server.empty_page).await.unwrap();
        context.close().await.unwrap();

        let context = launch_persistent(&dir.0).await.unwrap();
        let page = context.new_page().await.unwrap();
        page.goto(&t.server.empty_page).await.unwrap();
        assert_eq!(
            page.evaluate::<Option<String>>("() => localStorage.hey")
                .await
                .unwrap()
                .as_deref(),
            Some("hello")
        );
        context.close().await.unwrap();
    });
}

#[test]
fn dc_025_should_restore_state_from_user_data_dir_but_not_another_one() {
    // The last part of `DC-025`, split out to fit the per-test deadline.
    run(|t| async move {
        let dir = UserDataDir::new();
        let context = launch_persistent(&dir.0).await.unwrap();
        let page = context.new_page().await.unwrap();
        page.goto(&t.server.empty_page).await.unwrap();
        page.evaluate::<()>("() => { localStorage.hey = 'hello' }")
            .await
            .unwrap();
        context.close().await.unwrap();

        let dir2 = UserDataDir::new();
        let context = launch_persistent(&dir2.0).await.unwrap();
        let page = context.new_page().await.unwrap();
        page.goto(&t.server.empty_page).await.unwrap();
        assert_ne!(
            page.evaluate::<Option<String>>("() => localStorage.hey")
                .await
                .unwrap()
                .as_deref(),
            Some("hello")
        );
        context.close().await.unwrap();
    });
}

#[test]
fn dc_026_should_create_user_data_dir_if_it_does_not_exist() {
    run(|_t| async move {
        let dir = UserDataDir::new();
        let missing = dir.0.join("nonexisting");
        let context = launch_persistent(&missing).await.unwrap();
        context.close().await.unwrap();
        assert!(std::fs::read_dir(&missing).unwrap().count() > 0);
    });
}

#[test]
fn dc_028_should_have_default_url_when_launching_browser() {
    run(|_t| async move {
        let dir = UserDataDir::new();
        let context = launch_persistent(&dir.0).await.unwrap();
        let urls: Vec<String> = context.pages().iter().map(|p| p.url()).collect();
        assert_eq!(urls, vec!["about:blank".to_string()]);
        context.close().await.unwrap();
    });
}

#[test]
fn dc_029_should_throw_if_page_argument_is_passed() {
    run(|t| async move {
        let dir = UserDataDir::new();
        let error = launch_persistent(&dir.0)
            .args([t.server.empty_page.clone()])
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("can not specify page"),
            "{error}"
        );
    });
}

#[test]
fn dc_033_should_fire_close_event_for_a_persistent_context() {
    run(|_t| async move {
        let dir = UserDataDir::new();
        let context = launch_persistent(&dir.0).await.unwrap();
        let browser = context.browser().unwrap();
        let closed = Arc::new(Mutex::new(false));
        let c = closed.clone();
        context.on_close(move |_| *c.lock().unwrap() = true);
        context.close().await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(*closed.lock().unwrap());
        // Closing the persistent context closes the browser.
        assert!(!browser.is_connected());
    });
}

#[test]
fn dc_041_exposes_browser() {
    run(|_t| async move {
        let dir = UserDataDir::new();
        let context = launch_persistent(&dir.0).await.unwrap();
        let browser = context.browser().unwrap();
        assert!(!browser.version().is_empty());
        assert!(browser.contexts().contains(&context));
        let page = browser.new_page().await.unwrap();
        page.goto("data:text/html,<html><title>Title</title></html>")
            .await
            .unwrap();
        assert_eq!(page.title().await.unwrap(), "Title");
        browser.close().await.unwrap();
        assert_eq!(context.pages().len(), 0);
        // Next line should not throw.
        context.close().await.unwrap();
    });
}

#[test]
fn persistent_context_should_accept_launch_options_struct() {
    run(|_t| async move {
        let dir = UserDataDir::new();
        let mut options = LaunchOptions::default();
        if let Ok(path) = std::env::var("OXIDEWRIGHT_CHROME") {
            options.executable_path = Some(path.into());
        }
        let context = oxidewright::chromium()
            .launch_persistent_context(&dir.0)
            .launch_options(options)
            .await
            .unwrap();
        assert_eq!(context.pages().len(), 1);
        context.close().await.unwrap();
    });
}

// ----------------------------------------------------------------------
// browsercontext-events.spec.ts
// ----------------------------------------------------------------------

#[test]
fn bc_038_dialogclosed_event_should_work() {
    run(|t| async move {
        let page = t.page.clone();
        let dialog = t.context.wait_for_dialog();
        let promise = {
            let page = page.clone();
            tokio::spawn(async move {
                page.evaluate::<Option<String>>("() => prompt('hey?')")
                    .await
                    .unwrap()
            })
        };
        let dialog = dialog.await.unwrap();
        let closed1 = t.context.wait_for_dialog_closed();
        let closed2 = page.wait_for_dialog_closed();
        dialog.accept(Some("hello")).await.unwrap();
        assert_eq!(closed1.await.unwrap(), dialog);
        assert_eq!(closed2.await.unwrap(), dialog);
        assert_eq!(promise.await.unwrap().as_deref(), Some("hello"));
    });
}

#[test]
fn bc_045_pageload_event_should_work() {
    run(|t| async move {
        let event = t.context.wait_for_event(EventKind::PageLoad);
        t.page.goto(&t.server.empty_page).await.unwrap();
        match event.await.unwrap() {
            oxidewright::Event::PageLoad(page) => assert_eq!(page, t.page),
            other => panic!("unexpected {other:?}"),
        }
    });
}

#[test]
fn bc_046_framenavigated_event_should_work() {
    run(|t| async move {
        let event = t.context.wait_for_event(EventKind::FrameNavigated);
        t.page.goto(&t.server.empty_page).await.unwrap();
        match event.await.unwrap() {
            oxidewright::Event::FrameNavigated(frame) => {
                assert_eq!(frame, t.page.main_frame());
                assert_eq!(frame.url(), t.server.empty_page);
            }
            other => panic!("unexpected {other:?}"),
        }
    });
}

#[test]
fn bc_047_pageclose_event_should_work() {
    run(|t| async move {
        let page = t.context.new_page().await.unwrap();
        let event = t.context.wait_for_event(EventKind::PageClose);
        page.close().await.unwrap();
        match event.await.unwrap() {
            oxidewright::Event::PageClose(closed) => assert_eq!(closed, page),
            other => panic!("unexpected {other:?}"),
        }
    });
}

#[test]
fn context_remove_all_listeners_should_work() {
    run(|t| async move {
        let count = Arc::new(Mutex::new(0));
        let c = count.clone();
        t.context.on_page_load(move |_| *c.lock().unwrap() += 1);
        t.context.remove_all_listeners(None);
        t.page.goto(&t.server.empty_page).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(*count.lock().unwrap(), 0);
    });
}

// ----------------------------------------------------------------------
// page-dialog.spec.ts
// ----------------------------------------------------------------------

#[test]
fn di_002_should_fire_dialogclosed_when_dialog_is_accepted() {
    run(|t| async move {
        let closed: Arc<Mutex<Vec<oxidewright::Dialog>>> = Arc::default();
        let opened: Arc<Mutex<Option<oxidewright::Dialog>>> = Arc::default();
        let c = closed.clone();
        t.page.on_dialog_closed(move |d| c.lock().unwrap().push(d));
        let o = opened.clone();
        t.page.on_dialog(move |dialog| {
            *o.lock().unwrap() = Some(dialog.clone());
            tokio::spawn(async move { dialog.accept(None).await.unwrap() });
        });
        t.page.evaluate::<()>("() => alert('yo')").await.unwrap();
        for _ in 0..50 {
            if !closed.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(closed.lock().unwrap().len(), 1);
        assert_eq!(
            Some(closed.lock().unwrap()[0].clone()),
            *opened.lock().unwrap()
        );
        // Perform some roundtrips to ensure the event does not fire twice.
        t.page.evaluate::<i32>("() => 1").await.unwrap();
        t.page.evaluate::<i32>("() => 1").await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(closed.lock().unwrap().len(), 1);
    });
}

#[test]
fn di_003_should_fire_dialogclosed_when_dialog_is_dismissed() {
    run(|t| async move {
        let closed = t.page.wait_for_dialog_closed();
        t.page.on_dialog(|dialog| {
            tokio::spawn(async move { dialog.dismiss().await.unwrap() });
        });
        t.page
            .evaluate::<bool>("() => confirm('boolean?')")
            .await
            .unwrap();
        let dialog = closed.await.unwrap();
        assert_eq!(dialog.r#type(), "confirm");
        assert_eq!(dialog.message(), "boolean?");
    });
}

#[test]
fn di_004_should_fire_dialogclosed_for_auto_dismissed_dialogs() {
    run(|t| async move {
        let closed = t.page.wait_for_dialog_closed();
        t.page.evaluate::<()>("() => alert('yo')").await.unwrap();
        let dialog = closed.await.unwrap();
        assert_eq!(dialog.message(), "yo");
    });
}
