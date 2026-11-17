//! Context emulation options, ported from Playwright's
//! `tests/library/browsercontext-viewport.spec.ts`, `browsercontext-locale.spec.ts`,
//! `browsercontext-timezone-id.spec.ts`, `geolocation.spec.ts`, `permissions.spec.ts`,
//! `browsercontext-user-agent.spec.ts`, `browsercontext-device.spec.ts`,
//! `browsercontext-base-url.spec.ts` and `tests/page/page-emulate-media.spec.ts`.
//!
//! Not ported:
//! * VP-009/VP-010/VP-011/VP-014, DV-002..DV-006 (tap, clicks, drag, scrolling):
//!   need the input/locator APIs.
//! * VP-015 (WebKit only), LC-004..LC-007/LC-010/LC-012..LC-014, TZ-006, GE-009,
//!   UA-005/UA-007 (popups, workers, downloads, routing, WebSockets: other APIs).
//! * PE-012..PE-023 (permission change events, clipboard, storage access,
//!   local fonts/network, media capture: platform specific or need HTTPS/input).
//! * GE-004..GE-006 (TypeScript typing errors, not expressible in Rust).
//! * NA-091 (`launchPersistentContext` is not implemented).
//! * EM-002/EM-005 (invalid enum arguments cannot be expressed in Rust).

use oxidewright::{
    ColorScheme, Contrast, ForcedColors, Geolocation, Media, Page, ReducedMotion, ViewportSize,
    devices,
};
use serde_json::{Value, json};
use std::future::IntoFuture;

use crate::common::*;

async fn verify_viewport(page: &Page, width: u32, height: u32) {
    assert_eq!(page.viewport_size(), Some(ViewportSize { width, height }));
    let inner: (u32, u32) = page
        .evaluate("() => [window.innerWidth, window.innerHeight]")
        .await
        .unwrap();
    assert_eq!(inner, (width, height));
}

async fn matches(page: &Page, query: &str) -> bool {
    page.evaluate("q => matchMedia(q).matches")
        .arg(query)
        .await
        .unwrap()
}

#[test]
fn vp_001_should_get_the_proper_default_viewport_size() {
    run(|t| async move {
        verify_viewport(&t.page, 1280, 720).await;
    });
}

#[test]
fn vp_002_should_set_the_proper_viewport_size() {
    run(|t| async move {
        verify_viewport(&t.page, 1280, 720).await;
        t.page.set_viewport_size((345, 456)).await.unwrap();
        verify_viewport(&t.page, 345, 456).await;
    });
}

#[test]
fn vp_003_should_return_correct_outer_width_and_outer_height() {
    run(|t| async move {
        t.page.set_viewport_size((410, 420)).await.unwrap();
        let size: Value = t
            .page
            .evaluate("() => ({ innerWidth: window.innerWidth, innerHeight: window.innerHeight, outerWidth: window.outerWidth, outerHeight: window.outerHeight })")
            .await
            .unwrap();
        assert_eq!(size["innerWidth"], 410);
        assert_eq!(size["innerHeight"], 420);
        assert!(size["outerWidth"].as_u64() >= size["innerWidth"].as_u64());
        assert!(size["outerHeight"].as_u64() >= size["innerHeight"].as_u64());
    });
}

#[test]
fn vp_004_landscape_viewport_should_have_width_larger_than_height() {
    for (name, device) in devices() {
        if name.contains("landscape") || name.contains("Landscape") {
            assert!(device.viewport.width > device.viewport.height, "{name}");
        }
    }
}

#[test]
fn vp_005_should_emulate_device_width() {
    run(|t| async move {
        let page = &t.page;
        assert_eq!(page.viewport_size(), Some((1280, 720).into()));
        page.set_viewport_size((300, 300)).await.unwrap();
        let width: u32 = page.evaluate("() => window.screen.width").await.unwrap();
        assert_eq!(width, 300);
        assert!(matches(page, "(min-device-width: 200px)").await);
        assert!(!matches(page, "(min-device-width: 400px)").await);
        assert!(!matches(page, "(max-device-width: 200px)").await);
        assert!(matches(page, "(max-device-width: 400px)").await);
        assert!(!matches(page, "(device-width: 600px)").await);
        assert!(matches(page, "(device-width: 300px)").await);
        page.set_viewport_size((500, 500)).await.unwrap();
        let width: u32 = page.evaluate("() => window.screen.width").await.unwrap();
        assert_eq!(width, 500);
        assert!(matches(page, "(min-device-width: 400px)").await);
        assert!(!matches(page, "(min-device-width: 600px)").await);
        assert!(matches(page, "(device-width: 500px)").await);
    });
}

#[test]
fn vp_006_should_emulate_device_height() {
    run(|t| async move {
        let page = &t.page;
        page.set_viewport_size((300, 300)).await.unwrap();
        let height: u32 = page.evaluate("() => window.screen.height").await.unwrap();
        assert_eq!(height, 300);
        assert!(matches(page, "(min-device-height: 200px)").await);
        assert!(!matches(page, "(min-device-height: 400px)").await);
        assert!(matches(page, "(device-height: 300px)").await);
        page.set_viewport_size((500, 500)).await.unwrap();
        let height: u32 = page.evaluate("() => window.screen.height").await.unwrap();
        assert_eq!(height, 500);
        assert!(matches(page, "(device-height: 500px)").await);
    });
}

#[test]
fn vp_007_should_emulate_avail_width_and_avail_height() {
    run(|t| async move {
        t.page.set_viewport_size((500, 600)).await.unwrap();
        let avail: (u32, u32) = t
            .page
            .evaluate("() => [window.screen.availWidth, window.screen.availHeight]")
            .await
            .unwrap();
        assert_eq!(avail, (500, 600));
    });
}

#[test]
fn vp_008_should_not_have_touch_by_default() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        let touch: bool = t
            .page
            .evaluate("() => 'ontouchstart' in window")
            .await
            .unwrap();
        assert!(!touch);
    });
}

#[test]
fn vp_012_should_set_both_screen_and_viewport_options() {
    run(|t| async move {
        let context = t
            .browser
            .new_context()
            .screen((1280, 720))
            .viewport(Some((1000, 600).into()))
            .await
            .unwrap();
        let page = context.new_page().await.unwrap();
        let screen: (u32, u32) = page
            .evaluate("() => [screen.width, screen.height]")
            .await
            .unwrap();
        assert_eq!(screen, (1280, 720));
        let inner: (u32, u32) = page
            .evaluate("() => [window.innerWidth, window.innerHeight]")
            .await
            .unwrap();
        assert_eq!(inner, (1000, 600));
        context.close().await.unwrap();
    });
}

#[test]
fn vp_013_should_report_null_viewport_size_when_given_null_viewport() {
    run(|t| async move {
        let context = t.browser.new_context().viewport(None).await.unwrap();
        let page = context.new_page().await.unwrap();
        assert_eq!(page.viewport_size(), None);
        context.close().await.unwrap();
    });
}

#[test]
fn vp_016_should_be_able_to_get_correct_orientation_angle_on_non_mobile_devices() {
    run(|t| async move {
        let context = t
            .browser
            .new_context()
            .viewport(Some((300, 400).into()))
            .is_mobile(false)
            .await
            .unwrap();
        let page = context.new_page().await.unwrap();
        page.goto(&t.server.empty_page).await.unwrap();
        let angle: i32 = page
            .evaluate("() => window.screen.orientation.angle")
            .await
            .unwrap();
        assert_eq!(angle, 0);
        page.set_viewport_size((400, 300)).await.unwrap();
        let angle: i32 = page
            .evaluate("() => window.screen.orientation.angle")
            .await
            .unwrap();
        assert_eq!(angle, 0);
        context.close().await.unwrap();
    });
}

#[test]
fn vp_017_should_set_window_screen_orientation_type_for_mobile_devices() {
    run(|t| async move {
        let context = t
            .browser
            .new_context()
            .device(&devices()["iPhone 14"])
            .await
            .unwrap();
        let page = context.new_page().await.unwrap();
        page.goto(&t.server.empty_page).await.unwrap();
        let kind: String = page
            .evaluate("() => window.screen.orientation.type")
            .await
            .unwrap();
        assert_eq!(kind, "portrait-primary");
        context.close().await.unwrap();
    });
}

#[test]
fn viewport_options_are_validated() {
    run(|t| async move {
        assert_err_contains(
            t.browser
                .new_context()
                .viewport(None)
                .device_scale_factor(2.0)
                .await,
            "\"deviceScaleFactor\" option is not supported with null \"viewport\"",
        );
        assert_err_contains(
            t.browser.new_context().viewport(None).is_mobile(true).await,
            "\"isMobile\" option is not supported with null \"viewport\"",
        );
    });
}

#[test]
fn dsf_should_emulate_device_pixel_ratio() {
    run(|t| async move {
        let context = t
            .browser
            .new_context()
            .device_scale_factor(2.0)
            .await
            .unwrap();
        let page = context.new_page().await.unwrap();
        let ratio: f64 = page
            .evaluate("() => window.devicePixelRatio")
            .await
            .unwrap();
        assert_eq!(ratio, 2.0);
        context.close().await.unwrap();
    });
}

// ----------------------------------------------------------------------
// locale / timezone
// ----------------------------------------------------------------------

#[test]
fn lc_001_should_affect_accept_language_header() {
    run(|t| async move {
        let context = t.browser.new_context().locale("fr-CH").await.unwrap();
        let page = context.new_page().await.unwrap();
        page.goto(&t.server.empty_page).await.unwrap();
        let headers = t.server.request_headers("/empty.html").unwrap();
        let language = headers.get("accept-language").unwrap().to_str().unwrap();
        assert!(language.starts_with("fr-CH"), "{language}");
        context.close().await.unwrap();
    });
}

#[test]
fn lc_002_should_affect_navigator_language() {
    run(|t| async move {
        let context = t.browser.new_context().locale("fr-FR").await.unwrap();
        let page = context.new_page().await.unwrap();
        let language: String = page.evaluate("() => navigator.language").await.unwrap();
        assert_eq!(language, "fr-FR");
        context.close().await.unwrap();
    });
}

#[test]
fn lc_003_should_format_number() {
    run(|t| async move {
        for (locale, expected) in [("en-US", "1,000,000.5"), ("fr-FR", "1 000 000,5")] {
            let context = t.browser.new_context().locale(locale).await.unwrap();
            let page = context.new_page().await.unwrap();
            page.goto(&t.server.empty_page).await.unwrap();
            let formatted: String = page
                .evaluate("() => (1000000.50).toLocaleString().replace(/\\s/g, ' ')")
                .await
                .unwrap();
            assert_eq!(formatted, expected);
            context.close().await.unwrap();
        }
    });
}

#[test]
fn lc_008_should_be_isolated_between_contexts() {
    run(|t| async move {
        let context1 = t.browser.new_context().locale("en-US").await.unwrap();
        let context2 = t.browser.new_context().locale("ru-RU").await.unwrap();
        let page1 = context1.new_page().await.unwrap();
        let page2 = context2.new_page().await.unwrap();
        page1.goto(&t.server.empty_page).await.unwrap();
        page2.goto(&t.server.empty_page).await.unwrap();
        let a: String = page1
            .evaluate("() => (1000000.50).toLocaleString()")
            .await
            .unwrap();
        let b: String = page2
            .evaluate("() => (1000000.50).toLocaleString().replace(/\\s/g, ' ')")
            .await
            .unwrap();
        assert_eq!(a, "1,000,000.5");
        assert_eq!(b, "1 000 000,5");
        context1.close().await.unwrap();
        context2.close().await.unwrap();
    });
}

#[test]
fn lc_009_should_not_change_default_locale_in_another_context() {
    run(|t| async move {
        let default: String = t.page.evaluate("() => navigator.language").await.unwrap();
        let context = t.browser.new_context().locale("ru-RU").await.unwrap();
        let page = context.new_page().await.unwrap();
        let language: String = page.evaluate("() => navigator.language").await.unwrap();
        assert_eq!(language, "ru-RU");
        context.close().await.unwrap();
        let other = t.browser.new_context().await.unwrap();
        let page = other.new_page().await.unwrap();
        let language: String = page.evaluate("() => navigator.language").await.unwrap();
        assert_eq!(language, default);
        other.close().await.unwrap();
    });
}

#[test]
fn lc_011_should_affect_intl_date_time_format_resolved_options_locale() {
    run(|t| async move {
        let context = t.browser.new_context().locale("de-DE").await.unwrap();
        let page = context.new_page().await.unwrap();
        page.goto(&t.server.empty_page).await.unwrap();
        let locale: String = page
            .evaluate("() => Intl.DateTimeFormat().resolvedOptions().locale")
            .await
            .unwrap();
        assert_eq!(locale, "de-DE");
        context.close().await.unwrap();
    });
}

#[test]
fn tz_001_should_work() {
    run(|t| async move {
        let cases = [
            ("America/Jamaica", "Sat Nov 19 2016 13:12:34 GMT-0500"),
            ("Pacific/Honolulu", "Sat Nov 19 2016 08:12:34 GMT-1000"),
            ("America/Buenos_Aires", "Sat Nov 19 2016 15:12:34 GMT-0300"),
            ("Europe/Berlin", "Sat Nov 19 2016 19:12:34 GMT+0100"),
        ];
        for (timezone, expected) in cases {
            let context = t
                .browser
                .new_context()
                .locale("en-US")
                .timezone_id(timezone)
                .await
                .unwrap();
            let page = context.new_page().await.unwrap();
            let date: String = page
                .evaluate("() => new Date(1479579154987).toString()")
                .await
                .unwrap();
            assert!(date.contains(expected), "{timezone}: {date}");
            context.close().await.unwrap();
        }
    });
}

#[test]
fn tz_002_should_throw_for_invalid_timezone_ids_when_creating_pages() {
    run(|t| async move {
        for timezone in ["Foo/Bar", "Baz/Qux"] {
            let context = t.browser.new_context().timezone_id(timezone).await.unwrap();
            assert_err_contains(
                context.new_page().await,
                &format!("Invalid timezone ID: {timezone}"),
            );
            context.close().await.unwrap();
        }
    });
}

#[test]
fn tz_003_should_work_for_multiple_pages_sharing_same_process() {
    run(|t| async move {
        let context = t
            .browser
            .new_context()
            .timezone_id("Europe/Moscow")
            .await
            .unwrap();
        let page = context.new_page().await.unwrap();
        page.goto(&t.server.empty_page).await.unwrap();
        let other = context.new_page().await.unwrap();
        other.goto(&t.server.empty_page).await.unwrap();
        context.close().await.unwrap();
    });
}

#[test]
fn tz_004_should_not_change_default_timezone_in_another_context() {
    run(|t| async move {
        let func = "() => new Intl.DateTimeFormat().resolvedOptions().timeZone";
        let default: String = t.page.evaluate(func).await.unwrap();
        let context = t
            .browser
            .new_context()
            .timezone_id("Europe/Moscow")
            .await
            .unwrap();
        let page = context.new_page().await.unwrap();
        let timezone: String = page.evaluate(func).await.unwrap();
        assert_eq!(timezone, "Europe/Moscow");
        context.close().await.unwrap();
        let context = t.browser.new_context().await.unwrap();
        let page = context.new_page().await.unwrap();
        let timezone: String = page.evaluate(func).await.unwrap();
        assert_eq!(timezone, default);
        context.close().await.unwrap();
    });
}

#[test]
fn tz_005_should_affect_intl_date_time_format_resolved_options_time_zone() {
    run(|t| async move {
        let context = t
            .browser
            .new_context()
            .timezone_id("America/Jamaica")
            .await
            .unwrap();
        let page = context.new_page().await.unwrap();
        page.goto(&t.server.empty_page).await.unwrap();
        let timezone: String = page
            .evaluate("() => Intl.DateTimeFormat().resolvedOptions().timeZone")
            .await
            .unwrap();
        assert_eq!(timezone, "America/Jamaica");
        context.close().await.unwrap();
    });
}

// ----------------------------------------------------------------------
// geolocation / permissions
// ----------------------------------------------------------------------

const GET_POSITION: &str = "() => new Promise(resolve => navigator.geolocation.getCurrentPosition(position => resolve({ latitude: position.coords.latitude, longitude: position.coords.longitude })))";

async fn permission(page: &Page, name: &str) -> String {
    page.evaluate("name => navigator.permissions.query({ name }).then(result => result.state)")
        .arg(name)
        .await
        .unwrap()
}

#[test]
fn ge_001_should_work() {
    run(|t| async move {
        t.context.grant_permissions(["geolocation"]).await.unwrap();
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.context
            .set_geolocation(Some(Geolocation::new(10.0, 10.0)))
            .await
            .unwrap();
        let position: Value = t.page.evaluate(GET_POSITION).await.unwrap();
        assert_eq!(position, json!({ "latitude": 10, "longitude": 10 }));
    });
}

#[test]
fn ge_002_should_throw_when_invalid_longitude() {
    run(|t| async move {
        assert_err_contains(
            t.context
                .set_geolocation(Some(Geolocation::new(10.0, 200.0)))
                .await,
            "geolocation.longitude: precondition -180 <= LONGITUDE <= 180 failed.",
        );
    });
}

#[test]
fn ge_003_should_isolate_contexts() {
    run(|t| async move {
        t.context.grant_permissions(["geolocation"]).await.unwrap();
        t.context
            .set_geolocation(Some(Geolocation::new(10.0, 10.0)))
            .await
            .unwrap();
        t.page.goto(&t.server.empty_page).await.unwrap();
        let context2 = t
            .browser
            .new_context()
            .permissions(["geolocation"])
            .geolocation(Geolocation::new(20.0, 20.0))
            .await
            .unwrap();
        let page2 = context2.new_page().await.unwrap();
        page2.goto(&t.server.empty_page).await.unwrap();
        let position: Value = t.page.evaluate(GET_POSITION).await.unwrap();
        assert_eq!(position, json!({ "latitude": 10, "longitude": 10 }));
        let position: Value = page2.evaluate(GET_POSITION).await.unwrap();
        assert_eq!(position, json!({ "latitude": 20, "longitude": 20 }));
        context2.close().await.unwrap();
    });
}

#[test]
fn ge_007_should_use_context_options() {
    run(|t| async move {
        let context = t
            .browser
            .new_context()
            .geolocation(Geolocation::new(10.0, 10.0))
            .permissions(["geolocation"])
            .await
            .unwrap();
        let page = context.new_page().await.unwrap();
        page.goto(&t.server.empty_page).await.unwrap();
        let position: Value = page.evaluate(GET_POSITION).await.unwrap();
        assert_eq!(position, json!({ "latitude": 10, "longitude": 10 }));
        context.close().await.unwrap();
    });
}

#[test]
fn ge_008_watch_position_should_be_notified() {
    run(|t| async move {
        t.context.grant_permissions(["geolocation"]).await.unwrap();
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.page
            .evaluate::<()>(
                "() => { window.positions = []; navigator.geolocation.watchPosition(pos => window.positions.push(`lat=${pos.coords.latitude} lng=${pos.coords.longitude}`), err => {}); }",
            )
            .await
            .unwrap();
        for (lat, lng) in [(0.0, 0.0), (10.0, 10.0), (20.0, 20.0), (30.0, 30.0)] {
            t.context
                .set_geolocation(Some(Geolocation::new(lat, lng)))
                .await
                .unwrap();
            let expected = format!("lat={lat} lng={lng}");
            let mut seen = false;
            // Up to 10s: slow CI runners took over 2s to deliver a position.
            for _ in 0..500 {
                let positions: Vec<String> =
                    t.page.evaluate("() => window.positions").await.unwrap();
                if positions.contains(&expected) {
                    seen = true;
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            assert!(seen, "{expected}");
        }
    });
}

#[test]
fn pe_001_should_be_prompt_by_default() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        assert_eq!(permission(&t.page, "geolocation").await, "prompt");
    });
}

#[test]
fn pe_002_should_deny_permission_when_not_listed() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.context
            .grant_permissions(Vec::<String>::new())
            .origin(&t.server.empty_page)
            .await
            .unwrap();
        assert_eq!(permission(&t.page, "geolocation").await, "denied");
    });
}

#[test]
fn pe_003_should_fail_when_bad_permission_is_given() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        assert_err_contains(
            t.context
                .grant_permissions(["foo"])
                .origin(&t.server.empty_page)
                .await,
            "Unknown permission: foo",
        );
    });
}

#[test]
fn pe_004_should_grant_geolocation_permission_when_origin_is_listed() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.context
            .grant_permissions(["geolocation"])
            .origin(&t.server.empty_page)
            .await
            .unwrap();
        assert_eq!(permission(&t.page, "geolocation").await, "granted");
    });
}

#[test]
fn pe_005_should_prompt_for_geolocation_permission_when_origin_is_not_listed() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.context
            .grant_permissions(["geolocation"])
            .origin(&t.server.empty_page)
            .await
            .unwrap();
        t.page
            .goto(format!("{}/empty.html", t.server.cross_process_prefix))
            .await
            .unwrap();
        assert_eq!(permission(&t.page, "geolocation").await, "prompt");
    });
}

#[test]
fn pe_006_should_grant_notifications_permission_when_listed() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.context
            .grant_permissions(["notifications"])
            .origin(&t.server.empty_page)
            .await
            .unwrap();
        assert_eq!(permission(&t.page, "notifications").await, "granted");
    });
}

#[test]
fn pe_007_should_accumulate_when_adding() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.context.grant_permissions(["geolocation"]).await.unwrap();
        t.context
            .grant_permissions(["notifications"])
            .await
            .unwrap();
        assert_eq!(permission(&t.page, "geolocation").await, "granted");
        assert_eq!(permission(&t.page, "notifications").await, "granted");
    });
}

#[test]
fn pe_008_should_clear_permissions() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.context.grant_permissions(["geolocation"]).await.unwrap();
        t.context.clear_permissions().await.unwrap();
        t.context
            .grant_permissions(["notifications"])
            .await
            .unwrap();
        assert_ne!(permission(&t.page, "geolocation").await, "granted");
        assert_eq!(permission(&t.page, "notifications").await, "granted");
    });
}

#[test]
fn pe_009_should_grant_permission_when_listed_for_all_domains() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.context.grant_permissions(["geolocation"]).await.unwrap();
        assert_eq!(permission(&t.page, "geolocation").await, "granted");
    });
}

#[test]
fn pe_010_should_grant_permission_when_creating_context() {
    run(|t| async move {
        let context = t
            .browser
            .new_context()
            .permissions(["geolocation"])
            .await
            .unwrap();
        let page = context.new_page().await.unwrap();
        page.goto(&t.server.empty_page).await.unwrap();
        assert_eq!(permission(&page, "geolocation").await, "granted");
        context.close().await.unwrap();
    });
}

#[test]
fn pe_011_should_reset_permissions() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.context
            .grant_permissions(["geolocation"])
            .origin(&t.server.empty_page)
            .await
            .unwrap();
        assert_eq!(permission(&t.page, "geolocation").await, "granted");
        t.context.clear_permissions().await.unwrap();
        assert_eq!(permission(&t.page, "geolocation").await, "prompt");
    });
}

// ----------------------------------------------------------------------
// user agent / devices / base URL
// ----------------------------------------------------------------------

#[test]
fn ua_001_should_work() {
    run(|t| async move {
        let default: String = t.page.evaluate("() => navigator.userAgent").await.unwrap();
        assert!(default.contains("Mozilla"));
        let context = t.browser.new_context().user_agent("foobar").await.unwrap();
        let page = context.new_page().await.unwrap();
        page.goto(&t.server.empty_page).await.unwrap();
        let headers = t.server.request_headers("/empty.html").unwrap();
        assert_eq!(headers.get("user-agent").unwrap(), "foobar");
        let ua: String = page.evaluate("() => navigator.userAgent").await.unwrap();
        assert_eq!(ua, "foobar");
        context.close().await.unwrap();
    });
}

#[test]
fn ua_002_should_work_for_subframes() {
    run(|t| async move {
        let context = t.browser.new_context().user_agent("foobar").await.unwrap();
        let page = context.new_page().await.unwrap();
        page.goto(&t.server.empty_page).await.unwrap();
        attach_frame(&page, "frame1", &format!("{}/title.html", t.server.prefix)).await;
        let headers = t.server.request_headers("/title.html").unwrap();
        assert_eq!(headers.get("user-agent").unwrap(), "foobar");
        context.close().await.unwrap();
    });
}

#[test]
fn ua_003_should_emulate_device_user_agent() {
    run(|t| async move {
        let iphone = &devices()["iPhone 6"];
        let context = t
            .browser
            .new_context()
            .user_agent(iphone.user_agent.clone())
            .await
            .unwrap();
        let page = context.new_page().await.unwrap();
        page.goto(&t.server.empty_page).await.unwrap();
        let ua: String = page.evaluate("() => navigator.userAgent").await.unwrap();
        assert!(ua.contains("iPhone"), "{ua}");
        context.close().await.unwrap();
    });
}

#[test]
fn ua_006_should_work_for_navigator_user_agent_data_and_sec_ch_ua_headers() {
    run(|t| async move {
        let context = t
            .browser
            .new_context()
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
            .await
            .unwrap();
        let page = context.new_page().await.unwrap();
        page.goto(&t.server.empty_page).await.unwrap();
        let platform: String = page
            .evaluate("() => navigator.userAgentData.platform")
            .await
            .unwrap();
        assert_eq!(platform, "Windows");
        let mobile: bool = page
            .evaluate("() => navigator.userAgentData.mobile")
            .await
            .unwrap();
        assert!(!mobile);
        context.close().await.unwrap();
    });
}

#[test]
fn dv_001_should_work() {
    run(|t| async move {
        let iphone = &devices()["iPhone 6"];
        let context = t.browser.new_context().device(iphone).await.unwrap();
        let page = context.new_page().await.unwrap();
        page.set_content(
            "<meta name=\"viewport\" content=\"width=device-width, user-scalable=no\" />",
        )
        .await
        .unwrap();
        let width: u32 = page.evaluate("() => window.innerWidth").await.unwrap();
        assert_eq!(width, 375);
        let ua: String = page.evaluate("() => navigator.userAgent").await.unwrap();
        assert!(ua.contains("iPhone"));
        context.close().await.unwrap();
    });
}

#[test]
fn dv_007_should_emulate_viewport_and_screen_size() {
    run(|t| async move {
        let context = t
            .browser
            .new_context()
            .device(&devices()["iPhone 12"])
            .await
            .unwrap();
        let page = context.new_page().await.unwrap();
        page.set_content(
            "<meta name=\"viewport\" content=\"width=device-width, user-scalable=no\" />",
        )
        .await
        .unwrap();
        let sizes: Value = page
            .evaluate("() => ({ screen: { width: window.screen.width, height: window.screen.height }, viewport: { width: window.innerWidth, height: window.innerHeight } })")
            .await
            .unwrap();
        assert_eq!(
            sizes,
            json!({ "screen": { "width": 390, "height": 844 }, "viewport": { "width": 390, "height": 664 } })
        );
        context.close().await.unwrap();
    });
}

#[test]
fn dv_008_should_emulate_viewport_without_screen_size() {
    run(|t| async move {
        let mut options = oxidewright::NewContextOptions::from(&devices()["iPhone 6"]);
        options.screen = None;
        let context = t.browser.new_context().options(options).await.unwrap();
        let page = context.new_page().await.unwrap();
        page.set_content(
            "<meta name=\"viewport\" content=\"width=device-width, user-scalable=no\" />",
        )
        .await
        .unwrap();
        let sizes: Value = page
            .evaluate("() => ({ screen: { width: window.screen.width, height: window.screen.height }, viewport: { width: window.innerWidth, height: window.innerHeight } })")
            .await
            .unwrap();
        assert_eq!(
            sizes,
            json!({ "screen": { "width": 375, "height": 667 }, "viewport": { "width": 375, "height": 667 } })
        );
        context.close().await.unwrap();
    });
}

#[test]
fn na_089_should_construct_a_new_url_when_a_base_url_in_browser_new_context_is_passed_to_page_goto()
{
    run(|t| async move {
        let context = t
            .browser
            .new_context()
            .base_url(&t.server.prefix)
            .await
            .unwrap();
        let page = context.new_page().await.unwrap();
        let response = page.goto("/empty.html").await.unwrap().unwrap();
        assert_eq!(response.url(), t.server.empty_page);
        context.close().await.unwrap();
    });
}

#[test]
fn na_092_should_construct_the_urls_correctly_when_a_base_url_without_a_trailing_slash_is_passed() {
    run(|t| async move {
        let context = t
            .browser
            .new_context()
            .base_url(format!("{}/url-construction", t.server.prefix))
            .await
            .unwrap();
        let page = context.new_page().await.unwrap();
        let url = |path: &str| {
            let page = page.clone();
            let path = path.to_string();
            async move { page.goto(path).await.unwrap().unwrap().url().to_string() }
        };
        assert_eq!(
            url("mypage.html").await,
            format!("{}/mypage.html", t.server.prefix)
        );
        assert_eq!(
            url("./mypage.html").await,
            format!("{}/mypage.html", t.server.prefix)
        );
        assert_eq!(
            url("/mypage.html").await,
            format!("{}/mypage.html", t.server.prefix)
        );
        context.close().await.unwrap();
    });
}

#[test]
fn na_093_should_construct_the_urls_correctly_when_a_base_url_with_a_trailing_slash_is_passed() {
    run(|t| async move {
        let context = t
            .browser
            .new_context()
            .base_url(format!("{}/url-construction/", t.server.prefix))
            .await
            .unwrap();
        let page = context.new_page().await.unwrap();
        let response = page.goto("mypage.html").await.unwrap().unwrap();
        assert_eq!(
            response.url(),
            format!("{}/url-construction/mypage.html", t.server.prefix)
        );
        let response = page.goto("/mypage.html").await.unwrap().unwrap();
        assert_eq!(response.url(), format!("{}/mypage.html", t.server.prefix));
        context.close().await.unwrap();
    });
}

#[test]
fn na_094_should_not_construct_a_new_url_when_valid_urls_are_passed() {
    run(|t| async move {
        let context = t
            .browser
            .new_context()
            .base_url("http://localhost")
            .await
            .unwrap();
        let page = context.new_page().await.unwrap();
        let response = page.goto(&t.server.empty_page).await.unwrap().unwrap();
        assert_eq!(response.url(), t.server.empty_page);
        page.goto("data:text/html,Hello world").await.unwrap();
        let url: String = page.evaluate("() => window.location.href").await.unwrap();
        assert_eq!(url, "data:text/html,Hello world");
        context.close().await.unwrap();
    });
}

#[test]
fn na_095_should_be_able_to_match_a_url_relative_to_its_given_url_with_url_matcher() {
    run(|t| async move {
        let context = t
            .browser
            .new_context()
            .base_url(format!("{}/foobar/", t.server.prefix))
            .await
            .unwrap();
        let page = context.new_page().await.unwrap();
        page.goto("/kek/index.html").await.unwrap();
        page.wait_for_url("/kek/index.html").await.unwrap();
        assert_eq!(page.url(), format!("{}/kek/index.html", t.server.prefix));
        context.close().await.unwrap();
    });
}

// ----------------------------------------------------------------------
// emulate media
// ----------------------------------------------------------------------

#[test]
fn em_001_should_emulate_type() {
    run(|t| async move {
        let page = &t.page;
        assert!(matches(page, "screen").await);
        assert!(!matches(page, "print").await);
        page.emulate_media().media(Media::Print).await.unwrap();
        assert!(!matches(page, "screen").await);
        assert!(matches(page, "print").await);
        page.emulate_media().await.unwrap();
        assert!(matches(page, "print").await);
        page.emulate_media().media(Media::NoOverride).await.unwrap();
        assert!(matches(page, "screen").await);
        assert!(!matches(page, "print").await);
    });
}

#[test]
fn em_003_should_emulate_color_scheme_should_work() {
    run(|t| async move {
        let page = &t.page;
        page.emulate_media()
            .color_scheme(ColorScheme::Light)
            .await
            .unwrap();
        assert!(matches(page, "(prefers-color-scheme: light)").await);
        assert!(!matches(page, "(prefers-color-scheme: dark)").await);
        page.emulate_media()
            .color_scheme(ColorScheme::Dark)
            .await
            .unwrap();
        assert!(matches(page, "(prefers-color-scheme: dark)").await);
        assert!(!matches(page, "(prefers-color-scheme: light)").await);
    });
}

#[test]
fn em_004_should_default_to_light() {
    // Unlike Playwright, oxidewright does not force `light` when nothing is
    // emulated (stealth): the default is the browser's own (OS) preference.
    run(|t| async move {
        let page = &t.page;
        let initially_light = matches(page, "(prefers-color-scheme: light)").await;
        page.emulate_media()
            .color_scheme(ColorScheme::Dark)
            .await
            .unwrap();
        assert!(matches(page, "(prefers-color-scheme: dark)").await);
        page.emulate_media()
            .color_scheme(ColorScheme::NoOverride)
            .await
            .unwrap();
        assert_eq!(
            matches(page, "(prefers-color-scheme: light)").await,
            initially_light
        );
        page.emulate_media()
            .color_scheme(ColorScheme::Light)
            .await
            .unwrap();
        assert!(matches(page, "(prefers-color-scheme: light)").await);
    });
}

#[test]
fn em_006_should_work_during_navigation() {
    run(|t| async move {
        let page = &t.page;
        page.emulate_media()
            .color_scheme(ColorScheme::Light)
            .await
            .unwrap();
        let navigation = page.goto(&t.server.empty_page);
        let page2 = page.clone();
        let emulate = async move {
            for _ in 0..9 {
                let scheme = if fastrand::bool() {
                    ColorScheme::Dark
                } else {
                    ColorScheme::Light
                };
                page2.emulate_media().color_scheme(scheme).await.unwrap();
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            }
        };
        let (result, ()) = tokio::join!(navigation.into_future(), emulate);
        result.unwrap();
        page.emulate_media()
            .color_scheme(ColorScheme::Dark)
            .await
            .unwrap();
        assert!(matches(page, "(prefers-color-scheme: dark)").await);
    });
}

#[test]
fn em_008_should_emulate_reduced_motion() {
    run(|t| async move {
        let page = &t.page;
        // Not overridden by default: the initial value is the host's setting.
        page.emulate_media()
            .reduced_motion(ReducedMotion::Reduce)
            .await
            .unwrap();
        assert!(matches(page, "(prefers-reduced-motion: reduce)").await);
        assert!(!matches(page, "(prefers-reduced-motion: no-preference)").await);
        page.emulate_media()
            .reduced_motion(ReducedMotion::NoPreference)
            .await
            .unwrap();
        assert!(!matches(page, "(prefers-reduced-motion: reduce)").await);
        assert!(matches(page, "(prefers-reduced-motion: no-preference)").await);
    });
}

#[test]
fn em_009_should_keep_reduced_motion_and_color_emulation_after_reload() {
    run(|t| async move {
        let page = &t.page;
        page.emulate_media()
            .reduced_motion(ReducedMotion::Reduce)
            .color_scheme(ColorScheme::Dark)
            .await
            .unwrap();
        page.goto(&t.server.empty_page).await.unwrap();
        page.reload().await.unwrap();
        assert!(matches(page, "(prefers-reduced-motion: reduce)").await);
        assert!(matches(page, "(prefers-color-scheme: dark)").await);
    });
}

#[test]
fn em_010_should_emulate_forced_colors() {
    run(|t| async move {
        let page = &t.page;
        assert!(matches(page, "(forced-colors: none)").await);
        page.emulate_media()
            .forced_colors(ForcedColors::None)
            .await
            .unwrap();
        assert!(matches(page, "(forced-colors: none)").await);
        assert!(!matches(page, "(forced-colors: active)").await);
        page.emulate_media()
            .forced_colors(ForcedColors::Active)
            .await
            .unwrap();
        assert!(!matches(page, "(forced-colors: none)").await);
        assert!(matches(page, "(forced-colors: active)").await);
    });
}

#[test]
fn em_011_should_emulate_contrast() {
    run(|t| async move {
        let page = &t.page;
        assert!(matches(page, "(prefers-contrast: no-preference)").await);
        page.emulate_media()
            .contrast(Contrast::NoPreference)
            .await
            .unwrap();
        assert!(matches(page, "(prefers-contrast: no-preference)").await);
        assert!(!matches(page, "(prefers-contrast: more)").await);
        page.emulate_media().contrast(Contrast::More).await.unwrap();
        assert!(!matches(page, "(prefers-contrast: no-preference)").await);
        assert!(matches(page, "(prefers-contrast: more)").await);
    });
}

#[test]
fn em_012_should_report_hover_and_fine_pointer_for_desktop() {
    run(|t| async move {
        // Headless Chrome on Linux CI machines without a pointing device reports
        // `hover: none`; the media features come from the host there.
        if cfg!(target_os = "linux") {
            return;
        }
        assert!(matches(&t.page, "(hover: hover)").await);
        assert!(matches(&t.page, "(pointer: fine)").await);
    });
}

#[test]
fn context_color_scheme_option_should_work() {
    run(|t| async move {
        let context = t
            .browser
            .new_context()
            .color_scheme(ColorScheme::Dark)
            .reduced_motion(ReducedMotion::Reduce)
            .await
            .unwrap();
        let page = context.new_page().await.unwrap();
        assert!(matches(&page, "(prefers-color-scheme: dark)").await);
        assert!(matches(&page, "(prefers-reduced-motion: reduce)").await);
        page.emulate_media()
            .color_scheme(ColorScheme::Light)
            .await
            .unwrap();
        assert!(matches(&page, "(prefers-color-scheme: light)").await);
        assert!(matches(&page, "(prefers-reduced-motion: reduce)").await);
        context.close().await.unwrap();
    });
}

// ----------------------------------------------------------------------
// misc context options
// ----------------------------------------------------------------------

#[test]
fn should_disable_javascript() {
    run(|t| async move {
        let context = t
            .browser
            .new_context()
            .java_script_enabled(false)
            .await
            .unwrap();
        let page = context.new_page().await.unwrap();
        page.goto("data:text/html, <script>var something = 'forbidden'</script>")
            .await
            .unwrap();
        let has: bool = page
            .evaluate("() => typeof something !== 'undefined'")
            .await
            .unwrap_or(false);
        assert!(!has);
        context.close().await.unwrap();
    });
}

#[test]
fn should_bypass_csp_meta_tag() {
    run(|t| async move {
        let html = "<meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'self'\">";
        t.server.set_content("/csp.html", "text/html", html);
        let context = t.browser.new_context().bypass_csp(true).await.unwrap();
        let page = context.new_page().await.unwrap();
        page.goto(format!("{}/csp.html", t.server.prefix))
            .await
            .unwrap();
        page.add_script_tag()
            .content("window.__injected = 42;")
            .await
            .unwrap();
        let value: i32 = page.evaluate("() => window.__injected").await.unwrap();
        assert_eq!(value, 42);
        context.close().await.unwrap();
    });
}

#[test]
fn should_block_service_workers() {
    run(|t| async move {
        let context = t
            .browser
            .new_context()
            .service_workers(oxidewright::ServiceWorkerPolicy::Block)
            .await
            .unwrap();
        let page = context.new_page().await.unwrap();
        page.goto(&t.server.empty_page).await.unwrap();
        let registration: Value = page
            .evaluate("() => navigator.serviceWorker.register('sw.js')")
            .await
            .unwrap();
        assert!(registration.is_null());
        context.close().await.unwrap();
    });
}

#[test]
fn has_touch_option_should_work() {
    run(|t| async move {
        let context = t.browser.new_context().has_touch(true).await.unwrap();
        let page = context.new_page().await.unwrap();
        page.goto(&t.server.empty_page).await.unwrap();
        let touch: bool = page
            .evaluate("() => 'ontouchstart' in window")
            .await
            .unwrap();
        assert!(touch);
        context.close().await.unwrap();
    });
}
