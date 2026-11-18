//! The human-like input (`oxidewright::human`, not part of Playwright): the
//! events reach the page as a person's would, and the result is right.

use serde::Deserialize;

use crate::common::*;
use oxidewright::Page;
use oxidewright::human::{Human, KeyboardLayout, ReadStyle, ScrollDirection};

/// Records the trusted mouse, key, wheel and touch events of the page.
const RECORDER: &str = r#"<script>
  window.ev = [];
  for (const type of ['mousemove', 'mousedown', 'mouseup', 'click', 'keydown', 'keyup', 'wheel', 'touchstart', 'touchend']) {
    addEventListener(type, e => window.ev.push({
      type, t: performance.now(), x: e.clientX ?? (e.touches && e.touches[0] ? e.touches[0].clientX : null),
      y: e.clientY ?? (e.touches && e.touches[0] ? e.touches[0].clientY : null), key: e.key ?? null, code: e.code ?? null,
      shift: !!e.shiftKey, ctrl: !!e.ctrlKey, alt: !!e.altKey, trusted: e.isTrusted,
      target: e.target && e.target.id || null,
    }), true);
  }
</script>"#;

#[derive(Debug, Deserialize)]
struct Ev {
    #[serde(rename = "type")]
    kind: String,
    t: f64,
    x: Option<f64>,
    y: Option<f64>,
    key: Option<String>,
    code: Option<String>,
    shift: bool,
    ctrl: bool,
    alt: bool,
    trusted: bool,
    target: Option<String>,
}

async fn events(page: &Page) -> Vec<Ev> {
    page.evaluate("() => window.ev").await.unwrap()
}

async fn value(page: &Page, selector: &str) -> String {
    page.evaluate("s => document.querySelector(s).value")
        .arg(selector)
        .await
        .unwrap()
}

#[test]
fn click_moves_along_a_path_and_clicks_inside_the_element() {
    run(|t| async move {
        t.page
            .set_content(format!(
                r#"{RECORDER}<button id="b" style="position:absolute;left:600px;top:400px;width:120px;height:40px">OK</button>"#
            ))
            .await
            .unwrap();
        let human = Human::with_seed(1);
        human.click(&t.page.locator("#b")).await.unwrap();
        let ev = events(&t.page).await;
        assert!(ev.iter().all(|e| e.trusted));
        let moves = ev.iter().filter(|e| e.kind == "mousemove").count();
        assert!(moves >= 10, "a path, not a jump: {moves} moves");
        let click = ev.iter().find(|e| e.kind == "click").expect("a click");
        assert_eq!(click.target.as_deref(), Some("b"));
        let (x, y) = (click.x.unwrap(), click.y.unwrap());
        assert!((600.0..=720.0).contains(&x) && (400.0..=440.0).contains(&y));
        // Held like a finger, not 0 ms.
        let down = ev.iter().find(|e| e.kind == "mousedown").unwrap().t;
        let up = ev.iter().find(|e| e.kind == "mouseup").unwrap().t;
        assert!(up - down >= 30.0, "held {} ms", up - down);
    });
}

#[test]
fn types_with_the_keys_of_the_layout() {
    run(|t| async move {
        t.page
            .set_content(format!(r#"{RECORDER}<input id="i">"#))
            .await
            .unwrap();
        let human = Human::with_seed(2).layout(KeyboardLayout::Pt);
        let text = "Ana Conceição @x.pt 12";
        human
            .type_text(&t.page.locator("#i"), text)
            .errors(false)
            .await
            .unwrap();
        assert_eq!(value(&t.page, "#i").await, text);
        let ev = events(&t.page).await;
        let down = |key: &str| {
            ev.iter()
                .find(|e| e.kind == "keydown" && e.key.as_deref() == Some(key))
                .unwrap_or_else(|| panic!("no keydown for {key}"))
        };
        // A capital with Shift; `@` with AltGr (Ctrl+Alt) on Digit2; `ç` on its own key.
        let a = down("A");
        assert!(a.shift && a.code.as_deref() == Some("KeyA"));
        let at = down("@");
        assert!(at.ctrl && at.alt && !at.shift && at.code.as_deref() == Some("Digit2"));
        assert_eq!(down("ç").code.as_deref(), Some("Semicolon"));
        // `ã` is the dead key ~ then `a`.
        assert!(ev.iter().any(|e| e.kind == "keydown"
            && e.key.as_deref() == Some("Dead")
            && e.code.as_deref() == Some("Backslash")));
        assert!(
            ev.iter()
                .any(|e| e.kind == "keydown" && e.key.as_deref() == Some("Shift"))
        );
        // Keys are held.
        let keydowns = ev.iter().filter(|e| e.kind == "keydown").count();
        let keyups = ev.iter().filter(|e| e.kind == "keyup").count();
        assert_eq!(keydowns, keyups);
    });
}

#[test]
fn typos_are_corrected() {
    run(|t| async move {
        t.page.set_content(r#"<input id="i">"#).await.unwrap();
        // Several people, each with its own typos: the text always ends right.
        for seed in 10..13 {
            let human = Human::with_seed(seed);
            t.page
                .evaluate::<()>("() => document.querySelector('#i').value = ''")
                .await
                .unwrap();
            let text = "the quick brown fox jumps";
            human.type_text(&t.page.locator("#i"), text).await.unwrap();
            assert_eq!(value(&t.page, "#i").await, text, "seed {seed}");
        }
    });
}

#[test]
fn fixes_a_field_in_place_and_appends_to_it() {
    run(|t| async move {
        t.page
            .set_content(r#"<input id="i" value="Lopez" style="font:16px Arial;width:300px"><input id="j" value="Rua A  " style="font:16px Arial;width:300px">"#)
            .await
            .unwrap();
        let human = Human::with_seed(3).layout(KeyboardLayout::Pt);
        assert!(
            human
                .fix_field(&t.page.locator("#i"), "López")
                .await
                .unwrap()
        );
        assert_eq!(value(&t.page, "#i").await, "López");
        let expected = human
            .append_to_field(&t.page.locator("#j"), "2º", ", ")
            .await
            .unwrap();
        assert_eq!(expected.as_deref(), Some("Rua A, 2º"));
        assert_eq!(value(&t.page, "#j").await, "Rua A, 2º");
    });
}

#[test]
fn type_verified_gets_a_password_right() {
    run(|t| async move {
        t.page
            .set_content(r#"<input id="p" type="password" value="old">"#)
            .await
            .unwrap();
        let human = Human::with_seed(4).layout(KeyboardLayout::Es);
        human
            .type_verified(&t.page.locator("#p"), "Señal-2024!")
            .await
            .unwrap();
        assert_eq!(value(&t.page, "#p").await, "Señal-2024!");
    });
}

#[test]
fn scrolls_with_the_wheel_to_click_a_far_element() {
    run(|t| async move {
        t.page
            .set_content(format!(
                r#"{RECORDER}<div style="height:3000px"></div><button id="b">far</button><div style="height:600px"></div>"#
            ))
            .await
            .unwrap();
        let human = Human::with_seed(5);
        human.click(&t.page.locator("#b")).await.unwrap();
        let ev = events(&t.page).await;
        let wheels = ev.iter().filter(|e| e.kind == "wheel").count();
        assert!(wheels > 5, "glides in small wheel events: {wheels}");
        assert!(
            ev.iter()
                .any(|e| e.kind == "click" && e.target.as_deref() == Some("b"))
        );
        let y: f64 = t.page.evaluate("() => scrollY").await.unwrap();
        assert!(y > 2000.0);
        // And back up with the wheel.
        human
            .scroll(&t.page, 1500.0, ScrollDirection::Up)
            .await
            .unwrap();
        let y2: f64 = t.page.evaluate("() => scrollY").await.unwrap();
        assert!(y2 < y - 1000.0, "{y} -> {y2}");
    });
}

#[test]
fn on_the_grid_moves_are_whole_pixels_one_per_frame() {
    run(|t| async move {
        t.page.set_content(RECORDER).await.unwrap();
        let human = Human::with_seed(6).grid(1.0, 1000.0 / 60.0);
        human.move_to(&t.page, 700.0, 500.0).await.unwrap();
        human.move_to(&t.page, 200.0, 150.0).await.unwrap();
        let ev = events(&t.page).await;
        let moves: Vec<&Ev> = ev.iter().filter(|e| e.kind == "mousemove").collect();
        assert!(moves.len() > 20);
        for m in &moves {
            assert_eq!(m.x.unwrap().fract(), 0.0);
            assert_eq!(m.y.unwrap().fract(), 0.0);
        }
        // Intervals between moves: a whole number of frames (~16.7 ms),
        // except the pause between the two movements.
        let mut gaps: Vec<f64> = moves.windows(2).map(|w| w[1].t - w[0].t).collect();
        gaps.sort_by(f64::total_cmp);
        let median = gaps[gaps.len() / 2];
        assert!((12.0..=22.0).contains(&median), "median gap {median} ms");
    });
}

#[test]
fn reads_a_text_with_the_mouse_resting_near_the_target() {
    run(|t| async move {
        t.page
            .set_content(format!(
                r#"{RECORDER}<p style="width:400px"><span id="txt">I accept the terms of use and the privacy policy of this site. <a href="/terms">Terms</a></span></p><input type="checkbox" id="c">"#
            ))
            .await
            .unwrap();
        let human = Human::with_seed(7);
        let secs = human
            .read_text(&t.page.locator("#txt"))
            .target(&t.page.locator("#c"))
            .style(ReadStyle::Rest)
            .await
            .unwrap();
        assert!((1.4..20.0).contains(&secs), "{secs} s");
        let moves = events(&t.page)
            .await
            .into_iter()
            .filter(|e| e.kind == "mousemove")
            .count();
        assert!(moves > 0);
    });
}

#[test]
fn taps_on_a_touch_page() {
    run(|t| async move {
        let context = t.browser.new_context().has_touch(true).await.unwrap();
        let page = context.new_page().await.unwrap();
        page.set_content(format!(
            r#"{RECORDER}<button id="b" style="position:absolute;left:100px;top:100px;width:120px;height:60px">OK</button>"#
        ))
        .await
        .unwrap();
        let human = Human::with_seed(8).mobile(true);
        human.click(&page.locator("#b")).await.unwrap();
        let ev = events(&page).await;
        assert!(ev.iter().any(|e| e.kind == "touchstart"));
        assert!(
            ev.iter()
                .any(|e| e.kind == "click" && e.target.as_deref() == Some("b"))
        );
        assert!(!ev.iter().any(|e| e.kind == "mousedown" && !e.trusted));
        context.close().await.unwrap();
    });
}

#[test]
fn press_sends_a_held_key() {
    run(|t| async move {
        t.page
            .set_content(format!(r#"{RECORDER}<input id="i" autofocus>"#))
            .await
            .unwrap();
        t.page.locator("#i").focus().await.unwrap();
        let human = Human::with_seed(9);
        human.press(&t.page, "Enter").await.unwrap();
        let ev = events(&t.page).await;
        let down = ev
            .iter()
            .find(|e| e.kind == "keydown" && e.key.as_deref() == Some("Enter"))
            .unwrap();
        let up = ev
            .iter()
            .find(|e| e.kind == "keyup" && e.key.as_deref() == Some("Enter"))
            .unwrap();
        assert!(up.t - down.t >= 25.0);
    });
}
