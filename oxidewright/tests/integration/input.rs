//! Ports of `page-keyboard.spec.ts`, `page-mouse.spec.ts`, `wheel.spec.ts`
//! and `library/tap.spec.ts` (the parts that only need the keyboard, mouse
//! and touchscreen; element actions are replaced by `evaluate` focusing).
//!
//! Skipped:
//! - KB-005, KB-023, KB-024, KB-031, KB-045..KB-049, KB-053, MO-008..MO-011,
//!   MO-016, TA-001..TA-006, TA-008, TA-009: need locators / element actions
//!   or `expect` matchers (ported with the locator tests).
//! - KB-033, KB-040: macOS only.
//! - KB-037: WebKit only.
//! - KB-050, KB-051, MO-015: context menus are best-effort on Windows
//!   Chromium (skipped upstream on Windows).
//! - MO-018: needs popups.
//! - MO-019: needs SVG hit testing details per platform (fixme upstream).
//! - KB-006: uses the `console` event only for debugging; ported without it.

use serde_json::{Value, json};

use crate::common::*;
use oxidewright::Page;
use oxidewright::input::MouseButton;

async fn focus(page: &Page, selector: &str) {
    page.evaluate::<()>("s => document.querySelector(s).focus()")
        .arg(selector)
        .await
        .unwrap();
}

async fn value(page: &Page, selector: &str) -> String {
    page.evaluate("s => document.querySelector(s).value")
        .arg(selector)
        .await
        .unwrap()
}

async fn result(page: &Page) -> String {
    page.evaluate("getResult()").await.unwrap()
}

async fn wait_for_function(page: &Page, expression: &str) {
    for _ in 0..200 {
        if page.evaluate::<bool>(expression).await.unwrap_or(false) {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("condition never became true: {expression}");
}

async fn capture_last_keydown(page: &Page) -> oxidewright::JSHandle {
    page.evaluate_handle(
        "() => {
            const lastEvent = { repeat: false, location: -1, code: '', key: '', metaKey: false, keyIdentifier: 'unsupported' };
            document.addEventListener('keydown', e => {
                lastEvent.repeat = e.repeat;
                lastEvent.location = e.location;
                lastEvent.key = e.key;
                lastEvent.code = e.code;
                lastEvent.metaKey = e.metaKey;
                lastEvent.keyIdentifier = 'keyIdentifier' in e && typeof e['keyIdentifier'] === 'string' && e['keyIdentifier'];
            }, true);
            return lastEvent;
        }",
    )
    .await
    .unwrap()
}

// ---- page-keyboard.spec.ts ----

#[test]
fn should_type_into_a_textarea() {
    run(|t| async move {
        t.page
            .evaluate::<()>("() => { const textarea = document.createElement('textarea'); document.body.appendChild(textarea); textarea.focus(); }")
            .await
            .unwrap();
        let text = "Hello world. I am the text that was typed!";
        t.page.keyboard().r#type(text).await.unwrap();
        assert_eq!(value(&t.page, "textarea").await, text);
    });
}

#[test]
fn should_move_with_the_arrow_keys() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/textarea.html", t.server.prefix))
            .await
            .unwrap();
        focus(&t.page, "textarea").await;
        let keyboard = t.page.keyboard();
        keyboard.r#type("Hello World!").await.unwrap();
        assert_eq!(value(&t.page, "textarea").await, "Hello World!");
        for _ in 0.."World!".len() {
            keyboard.press("ArrowLeft").await.unwrap();
        }
        keyboard.r#type("inserted ").await.unwrap();
        assert_eq!(value(&t.page, "textarea").await, "Hello inserted World!");
        keyboard.down("Shift").await.unwrap();
        for _ in 0.."inserted ".len() {
            keyboard.press("ArrowLeft").await.unwrap();
        }
        keyboard.up("Shift").await.unwrap();
        keyboard.press("Backspace").await.unwrap();
        assert_eq!(value(&t.page, "textarea").await, "Hello World!");
    });
}

#[test]
fn should_send_a_character_with_insert_text() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/textarea.html", t.server.prefix))
            .await
            .unwrap();
        focus(&t.page, "textarea").await;
        t.page.keyboard().insert_text("嗨").await.unwrap();
        assert_eq!(value(&t.page, "textarea").await, "嗨");
        t.page
            .evaluate::<()>(
                "() => window.addEventListener('keydown', e => e.preventDefault(), true)",
            )
            .await
            .unwrap();
        t.page.keyboard().insert_text("a").await.unwrap();
        assert_eq!(value(&t.page, "textarea").await, "嗨a");
    });
}

#[test]
fn insert_text_should_only_emit_input_event() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/textarea.html", t.server.prefix))
            .await
            .unwrap();
        focus(&t.page, "textarea").await;
        let events = t
            .page
            .evaluate_handle(
                "() => { const events = []; for (const type of ['keydown', 'keyup', 'keypress', 'input']) document.addEventListener(type, e => events.push(e.type)); return events; }",
            )
            .await
            .unwrap();
        t.page.keyboard().insert_text("hello world").await.unwrap();
        let events: Vec<String> = events.json_value().await.unwrap();
        assert_eq!(events, vec!["input"]);
    });
}

#[test]
fn should_emit_keydown_keypress_text_input_and_input_when_typing_a_character() {
    run(|t| async move {
        t.page.set_content("<input>").await.unwrap();
        let events = t
            .page
            .evaluate_handle(
                "() => { const events = []; for (const type of ['keydown', 'keypress', 'textInput', 'input', 'keyup']) document.querySelector('input').addEventListener(type, () => events.push(type)); return events; }",
            )
            .await
            .unwrap();
        focus(&t.page, "input").await;
        t.page.keyboard().press("f").await.unwrap();
        let events: Vec<String> = events.json_value().await.unwrap();
        assert_eq!(
            events,
            vec!["keydown", "keypress", "textInput", "input", "keyup"]
        );
    });
}

#[test]
fn should_dispatch_key_events_in_separate_tasks() {
    run(|t| async move {
        t.page.set_content("<input>").await.unwrap();
        let log = t
            .page
            .evaluate_handle(
                "() => { const log = []; const input = document.querySelector('input'); for (const type of ['keydown', 'keypress']) input.addEventListener(type, () => { log.push(type); queueMicrotask(() => log.push('microtask-' + type)); }); input.focus(); return log; }",
            )
            .await
            .unwrap();
        t.page.keyboard().press("a").await.unwrap();
        let log: Vec<String> = log.json_value().await.unwrap();
        assert_eq!(
            log,
            vec![
                "keydown",
                "microtask-keydown",
                "keypress",
                "microtask-keypress"
            ]
        );
    });
}

#[test]
fn should_report_shift_key() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/keyboard.html", t.server.prefix))
            .await
            .unwrap();
        let keyboard = t.page.keyboard();
        for modifier in ["Shift", "Alt", "Control"] {
            keyboard.down(modifier).await.unwrap();
            assert_eq!(
                result(&t.page).await,
                format!("Keydown: {modifier} {modifier}Left LEFT [{modifier}]")
            );
            keyboard.down("!").await.unwrap();
            if modifier == "Shift" {
                assert_eq!(
                    result(&t.page).await,
                    format!(
                        "Keydown: ! Digit1 STANDARD [{modifier}]\nKeypress: ! Digit1 STANDARD 33 [{modifier}]"
                    )
                );
            } else {
                assert_eq!(
                    result(&t.page).await,
                    format!("Keydown: ! Digit1 STANDARD [{modifier}]")
                );
            }
            keyboard.up("!").await.unwrap();
            assert_eq!(
                result(&t.page).await,
                format!("Keyup: ! Digit1 STANDARD [{modifier}]")
            );
            keyboard.up(modifier).await.unwrap();
            assert_eq!(
                result(&t.page).await,
                format!("Keyup: {modifier} {modifier}Left LEFT []")
            );
        }
    });
}

#[test]
fn should_report_multiple_modifiers() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/keyboard.html", t.server.prefix))
            .await
            .unwrap();
        let keyboard = t.page.keyboard();
        keyboard.down("Control").await.unwrap();
        assert_eq!(
            result(&t.page).await,
            "Keydown: Control ControlLeft LEFT [Control]"
        );
        keyboard.down("Alt").await.unwrap();
        assert_eq!(
            result(&t.page).await,
            "Keydown: Alt AltLeft LEFT [Alt Control]"
        );
        keyboard.down(";").await.unwrap();
        assert_eq!(
            result(&t.page).await,
            "Keydown: ; Semicolon STANDARD [Alt Control]"
        );
        keyboard.up(";").await.unwrap();
        assert_eq!(
            result(&t.page).await,
            "Keyup: ; Semicolon STANDARD [Alt Control]"
        );
        keyboard.up("Control").await.unwrap();
        assert_eq!(
            result(&t.page).await,
            "Keyup: Control ControlLeft LEFT [Alt]"
        );
        keyboard.up("Alt").await.unwrap();
        assert_eq!(result(&t.page).await, "Keyup: Alt AltLeft LEFT []");
    });
}

#[test]
fn should_send_proper_codes_while_typing() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/keyboard.html", t.server.prefix))
            .await
            .unwrap();
        t.page.keyboard().r#type("!").await.unwrap();
        assert_eq!(
            result(&t.page).await,
            "Keydown: ! Digit1 STANDARD []\nKeypress: ! Digit1 STANDARD 33 []\nKeyup: ! Digit1 STANDARD []"
        );
        t.page.keyboard().r#type("^").await.unwrap();
        assert_eq!(
            result(&t.page).await,
            "Keydown: ^ Digit6 STANDARD []\nKeypress: ^ Digit6 STANDARD 94 []\nKeyup: ^ Digit6 STANDARD []"
        );
    });
}

#[test]
fn should_send_proper_codes_while_typing_with_shift() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/keyboard.html", t.server.prefix))
            .await
            .unwrap();
        let keyboard = t.page.keyboard();
        keyboard.down("Shift").await.unwrap();
        keyboard.type_text("~").await.unwrap();
        assert_eq!(
            result(&t.page).await,
            [
                "Keydown: Shift ShiftLeft LEFT [Shift]",
                "Keydown: ~ Backquote STANDARD [Shift]",
                "Keypress: ~ Backquote STANDARD 126 [Shift]",
                "Keyup: ~ Backquote STANDARD [Shift]",
            ]
            .join("\n")
        );
        keyboard.up("Shift").await.unwrap();
    });
}

#[test]
fn should_not_type_canceled_events() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/textarea.html", t.server.prefix))
            .await
            .unwrap();
        focus(&t.page, "textarea").await;
        t.page
            .evaluate::<()>(
                "() => { window.addEventListener('keydown', event => { event.stopPropagation(); event.stopImmediatePropagation(); if (event.key === 'l') event.preventDefault(); if (event.key === 'o') event.preventDefault(); }, false); }",
            )
            .await
            .unwrap();
        t.page.keyboard().r#type("Hello World!").await.unwrap();
        assert_eq!(value(&t.page, "textarea").await, "He Wrd!");
    });
}

async fn press_and_expect(t: &TestContext, key: &str, expected: &[&str]) {
    t.page
        .goto(format!("{}/input/keyboard.html", t.server.prefix))
        .await
        .unwrap();
    t.page.keyboard().press(key).await.unwrap();
    assert_eq!(result(&t.page).await, expected.join("\n"));
}

#[test]
fn should_press_plus() {
    run(|t| async move {
        press_and_expect(
            &t,
            "+",
            &[
                "Keydown: + Equal STANDARD []",
                "Keypress: + Equal STANDARD 43 []",
                "Keyup: + Equal STANDARD []",
            ],
        )
        .await;
    });
}

#[test]
fn should_press_shift_plus() {
    run(|t| async move {
        press_and_expect(
            &t,
            "Shift++",
            &[
                "Keydown: Shift ShiftLeft LEFT [Shift]",
                "Keydown: + Equal STANDARD [Shift]",
                "Keypress: + Equal STANDARD 43 [Shift]",
                "Keyup: + Equal STANDARD [Shift]",
                "Keyup: Shift ShiftLeft LEFT []",
            ],
        )
        .await;
    });
}

#[test]
fn should_support_plus_separated_modifiers() {
    run(|t| async move {
        press_and_expect(
            &t,
            "Shift+~",
            &[
                "Keydown: Shift ShiftLeft LEFT [Shift]",
                "Keydown: ~ Backquote STANDARD [Shift]",
                "Keypress: ~ Backquote STANDARD 126 [Shift]",
                "Keyup: ~ Backquote STANDARD [Shift]",
                "Keyup: Shift ShiftLeft LEFT []",
            ],
        )
        .await;
    });
}

#[test]
fn should_support_multiple_plus_separated_modifiers() {
    run(|t| async move {
        press_and_expect(
            &t,
            "Control+Shift+~",
            &[
                "Keydown: Control ControlLeft LEFT [Control]",
                "Keydown: Shift ShiftLeft LEFT [Control Shift]",
                "Keydown: ~ Backquote STANDARD [Control Shift]",
                "Keyup: ~ Backquote STANDARD [Control Shift]",
                "Keyup: Shift ShiftLeft LEFT [Control]",
                "Keyup: Control ControlLeft LEFT []",
            ],
        )
        .await;
    });
}

#[test]
fn should_shift_raw_codes() {
    run(|t| async move {
        press_and_expect(
            &t,
            "Shift+Digit3",
            &[
                "Keydown: Shift ShiftLeft LEFT [Shift]",
                "Keydown: # Digit3 STANDARD [Shift]",
                "Keypress: # Digit3 STANDARD 35 [Shift]",
                "Keyup: # Digit3 STANDARD [Shift]",
                "Keyup: Shift ShiftLeft LEFT []",
            ],
        )
        .await;
    });
}

#[test]
fn should_specify_repeat_property() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/textarea.html", t.server.prefix))
            .await
            .unwrap();
        focus(&t.page, "textarea").await;
        let last = capture_last_keydown(&t.page).await;
        let repeat = || async { last.evaluate::<bool>("e => e.repeat").await.unwrap() };
        let keyboard = t.page.keyboard();
        keyboard.down("a").await.unwrap();
        assert!(!repeat().await);
        keyboard.press("a").await.unwrap();
        assert!(repeat().await);
        keyboard.down("b").await.unwrap();
        assert!(!repeat().await);
        keyboard.down("b").await.unwrap();
        assert!(repeat().await);
        keyboard.up("a").await.unwrap();
        keyboard.down("a").await.unwrap();
        assert!(!repeat().await);
    });
}

#[test]
fn should_type_all_kinds_of_characters() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/textarea.html", t.server.prefix))
            .await
            .unwrap();
        focus(&t.page, "textarea").await;
        let text = "This text goes onto two lines.\nThis character is 嗨.";
        t.page.keyboard().r#type(text).await.unwrap();
        assert_eq!(value(&t.page, "textarea").await, text);
    });
}

#[test]
fn should_specify_location() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/textarea.html", t.server.prefix))
            .await
            .unwrap();
        let last = capture_last_keydown(&t.page).await;
        focus(&t.page, "textarea").await;
        for (key, location, expected_key) in [
            ("Digit5", 0, "5"),
            ("ControlLeft", 1, "Control"),
            ("ControlRight", 2, "Control"),
            ("NumpadSubtract", 3, "-"),
        ] {
            t.page.keyboard().press(key).await.unwrap();
            let event: Value = last.json_value().await.unwrap();
            assert_eq!(event["location"], json!(location), "{key}");
            assert_eq!(event["key"], json!(expected_key), "{key}");
            assert_eq!(event["code"], json!(key), "{key}");
        }
    });
}

#[test]
fn should_specify_numpad_location_on_keyup() {
    run(|t| async move {
        press_and_expect(
            &t,
            "NumpadSubtract",
            &[
                "Keydown: - NumpadSubtract NUMPAD []",
                "Keypress: - NumpadSubtract NUMPAD 45 []",
                "Keyup: - NumpadSubtract NUMPAD []",
            ],
        )
        .await;
    });
}

#[test]
fn should_press_enter() {
    run(|t| async move {
        t.page.set_content("<textarea></textarea>").await.unwrap();
        focus(&t.page, "textarea").await;
        let last = capture_last_keydown(&t.page).await;
        for (key, expected_key, expected_code) in [
            ("Enter", "Enter", "Enter"),
            ("NumpadEnter", "Enter", "NumpadEnter"),
            ("\n", "Enter", "Enter"),
            ("\r", "Enter", "Enter"),
        ] {
            t.page.keyboard().press(key).await.unwrap();
            let event: Value = last.json_value().await.unwrap();
            assert_eq!(event["key"], json!(expected_key));
            assert_eq!(event["code"], json!(expected_code));
            assert_eq!(value(&t.page, "textarea").await, "\n");
            t.page
                .evaluate::<()>("() => document.querySelector('textarea').value = ''")
                .await
                .unwrap();
        }
    });
}

#[test]
fn should_press_audio_and_media_control_keys() {
    run(|t| async move {
        t.page.set_content("<input autofocus>").await.unwrap();
        focus(&t.page, "input").await;
        let last = capture_last_keydown(&t.page).await;
        for key in [
            "AudioVolumeMute",
            "AudioVolumeDown",
            "AudioVolumeUp",
            "MediaTrackNext",
            "MediaTrackPrevious",
            "MediaPlayPause",
        ] {
            t.page.keyboard().press(key).await.unwrap();
            let event: Value = last.json_value().await.unwrap();
            assert_eq!(event["key"], json!(key));
            assert_eq!(event["code"], json!(key));
            assert_eq!(event["location"], json!(0));
        }
    });
}

#[test]
fn should_throw_on_unknown_keys() {
    run(|t| async move {
        let keyboard = t.page.keyboard();
        let err = assert_err_contains(
            keyboard.press("NotARealKey").await,
            "Unknown key: \"NotARealKey\"",
        );
        assert_eq!(err.api(), Some("keyboard.press"));
        assert_err_contains(keyboard.press("ё").await, "Unknown key: \"ё\"");
        assert_err_contains(keyboard.press("😊").await, "Unknown key: \"😊\"");
    });
}

#[test]
fn should_type_emoji() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/textarea.html", t.server.prefix))
            .await
            .unwrap();
        focus(&t.page, "textarea").await;
        t.page
            .keyboard()
            .r#type("👹 Tokyo street Japan 🇯🇵")
            .await
            .unwrap();
        assert_eq!(value(&t.page, "textarea").await, "👹 Tokyo street Japan 🇯🇵");
    });
}

#[test]
fn should_type_emoji_into_an_iframe() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        let frame = attach_frame(
            &t.page,
            "emoji-test",
            &format!("{}/input/textarea.html", t.server.prefix),
        )
        .await;
        frame
            .evaluate::<()>("() => document.querySelector('textarea').focus()")
            .await
            .unwrap();
        t.page
            .keyboard()
            .r#type("👹 Tokyo street Japan 🇯🇵")
            .await
            .unwrap();
        let value: String = frame
            .evaluate("() => document.querySelector('textarea').value")
            .await
            .unwrap();
        assert_eq!(value, "👹 Tokyo street Japan 🇯🇵");
    });
}

#[test]
fn should_handle_select_all() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/textarea.html", t.server.prefix))
            .await
            .unwrap();
        focus(&t.page, "textarea").await;
        let keyboard = t.page.keyboard();
        keyboard.r#type("some text").await.unwrap();
        keyboard.down("ControlOrMeta").await.unwrap();
        keyboard.press("a").await.unwrap();
        keyboard.up("ControlOrMeta").await.unwrap();
        keyboard.press("Backspace").await.unwrap();
        assert_eq!(value(&t.page, "textarea").await, "");
    });
}

#[test]
fn should_be_able_to_prevent_select_all() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/textarea.html", t.server.prefix))
            .await
            .unwrap();
        focus(&t.page, "textarea").await;
        let keyboard = t.page.keyboard();
        keyboard.r#type("some text").await.unwrap();
        t.page
            .evaluate::<()>(
                "() => document.querySelector('textarea').addEventListener('keydown', event => { if (event.key === 'a' && (event.metaKey || event.ctrlKey)) event.preventDefault(); }, false)",
            )
            .await
            .unwrap();
        keyboard.down("ControlOrMeta").await.unwrap();
        keyboard.press("a").await.unwrap();
        keyboard.up("ControlOrMeta").await.unwrap();
        keyboard.press("Backspace").await.unwrap();
        assert_eq!(value(&t.page, "textarea").await, "some tex");
    });
}

#[test]
fn should_press_the_meta_key() {
    run(|t| async move {
        let last = capture_last_keydown(&t.page).await;
        t.page.keyboard().press("Meta").await.unwrap();
        let event: Value = last.json_value().await.unwrap();
        assert_eq!(event["key"], json!("Meta"));
        assert_eq!(event["code"], json!("MetaLeft"));
        assert_eq!(event["metaKey"], json!(true));
    });
}

#[test]
fn should_work_with_keyboard_events_with_empty_html() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        let last = capture_last_keydown(&t.page).await;
        t.page.keyboard().press("a").await.unwrap();
        let key: String = last.evaluate("l => l.key").await.unwrap();
        assert_eq!(key, "a");
    });
}

#[test]
fn should_work_after_a_cross_origin_navigation() {
    run(|t| async move {
        t.page.goto(&t.server.empty_page).await.unwrap();
        t.page
            .goto(format!("{}/empty.html", t.server.cross_process_prefix))
            .await
            .unwrap();
        let last = capture_last_keydown(&t.page).await;
        t.page.keyboard().press("a").await.unwrap();
        let key: String = last.evaluate("l => l.key").await.unwrap();
        assert_eq!(key, "a");
    });
}

#[test]
fn should_scroll_with_page_down() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/scrollable.html", t.server.prefix))
            .await
            .unwrap();
        // A click is required for WebKit to send the event into the body.
        t.page.mouse().click(1, 1).await.unwrap();
        t.page.keyboard().press("PageDown").await.unwrap();
        wait_for_function(&t.page, "() => scrollY > 0").await;
    });
}

#[test]
fn should_move_around_the_selection_in_a_contenteditable() {
    run(|t| async move {
        t.page
            .set_content("<div contenteditable></div>")
            .await
            .unwrap();
        focus(&t.page, "div").await;
        let modifier = if cfg!(target_os = "macos") {
            "Alt"
        } else {
            "Control"
        };
        let keyboard = t.page.keyboard();
        keyboard.r#type("Hello World").await.unwrap();
        keyboard.down(modifier).await.unwrap();
        keyboard.down("Shift").await.unwrap();
        keyboard.press("ArrowLeft").await.unwrap();
        keyboard.up("Shift").await.unwrap();
        keyboard.up(modifier).await.unwrap();
        let selection: String = t
            .page
            .evaluate("() => window.getSelection().toString()")
            .await
            .unwrap();
        assert_eq!(selection, "World");
    });
}

async fn button_clicked_with(t: &TestContext, key: &str) {
    t.page
        .set_content("<button type=\"button\">a11y</button>")
        .await
        .unwrap();
    let actual = t
        .page
        .evaluate_handle(
            "() => { const actual = { clicked: false }; document.querySelector('button').addEventListener('click', () => (actual.clicked = true)); return actual; }",
        )
        .await
        .unwrap();
    focus(&t.page, "button").await;
    t.page.keyboard().press(key).await.unwrap();
    let actual: Value = actual.json_value().await.unwrap();
    assert_eq!(actual["clicked"], json!(true));
}

#[test]
fn should_dispatch_a_click_event_on_a_button_when_space_gets_pressed() {
    run(|t| async move {
        button_clicked_with(&t, "Space").await;
    });
}

#[test]
fn should_dispatch_a_click_event_on_a_button_when_enter_gets_pressed() {
    run(|t| async move {
        button_clicked_with(&t, "Enter").await;
    });
}

#[test]
fn should_support_simple_copy_pasting() {
    run(|t| async move {
        t.page
            .set_content("<div contenteditable>123</div>")
            .await
            .unwrap();
        focus(&t.page, "div").await;
        let keyboard = t.page.keyboard();
        for key in ["KeyA", "KeyC", "KeyV", "KeyV"] {
            keyboard
                .press(format!("ControlOrMeta+{key}"))
                .await
                .unwrap();
        }
        let text: String = t
            .page
            .evaluate("() => document.querySelector('div').textContent")
            .await
            .unwrap();
        assert_eq!(text, "123123");
    });
}

#[test]
fn should_support_simple_cut_pasting() {
    run(|t| async move {
        t.page
            .set_content("<div contenteditable>123</div>")
            .await
            .unwrap();
        focus(&t.page, "div").await;
        let keyboard = t.page.keyboard();
        for key in ["KeyA", "KeyX", "KeyV", "KeyV"] {
            keyboard
                .press(format!("ControlOrMeta+{key}"))
                .await
                .unwrap();
        }
        let text: String = t
            .page
            .evaluate("() => document.querySelector('div').textContent")
            .await
            .unwrap();
        assert_eq!(text, "123123");
    });
}

#[test]
fn should_have_correct_keydown_keyup_order_when_pressing_escape_key() {
    run(|t| async move {
        press_and_expect(
            &t,
            "Escape",
            &[
                "Keydown: Escape Escape STANDARD []",
                "Keyup: Escape Escape STANDARD []",
            ],
        )
        .await;
    });
}

// ---- page-mouse.spec.ts ----

const CLICK_PROMISE: &str = "(type) => { window['clickPromise'] = new Promise(resolve => { document.addEventListener(type, event => { resolve({ type: event.type, detail: event.detail, clientX: event.clientX, clientY: event.clientY, isTrusted: event.isTrusted, button: event.button }); }); }); }";

#[test]
fn should_click_the_document() {
    run(|t| async move {
        t.page
            .evaluate::<()>(CLICK_PROMISE)
            .arg("click")
            .await
            .unwrap();
        t.page.mouse().click(50, 60).await.unwrap();
        let event: Value = t
            .page
            .evaluate("() => window['clickPromise']")
            .await
            .unwrap();
        assert_eq!(
            event,
            json!({ "type": "click", "detail": 1, "clientX": 50, "clientY": 60, "isTrusted": true, "button": 0 })
        );
    });
}

#[test]
fn should_dblclick_the_div() {
    run(|t| async move {
        t.page
            .set_content("<div style='width: 100px; height: 100px;'>Click me</div>")
            .await
            .unwrap();
        t.page
            .evaluate::<()>(CLICK_PROMISE)
            .arg("dblclick")
            .await
            .unwrap();
        t.page.mouse().dblclick(50, 60).await.unwrap();
        let event: Value = t
            .page
            .evaluate("() => window['clickPromise']")
            .await
            .unwrap();
        assert_eq!(
            event,
            json!({ "type": "dblclick", "detail": 2, "clientX": 50, "clientY": 60, "isTrusted": true, "button": 0 })
        );
    });
}

#[test]
fn down_and_up_should_generate_click() {
    run(|t| async move {
        t.page
            .evaluate::<()>(CLICK_PROMISE)
            .arg("click")
            .await
            .unwrap();
        let mouse = t.page.mouse();
        mouse.r#move(50, 60).await.unwrap();
        mouse.down().await.unwrap();
        mouse.up().await.unwrap();
        let event: Value = t
            .page
            .evaluate("() => window['clickPromise']")
            .await
            .unwrap();
        assert_eq!(
            event,
            json!({ "type": "click", "detail": 1, "clientX": 50, "clientY": 60, "isTrusted": true, "button": 0 })
        );
    });
}

#[test]
fn should_pointerdown_the_div_with_a_custom_button() {
    run(|t| async move {
        t.page
            .set_content("<div style='width: 100px; height: 100px;'>Click me</div>")
            .await
            .unwrap();
        t.page
            .evaluate::<()>(
                "() => { window['pointerdownPromise'] = new Promise(resolve => { document.querySelector('div').addEventListener('pointerdown', event => { resolve({ type: event.type, detail: event.detail, clientX: event.clientX, clientY: event.clientY, isTrusted: event.isTrusted, button: event.button, buttons: event.buttons, pointerId: event.pointerId }); }); }); }",
            )
            .await
            .unwrap();
        t.page
            .mouse()
            .click(50, 60)
            .button(MouseButton::Middle)
            .await
            .unwrap();
        let event: Value = t
            .page
            .evaluate("() => window['pointerdownPromise']")
            .await
            .unwrap();
        assert_eq!(event["type"], json!("pointerdown"));
        assert_eq!(event["detail"], json!(0));
        assert_eq!(event["clientX"], json!(50));
        assert_eq!(event["clientY"], json!(60));
        assert_eq!(event["isTrusted"], json!(true));
        assert_eq!(event["button"], json!(1));
        assert_eq!(event["buttons"], json!(4));
        assert!(event["pointerId"].as_i64().unwrap() >= 0);
    });
}

#[test]
fn should_report_correct_buttons_property() {
    run(|t| async move {
        t.page
            .evaluate::<()>(
                "() => { window.__EVENTS = []; const handler = event => { window.__EVENTS.push({ type: event.type, button: event.button, buttons: event.buttons }); }; window.addEventListener('mousedown', handler, false); window.addEventListener('mouseup', handler, false); }",
            )
            .await
            .unwrap();
        let mouse = t.page.mouse();
        mouse.r#move(50, 60).await.unwrap();
        mouse.down().button(MouseButton::Middle).await.unwrap();
        mouse.down().button(MouseButton::Left).await.unwrap();
        mouse.up().button(MouseButton::Middle).await.unwrap();
        mouse.up().button(MouseButton::Left).await.unwrap();
        let events: Value = t.page.evaluate("() => window.__EVENTS").await.unwrap();
        assert_eq!(
            events,
            json!([
                { "type": "mousedown", "button": 1, "buttons": 4 },
                { "type": "mousedown", "button": 0, "buttons": 5 },
                { "type": "mouseup", "button": 1, "buttons": 1 },
                { "type": "mouseup", "button": 0, "buttons": 0 },
            ])
        );
    });
}

#[test]
fn should_report_correct_pointer_type_property() {
    run(|t| async move {
        let mouse = t.page.mouse();
        mouse.r#move(50, 60).await.unwrap();
        t.page
            .evaluate::<()>(
                "() => { window.__EVENTS = []; const handler = event => { window.__EVENTS.push({ type: event.type, pointerType: event.pointerType }); }; window.addEventListener('pointerdown', handler, false); window.addEventListener('pointermove', handler, false); window.addEventListener('pointerup', handler, false); }",
            )
            .await
            .unwrap();
        mouse.r#move(60, 50).await.unwrap();
        mouse.down().await.unwrap();
        mouse.up().await.unwrap();
        let events: Value = t.page.evaluate("() => window.__EVENTS").await.unwrap();
        assert_eq!(
            events,
            json!([
                { "type": "pointermove", "pointerType": "mouse" },
                { "type": "pointerdown", "pointerType": "mouse" },
                { "type": "pointerup", "pointerType": "mouse" },
            ])
        );
    });
}

#[test]
fn should_select_the_text_with_mouse() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/textarea.html", t.server.prefix))
            .await
            .unwrap();
        focus(&t.page, "textarea").await;
        let text = "This is the text that we are going to try to select. Let's see how it goes.";
        t.page.keyboard().r#type(text).await.unwrap();
        t.page
            .evaluate::<()>(
                "() => new Promise(f => requestAnimationFrame(() => requestAnimationFrame(f)))",
            )
            .await
            .unwrap();
        t.page
            .evaluate::<()>("() => document.querySelector('textarea').scrollTop = 0")
            .await
            .unwrap();
        let (x, y): (f64, f64) = t
            .page
            .evaluate("() => { const rect = document.querySelector('textarea').getBoundingClientRect(); return [rect.left, rect.top]; }")
            .await
            .unwrap();
        let mouse = t.page.mouse();
        mouse.r#move(x + 2.0, y + 2.0).await.unwrap();
        mouse.down().await.unwrap();
        mouse.r#move(200, 200).await.unwrap();
        mouse.up().await.unwrap();
        let selected: String = t
            .page
            .evaluate("() => { const textarea = document.querySelector('textarea'); return textarea.value.substring(textarea.selectionStart, textarea.selectionEnd); }")
            .await
            .unwrap();
        assert_eq!(selected, text);
    });
}

#[test]
fn should_set_modifier_keys_on_click() {
    // `page.click('#button-3')` replaced by a mouse click at the button's center.
    run(|t| async move {
        t.page
            .goto(format!("{}/input/scrollable.html", t.server.prefix))
            .await
            .unwrap();
        t.page
            .evaluate::<()>("() => document.querySelector('#button-3').addEventListener('mousedown', e => window['lastEvent'] = e, true)")
            .await
            .unwrap();
        let (x, y): (f64, f64) = t
            .page
            .evaluate("() => { const r = document.querySelector('#button-3').getBoundingClientRect(); return [r.x + r.width / 2, r.y + r.height / 2]; }")
            .await
            .unwrap();
        let modifiers = [
            ("Shift", "shiftKey"),
            ("Control", "ctrlKey"),
            ("Alt", "altKey"),
            ("Meta", "metaKey"),
        ];
        for (modifier, property) in modifiers {
            t.page.keyboard().down(modifier).await.unwrap();
            t.page.mouse().click(x, y).await.unwrap();
            let set: bool = t
                .page
                .evaluate("mod => window['lastEvent'][mod]")
                .arg(property)
                .await
                .unwrap();
            assert!(set, "{property} should be true");
            t.page.keyboard().up(modifier).await.unwrap();
        }
        t.page.mouse().click(x, y).await.unwrap();
        for (_, property) in modifiers {
            let set: bool = t
                .page
                .evaluate("mod => window['lastEvent'][mod]")
                .arg(property)
                .await
                .unwrap();
            assert!(!set, "{property} should be false");
        }
    });
}

#[test]
fn should_tween_mouse_movement() {
    run(|t| async move {
        let mouse = t.page.mouse();
        mouse.r#move(100, 100).await.unwrap();
        t.page
            .evaluate::<()>(
                "() => { window['result'] = []; document.addEventListener('mousemove', event => { window['result'].push([event.clientX, event.clientY, event.movementX, event.movementY]); }); }",
            )
            .await
            .unwrap();
        mouse.r#move(200, 300).steps(5).await.unwrap();
        let result: Value = t.page.evaluate("result").await.unwrap();
        assert_eq!(
            result,
            json!([
                [120, 140, 20, 40],
                [140, 180, 20, 40],
                [160, 220, 20, 40],
                [180, 260, 20, 40],
                [200, 300, 20, 40]
            ])
        );
    });
}

#[test]
fn should_always_round_down() {
    run(|t| async move {
        t.page
            .evaluate::<()>("() => { document.addEventListener('mousedown', event => { window['result'] = [event.clientX, event.clientY]; }); }")
            .await
            .unwrap();
        t.page.mouse().click(50.1, 50.9).await.unwrap();
        let result: Value = t.page.evaluate("result").await.unwrap();
        assert_eq!(result, json!([50, 50]));
    });
}

#[test]
fn should_not_crash_on_mouse_drag_with_any_button() {
    run(|t| async move {
        t.page
            .evaluate::<()>(
                "() => window.addEventListener('contextmenu', e => e.preventDefault(), false)",
            )
            .await
            .unwrap();
        let mouse = t.page.mouse();
        for button in [MouseButton::Left, MouseButton::Middle, MouseButton::Right] {
            mouse.r#move(50, 50).await.unwrap();
            mouse.down().button(button).await.unwrap();
            mouse.r#move(100, 100).await.unwrap();
        }
    });
}

#[test]
fn should_drag_and_drop_with_the_mouse() {
    // Exercises the drag manager (`Input.setInterceptDrags`), like
    // `page-drag.spec.ts` "should work" with raw mouse calls.
    run(|t| async move {
        t.page
            .set_content(
                "<div id=source draggable=true style='width:50px;height:50px;background:red'>drag</div>
                 <div id=target style='width:100px;height:100px;background:blue;margin-top:20px'></div>
                 <script>
                   window.events = [];
                   source.addEventListener('dragstart', e => { e.dataTransfer.setData('text/plain', 'hello'); window.events.push('dragstart'); });
                   target.addEventListener('dragover', e => e.preventDefault());
                   target.addEventListener('drop', e => { e.preventDefault(); window.events.push('drop:' + e.dataTransfer.getData('text/plain')); });
                 </script>",
            )
            .await
            .unwrap();
        let mouse = t.page.mouse();
        mouse.r#move(25, 25).await.unwrap();
        mouse.down().await.unwrap();
        mouse.r#move(50, 120).await.unwrap();
        mouse.up().await.unwrap();
        let events: Vec<String> = t.page.evaluate("() => window.events").await.unwrap();
        assert_eq!(events, vec!["dragstart", "drop:hello"]);
    });
}

// ---- wheel.spec.ts ----

const LISTEN_FOR_WHEEL: &str = "selector => { document.querySelector(selector).addEventListener('wheel', event => { window['lastEvent'] = { deltaX: event.deltaX, deltaY: event.deltaY, clientX: event.clientX, clientY: event.clientY, deltaMode: event.deltaMode, ctrlKey: event.ctrlKey, shiftKey: event.shiftKey, altKey: event.altKey, metaKey: event.metaKey }; }, { passive: false }); }";

async fn expect_wheel_event(page: &Page, mut expected: Value) {
    wait_for_function(page, "() => !!window['lastEvent']").await;
    let mut received: Value = page.evaluate("window.lastEvent").await.unwrap();
    if cfg!(target_os = "macos") {
        // Chromium scales deltas by the host device scale factor on macOS.
        for v in [&mut received, &mut expected] {
            let o = v.as_object_mut().unwrap();
            o.remove("deltaX");
            o.remove("deltaY");
        }
    }
    assert_eq!(received, expected);
}

fn wheel_event(dx: i32, dy: i32, shift: bool) -> Value {
    json!({
        "deltaX": dx, "deltaY": dy, "clientX": 50, "clientY": 60, "deltaMode": 0,
        "ctrlKey": false, "shiftKey": shift, "altKey": false, "metaKey": false,
    })
}

async fn setup_wheel_page(page: &Page) {
    page.set_content("<div style=\"width: 5000px; height: 5000px;\"></div>")
        .await
        .unwrap();
    page.mouse().r#move(50, 60).await.unwrap();
    page.evaluate::<()>(LISTEN_FOR_WHEEL)
        .arg("div")
        .await
        .unwrap();
}

#[test]
fn should_dispatch_wheel_events() {
    run(|t| async move {
        setup_wheel_page(&t.page).await;
        t.page.mouse().wheel(0, 100).await.unwrap();
        wait_for_function(&t.page, "window.scrollY === 100").await;
        expect_wheel_event(&t.page, wheel_event(0, 100, false)).await;
    });
}

#[test]
fn should_scroll_when_nobody_is_listening() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/scrollable.html", t.server.prefix))
            .await
            .unwrap();
        t.page.mouse().r#move(50, 60).await.unwrap();
        t.page.mouse().wheel(0, 100).await.unwrap();
        wait_for_function(&t.page, "window.scrollY === 100").await;
    });
}

#[test]
fn should_set_the_modifiers() {
    run(|t| async move {
        setup_wheel_page(&t.page).await;
        t.page.keyboard().down("Shift").await.unwrap();
        t.page.mouse().wheel(0, 100).await.unwrap();
        expect_wheel_event(&t.page, wheel_event(0, 100, true)).await;
    });
}

#[test]
fn should_scroll_horizontally() {
    run(|t| async move {
        setup_wheel_page(&t.page).await;
        t.page.mouse().wheel(100, 0).await.unwrap();
        expect_wheel_event(&t.page, wheel_event(100, 0, false)).await;
        wait_for_function(&t.page, "window.scrollX === 100").await;
    });
}

#[test]
fn should_work_when_the_event_is_canceled() {
    run(|t| async move {
        setup_wheel_page(&t.page).await;
        t.page
            .evaluate::<()>("() => document.querySelector('div').addEventListener('wheel', e => e.preventDefault())")
            .await
            .unwrap();
        for _ in 0..10 {
            t.page
                .evaluate::<()>(
                    "() => new Promise(f => requestAnimationFrame(() => requestAnimationFrame(f)))",
                )
                .await
                .unwrap();
        }
        t.page.mouse().wheel(0, 100).await.unwrap();
        expect_wheel_event(&t.page, wheel_event(0, 100, false)).await;
        let scroll_y: f64 = t.page.evaluate("window.scrollY").await.unwrap();
        assert_eq!(scroll_y, 0.0);
    });
}

// ---- library/tap.spec.ts ----

#[test]
fn should_send_well_formed_touch_points() {
    run(|t| async move {
        let collect = "type => new Promise(resolve => { document.addEventListener(type, event => { resolve([...event.touches].map(t => ({ identifier: t.identifier, clientX: t.clientX, clientY: t.clientY, pageX: t.pageX, pageY: t.pageY, radiusX: t.radiusX, radiusY: t.radiusY, rotationAngle: t.rotationAngle, force: t.force }))); }, false); })";
        t.page
            .evaluate::<()>(&format!(
                "() => {{ window.__touchstart = ({collect})('touchstart'); window.__touchend = ({collect})('touchend'); }}"
            ))
            .await
            .unwrap();
        t.page.touchscreen().tap(40, 60).await.unwrap();
        let touchstart: Value = t.page.evaluate("() => window.__touchstart").await.unwrap();
        let touchend: Value = t.page.evaluate("() => window.__touchend").await.unwrap();
        assert_eq!(
            touchstart,
            json!([{ "clientX": 40, "clientY": 60, "force": 1, "identifier": 0, "pageX": 40, "pageY": 60, "radiusX": 1, "radiusY": 1, "rotationAngle": 0 }])
        );
        assert_eq!(touchend, json!([]));
    });
}
