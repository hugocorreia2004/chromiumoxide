//! Port of `page-add-locator-handler.spec.ts`.
//!
//! `tests/assets/input/handle-locator.html` is a small rewrite of
//! Playwright's asset (an interstitial shown over `#target`).
//!
//! Skipped: LH-008 (`toHaveScreenshot` against a golden image).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use oxidewright::GetByRoleOptions;
use oxidewright::expect::expect;
use serde_json::json;

use crate::common::*;

async fn setup(t: &TestContext, args: serde_json::Value) {
    t.page
        .evaluate::<()>(
            "args => { window.clicked = 0; window.setupAnnoyingInterstitial(...args); }",
        )
        .arg(args)
        .await
        .unwrap();
}

async fn clicked(t: &TestContext) -> Option<i64> {
    t.page.evaluate("() => window.clicked").await.unwrap()
}

fn close_button(page: &oxidewright::Page) -> oxidewright::Locator {
    page.get_by_role_with("button", GetByRoleOptions::new().name("close"))
}

#[test]
fn lh_001_should_work() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/handle-locator.html", t.server.prefix))
            .await
            .unwrap();
        let before = Arc::new(AtomicUsize::new(0));
        let after = Arc::new(AtomicUsize::new(0));
        let original = t.page.get_by_text("This interstitial covers the button");
        let (b, a, page, expected) = (
            before.clone(),
            after.clone(),
            t.page.clone(),
            original.clone(),
        );
        t.page
            .add_locator_handler(&original, move |locator| {
                assert_eq!(locator, expected);
                let (b, a, page) = (b.clone(), a.clone(), page.clone());
                async move {
                    b.fetch_add(1, Ordering::SeqCst);
                    page.locator("#close").click().await.unwrap();
                    a.fetch_add(1, Ordering::SeqCst);
                }
            })
            .await
            .unwrap();
        for (args, times) in [
            (json!(["mouseover", 1]), 1),
            (json!(["mouseover", 1, "capture"]), 1),
            (json!(["pointerover", 1]), 1),
            (json!(["pointerover", 1, "capture"]), 1),
            (json!(["none", 1]), 1),
            (json!(["remove", 1]), 1),
            (json!(["hide", 1]), 1),
        ] {
            t.page.locator("#aside").hover().await.unwrap();
            before.store(0, Ordering::SeqCst);
            after.store(0, Ordering::SeqCst);
            setup(&t, args.clone()).await;
            assert_eq!(before.load(Ordering::SeqCst), 0);
            t.page.locator("#target").click().await.unwrap();
            assert_eq!(before.load(Ordering::SeqCst), times, "{args}");
            assert_eq!(after.load(Ordering::SeqCst), times, "{args}");
            assert_eq!(clicked(&t).await, Some(1), "{args}");
            expect(&t.page.locator("#interstitial"))
                .not()
                .to_be_visible()
                .await
                .unwrap();
        }
    });
}

#[test]
fn lh_002_should_work_with_a_custom_check() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/handle-locator.html", t.server.prefix))
            .await
            .unwrap();
        let page = t.page.clone();
        t.page
            .add_locator_handler(&t.page.locator("body"), move |_| {
                let page = page.clone();
                async move {
                    if page
                        .get_by_text("This interstitial covers the button")
                        .is_visible()
                        .await
                        .unwrap()
                    {
                        page.locator("#close").click().await.unwrap();
                    }
                }
            })
            .no_wait_after(true)
            .await
            .unwrap();
        for args in [
            json!(["mouseover", 2]),
            json!(["none", 1]),
            json!(["remove", 1]),
            json!(["hide", 1]),
        ] {
            t.page.locator("#aside").hover().await.unwrap();
            setup(&t, args.clone()).await;
            t.page.locator("#target").click().await.unwrap();
            assert_eq!(clicked(&t).await, Some(1), "{args}");
            expect(&t.page.locator("#interstitial"))
                .not()
                .to_be_visible()
                .await
                .unwrap();
        }
    });
}

#[test]
fn lh_003_should_work_with_locator_hover() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/handle-locator.html", t.server.prefix))
            .await
            .unwrap();
        let page = t.page.clone();
        t.page
            .add_locator_handler(
                &t.page.get_by_text("This interstitial covers the button"),
                move |_| {
                    let page = page.clone();
                    async move { page.locator("#close").click().await }
                },
            )
            .await
            .unwrap();
        t.page.locator("#aside").hover().await.unwrap();
        setup(&t, json!(["pointerover", 1, "capture"])).await;
        t.page.locator("#target").hover().await.unwrap();
        expect(&t.page.locator("#interstitial"))
            .not()
            .to_be_visible()
            .await
            .unwrap();
        let color: String = t
            .page
            .eval_on_selector("#target", "e => window.getComputedStyle(e).backgroundColor")
            .await
            .unwrap();
        assert_eq!(color, "rgb(255, 255, 0)");
    });
}

#[test]
fn lh_004_should_not_work_with_force_true() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/handle-locator.html", t.server.prefix))
            .await
            .unwrap();
        let page = t.page.clone();
        t.page
            .add_locator_handler(
                &t.page.get_by_text("This interstitial covers the button"),
                move |_| {
                    let page = page.clone();
                    async move { page.locator("#close").click().await }
                },
            )
            .await
            .unwrap();
        t.page.locator("#aside").hover().await.unwrap();
        t.page
            .evaluate::<()>("() => window.setupAnnoyingInterstitial('none', 1)")
            .await
            .unwrap();
        t.page
            .locator("#target")
            .click()
            .force(true)
            .timeout(2000)
            .await
            .unwrap();
        assert!(t.page.locator("#interstitial").is_visible().await.unwrap());
        assert_eq!(clicked(&t).await, None);
    });
}

#[test]
fn lh_005_should_throw_when_handler_times_out() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/handle-locator.html", t.server.prefix))
            .await
            .unwrap();
        let called = Arc::new(AtomicUsize::new(0));
        let release = Arc::new(tokio::sync::Notify::new());
        let (finished_tx, finished_rx) = tokio::sync::oneshot::channel::<bool>();
        let finished_tx = Arc::new(std::sync::Mutex::new(Some(finished_tx)));
        let (c, r, page) = (called.clone(), release.clone(), t.page.clone());
        t.page
            .add_locator_handler(
                &t.page.get_by_text("This interstitial covers the button"),
                move |_| {
                    let (c, r, page, tx) =
                        (c.clone(), r.clone(), page.clone(), finished_tx.clone());
                    async move {
                        c.fetch_add(1, Ordering::SeqCst);
                        r.notified().await;
                        let ok = page.locator("#close").click().timeout(3000).await.is_ok();
                        if let Some(tx) = tx.lock().unwrap().take() {
                            let _ = tx.send(ok);
                        }
                    }
                },
            )
            .await
            .unwrap();
        t.page.locator("#aside").hover().await.unwrap();
        setup(&t, json!(["mouseover", 1])).await;
        let r = t.page.locator("#target").click().timeout(3000).await;
        assert_err_contains(r, "Timeout 3000ms exceeded");
        let r = t.page.locator("#target").click().timeout(3000).await;
        assert_err_contains(r, "Timeout 3000ms exceeded");
        // Should not enter the same handler while it is still running.
        assert_eq!(called.load(Ordering::SeqCst), 1);
        release.notify_one();
        assert!(finished_rx.await.unwrap());
    });
}

#[test]
fn lh_006_should_work_with_to_be_visible() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/handle-locator.html", t.server.prefix))
            .await
            .unwrap();
        let called = Arc::new(AtomicUsize::new(0));
        let (c, page) = (called.clone(), t.page.clone());
        t.page
            .add_locator_handler(
                &t.page.get_by_text("This interstitial covers the button"),
                move |_| {
                    let (c, page) = (c.clone(), page.clone());
                    async move {
                        c.fetch_add(1, Ordering::SeqCst);
                        page.locator("#close").click().await
                    }
                },
            )
            .await
            .unwrap();
        setup(&t, json!(["remove", 1])).await;
        expect(&t.page.locator("#target"))
            .to_be_visible()
            .await
            .unwrap();
        expect(&t.page.locator("#interstitial"))
            .not()
            .to_be_visible()
            .await
            .unwrap();
        assert_eq!(called.load(Ordering::SeqCst), 1);
    });
}

#[test]
fn lh_007_should_work_with_locator_wait_for() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/handle-locator.html", t.server.prefix))
            .await
            .unwrap();
        let called = Arc::new(AtomicUsize::new(0));
        let (c, page) = (called.clone(), t.page.clone());
        t.page
            .add_locator_handler(
                &t.page.get_by_text("This interstitial covers the button"),
                move |_| {
                    let (c, page) = (c.clone(), page.clone());
                    async move {
                        c.fetch_add(1, Ordering::SeqCst);
                        page.locator("#close").click().await
                    }
                },
            )
            .await
            .unwrap();
        setup(&t, json!(["remove", 1])).await;
        t.page.locator("#target").wait_for().await.unwrap();
        expect(&t.page.locator("#interstitial"))
            .not()
            .to_be_visible()
            .await
            .unwrap();
        assert_eq!(called.load(Ordering::SeqCst), 1);
    });
}

#[test]
fn lh_009_should_work_when_owner_frame_detaches() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.page
            .evaluate::<()>(
                "() => { const iframe = document.createElement('iframe'); iframe.src = 'data:text/html,<body>hello from iframe</body>'; document.body.append(iframe); const target = document.createElement('button'); target.textContent = 'Click me'; target.id = 'target'; target.addEventListener('click', () => window._clicked = true); document.body.appendChild(target); const closeButton = document.createElement('button'); closeButton.textContent = 'close'; closeButton.id = 'close'; closeButton.addEventListener('click', () => iframe.remove()); document.body.appendChild(closeButton); }",
            )
            .await
            .unwrap();
        let page = t.page.clone();
        t.page
            .add_locator_handler(&t.page.frame_locator("iframe").locator("body"), move |_| {
                let page = page.clone();
                async move { page.locator("#close").click().await }
            })
            .await
            .unwrap();
        t.page.locator("#target").click().await.unwrap();
        assert!(t.page.query_selector("iframe").await.unwrap().is_none());
        let clicked: bool = t.page.evaluate("() => window._clicked").await.unwrap();
        assert!(clicked);
    });
}

#[test]
fn lh_010_should_work_with_times_option() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/handle-locator.html", t.server.prefix))
            .await
            .unwrap();
        let called = Arc::new(AtomicUsize::new(0));
        let c = called.clone();
        t.page
            .add_locator_handler(&t.page.locator("body"), move |_| {
                let c = c.clone();
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                }
            })
            .no_wait_after(true)
            .times(2)
            .await
            .unwrap();
        t.page.locator("#aside").hover().await.unwrap();
        setup(&t, json!(["mouseover", 4])).await;
        let r = t.page.locator("#target").click().timeout(3000).await;
        assert_eq!(called.load(Ordering::SeqCst), 2);
        assert_eq!(clicked(&t).await, Some(0));
        expect(&t.page.locator("#interstitial"))
            .to_be_visible()
            .await
            .unwrap();
        let e = assert_err_contains(r, "Timeout 3000ms exceeded");
        assert!(
            e.to_string().contains(
                "<div>This interstitial covers the button</div> from <div class=\"visible\" id=\"interstitial\">\u{2026}</div> subtree intercepts pointer events"
            ),
            "{e}"
        );
    });
}

#[test]
fn lh_011_should_wait_for_hidden_by_default() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/handle-locator.html", t.server.prefix))
            .await
            .unwrap();
        let called = Arc::new(AtomicUsize::new(0));
        let c = called.clone();
        t.page
            .add_locator_handler(&close_button(&t.page), move |button| {
                let c = c.clone();
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    button.click().await
                }
            })
            .await
            .unwrap();
        t.page.locator("#aside").hover().await.unwrap();
        setup(&t, json!(["timeout", 1])).await;
        t.page.locator("#target").click().await.unwrap();
        assert_eq!(clicked(&t).await, Some(1));
        expect(&t.page.locator("#interstitial"))
            .not()
            .to_be_visible()
            .await
            .unwrap();
        assert_eq!(called.load(Ordering::SeqCst), 1);
    });
}

#[test]
fn lh_012_should_wait_for_hidden_by_default_2() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/handle-locator.html", t.server.prefix))
            .await
            .unwrap();
        let called = Arc::new(AtomicUsize::new(0));
        let c = called.clone();
        t.page
            .add_locator_handler(&close_button(&t.page), move |_| {
                let c = c.clone();
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                }
            })
            .await
            .unwrap();
        t.page.locator("#aside").hover().await.unwrap();
        setup(&t, json!(["hide", 1])).await;
        let r = t.page.locator("#target").click().timeout(3000).await;
        assert_eq!(clicked(&t).await, Some(0));
        assert!(t.page.locator("#interstitial").is_visible().await.unwrap());
        assert_eq!(called.load(Ordering::SeqCst), 1);
        assert_err_contains(
            r,
            "locator handler has finished, waiting for getByRole('button', { name: 'close' }) to be hidden",
        );
    });
}

#[test]
fn lh_013_should_work_with_no_wait_after() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/handle-locator.html", t.server.prefix))
            .await
            .unwrap();
        let called = Arc::new(AtomicUsize::new(0));
        let (c, page) = (called.clone(), t.page.clone());
        t.page
            .add_locator_handler(&close_button(&t.page), move |button| {
                let (c, page) = (c.clone(), page.clone());
                async move {
                    let n = c.fetch_add(1, Ordering::SeqCst) + 1;
                    if n == 1 {
                        button.click().await.unwrap();
                    } else {
                        page.locator("#interstitial")
                            .wait_for()
                            .state(oxidewright::WaitForSelectorState::Hidden)
                            .await
                            .unwrap();
                    }
                }
            })
            .no_wait_after(true)
            .await
            .unwrap();
        t.page.locator("#aside").hover().await.unwrap();
        setup(&t, json!(["timeout", 1])).await;
        t.page.locator("#target").click().await.unwrap();
        assert_eq!(clicked(&t).await, Some(1));
        expect(&t.page.locator("#interstitial"))
            .not()
            .to_be_visible()
            .await
            .unwrap();
        assert_eq!(called.load(Ordering::SeqCst), 2);
    });
}

#[test]
fn lh_014_should_remove_locator_handler() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/handle-locator.html", t.server.prefix))
            .await
            .unwrap();
        let called = Arc::new(AtomicUsize::new(0));
        let c = called.clone();
        t.page
            .add_locator_handler(&close_button(&t.page), move |locator| {
                let c = c.clone();
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    locator.click().await
                }
            })
            .await
            .unwrap();
        setup(&t, json!(["hide", 1])).await;
        t.page.locator("#target").click().await.unwrap();
        assert_eq!(called.load(Ordering::SeqCst), 1);
        assert_eq!(clicked(&t).await, Some(1));
        expect(&t.page.locator("#interstitial"))
            .not()
            .to_be_visible()
            .await
            .unwrap();

        setup(&t, json!(["hide", 1])).await;
        t.page
            .remove_locator_handler(&close_button(&t.page))
            .await
            .unwrap();
        let r = t.page.locator("#target").click().timeout(3000).await;
        assert_eq!(called.load(Ordering::SeqCst), 1);
        assert_eq!(clicked(&t).await, Some(0));
        expect(&t.page.locator("#interstitial"))
            .to_be_visible()
            .await
            .unwrap();
        assert_err_contains(r, "Timeout 3000ms exceeded");
    });
}

#[test]
fn lh_locator_must_belong_to_the_main_frame() {
    run(|t| async move {
        attach_frame(&t.page, "frame1", &t.server.empty_page).await;
        let frame = t.page.frames()[1].clone();
        let r = t
            .page
            .add_locator_handler(&frame.locator("body"), |_| async {})
            .await;
        assert_err_contains(r, "Locator must belong to the main frame of this page");
    });
}
