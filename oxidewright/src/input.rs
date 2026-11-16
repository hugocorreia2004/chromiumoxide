//! Keyboard, mouse and touchscreen input, ported from Playwright's
//! `server/input.ts`, `server/chromium/crInput.ts`,
//! `server/usKeyboardLayout.ts`, `server/macEditingCommands.ts` and
//! `server/chromium/crDragDrop.ts`.
//!
//! Everything goes through the `Input` domain of the page's own session
//! (`Input.dispatchKeyEvent`, `Input.dispatchMouseEvent`,
//! `Input.insertText`, `Input.dispatchTouchEvent`, `Input.dispatchDragEvent`).

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::future::{Future, IntoFuture};
use std::pin::Pin;
use std::str::FromStr;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use chromiumoxide::cdp::browser_protocol::input::{
    DispatchDragEventParams, DispatchDragEventType, DispatchKeyEventParams, DispatchKeyEventType,
    DispatchMouseEventParams, DispatchMouseEventType, DispatchTouchEventParams,
    DispatchTouchEventType, DragData, EventDragIntercepted, InsertTextParams,
    MouseButton as CdpMouseButton, SetInterceptDragsParams, TouchPoint,
};
use futures::StreamExt;

use crate::error::{Error, Result, ResultExt};
use crate::execution_context::{Evaluated, World};
use crate::js::JsValue;
use crate::page::{Page, PageInner};

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

// ----------------------------------------------------------------------
// Public enums
// ----------------------------------------------------------------------

/// A keyboard modifier, as accepted by the `modifiers` option of actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyboardModifier {
    /// `Alt`
    Alt,
    /// `Control`
    Control,
    /// `Meta` on macOS, `Control` elsewhere.
    ControlOrMeta,
    /// `Meta`
    Meta,
    /// `Shift`
    Shift,
}

impl KeyboardModifier {
    /// The Playwright name of the modifier.
    pub fn as_str(&self) -> &'static str {
        match self {
            KeyboardModifier::Alt => "Alt",
            KeyboardModifier::Control => "Control",
            KeyboardModifier::ControlOrMeta => "ControlOrMeta",
            KeyboardModifier::Meta => "Meta",
            KeyboardModifier::Shift => "Shift",
        }
    }

    /// `ControlOrMeta` resolved for the current platform.
    pub(crate) fn resolve(self) -> KeyboardModifier {
        match self {
            KeyboardModifier::ControlOrMeta => {
                if IS_MAC {
                    KeyboardModifier::Meta
                } else {
                    KeyboardModifier::Control
                }
            }
            other => other,
        }
    }

    fn from_key(key: &str) -> Option<KeyboardModifier> {
        match key {
            "Alt" => Some(KeyboardModifier::Alt),
            "Control" => Some(KeyboardModifier::Control),
            "Meta" => Some(KeyboardModifier::Meta),
            "Shift" => Some(KeyboardModifier::Shift),
            _ => None,
        }
    }
}

impl fmt::Display for KeyboardModifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for KeyboardModifier {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "Alt" => Ok(KeyboardModifier::Alt),
            "Control" => Ok(KeyboardModifier::Control),
            "ControlOrMeta" => Ok(KeyboardModifier::ControlOrMeta),
            "Meta" => Ok(KeyboardModifier::Meta),
            "Shift" => Ok(KeyboardModifier::Shift),
            other => Err(Error::invalid_argument(format!("Unknown modifier {other}"))),
        }
    }
}

/// A mouse button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum MouseButton {
    /// The primary button (default).
    #[default]
    Left,
    /// The secondary button.
    Right,
    /// The middle button (wheel).
    Middle,
}

impl MouseButton {
    /// The Playwright name of the button.
    pub fn as_str(&self) -> &'static str {
        match self {
            MouseButton::Left => "left",
            MouseButton::Right => "right",
            MouseButton::Middle => "middle",
        }
    }

    fn to_cdp(self) -> CdpMouseButton {
        match self {
            MouseButton::Left => CdpMouseButton::Left,
            MouseButton::Right => CdpMouseButton::Right,
            MouseButton::Middle => CdpMouseButton::Middle,
        }
    }
}

impl fmt::Display for MouseButton {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for MouseButton {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "left" => Ok(MouseButton::Left),
            "right" => Ok(MouseButton::Right),
            "middle" => Ok(MouseButton::Middle),
            _ => Err(Error::invalid_argument(
                "button: expected one of (left|right|middle)",
            )),
        }
    }
}

// ----------------------------------------------------------------------
// Keyboard layout
// ----------------------------------------------------------------------

const IS_MAC: bool = cfg!(target_os = "macos");

/// `keypadLocation` of `usKeyboardLayout.ts`.
const KEYPAD_LOCATION: i64 = 3;

/// The modifiers in Playwright's `kModifiers` order.
const MODIFIERS: [KeyboardModifier; 4] = [
    KeyboardModifier::Alt,
    KeyboardModifier::Control,
    KeyboardModifier::Meta,
    KeyboardModifier::Shift,
];

/// `KeyDefinition` of `usKeyboardLayout.ts`.
struct KeyDefinition {
    code: &'static str,
    key: &'static str,
    key_code: i64,
    key_code_without_location: Option<i64>,
    shift_key: Option<&'static str>,
    shift_key_code: Option<i64>,
    text: Option<&'static str>,
    location: i64,
}

const fn def(code: &'static str, key_code: i64, key: &'static str) -> KeyDefinition {
    KeyDefinition {
        code,
        key,
        key_code,
        key_code_without_location: None,
        shift_key: None,
        shift_key_code: None,
        text: None,
        location: 0,
    }
}

const fn shift(
    code: &'static str,
    key_code: i64,
    shift_key: &'static str,
    key: &'static str,
) -> KeyDefinition {
    KeyDefinition {
        shift_key: Some(shift_key),
        ..def(code, key_code, key)
    }
}

const fn located(
    code: &'static str,
    key_code: i64,
    without_location: Option<i64>,
    key: &'static str,
    location: i64,
) -> KeyDefinition {
    KeyDefinition {
        key_code_without_location: without_location,
        location,
        ..def(code, key_code, key)
    }
}

const fn numpad(
    code: &'static str,
    key_code: i64,
    shift_key_code: i64,
    key: &'static str,
    shift_key: &'static str,
) -> KeyDefinition {
    KeyDefinition {
        shift_key: Some(shift_key),
        shift_key_code: Some(shift_key_code),
        location: KEYPAD_LOCATION,
        ..def(code, key_code, key)
    }
}

/// `USKeyboardLayout`, in declaration order (order matters for aliases).
const US_KEYBOARD_LAYOUT: &[KeyDefinition] = &[
    // Functions row
    def("Escape", 27, "Escape"),
    def("F1", 112, "F1"),
    def("F2", 113, "F2"),
    def("F3", 114, "F3"),
    def("F4", 115, "F4"),
    def("F5", 116, "F5"),
    def("F6", 117, "F6"),
    def("F7", 118, "F7"),
    def("F8", 119, "F8"),
    def("F9", 120, "F9"),
    def("F10", 121, "F10"),
    def("F11", 122, "F11"),
    def("F12", 123, "F12"),
    // Numbers row
    shift("Backquote", 192, "~", "`"),
    shift("Digit1", 49, "!", "1"),
    shift("Digit2", 50, "@", "2"),
    shift("Digit3", 51, "#", "3"),
    shift("Digit4", 52, "$", "4"),
    shift("Digit5", 53, "%", "5"),
    shift("Digit6", 54, "^", "6"),
    shift("Digit7", 55, "&", "7"),
    shift("Digit8", 56, "*", "8"),
    shift("Digit9", 57, "(", "9"),
    shift("Digit0", 48, ")", "0"),
    shift("Minus", 189, "_", "-"),
    shift("Equal", 187, "+", "="),
    shift("Backslash", 220, "|", "\\"),
    def("Backspace", 8, "Backspace"),
    // First row
    def("Tab", 9, "Tab"),
    shift("KeyQ", 81, "Q", "q"),
    shift("KeyW", 87, "W", "w"),
    shift("KeyE", 69, "E", "e"),
    shift("KeyR", 82, "R", "r"),
    shift("KeyT", 84, "T", "t"),
    shift("KeyY", 89, "Y", "y"),
    shift("KeyU", 85, "U", "u"),
    shift("KeyI", 73, "I", "i"),
    shift("KeyO", 79, "O", "o"),
    shift("KeyP", 80, "P", "p"),
    shift("BracketLeft", 219, "{", "["),
    shift("BracketRight", 221, "}", "]"),
    // Second row
    def("CapsLock", 20, "CapsLock"),
    shift("KeyA", 65, "A", "a"),
    shift("KeyS", 83, "S", "s"),
    shift("KeyD", 68, "D", "d"),
    shift("KeyF", 70, "F", "f"),
    shift("KeyG", 71, "G", "g"),
    shift("KeyH", 72, "H", "h"),
    shift("KeyJ", 74, "J", "j"),
    shift("KeyK", 75, "K", "k"),
    shift("KeyL", 76, "L", "l"),
    shift("Semicolon", 186, ":", ";"),
    shift("Quote", 222, "\"", "'"),
    KeyDefinition {
        text: Some("\r"),
        ..def("Enter", 13, "Enter")
    },
    // Third row
    located("ShiftLeft", 160, Some(16), "Shift", 1),
    shift("KeyZ", 90, "Z", "z"),
    shift("KeyX", 88, "X", "x"),
    shift("KeyC", 67, "C", "c"),
    shift("KeyV", 86, "V", "v"),
    shift("KeyB", 66, "B", "b"),
    shift("KeyN", 78, "N", "n"),
    shift("KeyM", 77, "M", "m"),
    shift("Comma", 188, "<", ","),
    shift("Period", 190, ">", "."),
    shift("Slash", 191, "?", "/"),
    located("ShiftRight", 161, Some(16), "Shift", 2),
    // Last row
    located("ControlLeft", 162, Some(17), "Control", 1),
    located("MetaLeft", 91, None, "Meta", 1),
    located("AltLeft", 164, Some(18), "Alt", 1),
    def("Space", 32, " "),
    located("AltRight", 165, Some(18), "Alt", 2),
    def("AltGraph", 225, "AltGraph"),
    located("MetaRight", 92, None, "Meta", 2),
    def("ContextMenu", 93, "ContextMenu"),
    located("ControlRight", 163, Some(17), "Control", 2),
    // Center block
    def("PrintScreen", 44, "PrintScreen"),
    def("ScrollLock", 145, "ScrollLock"),
    def("Pause", 19, "Pause"),
    def("PageUp", 33, "PageUp"),
    def("PageDown", 34, "PageDown"),
    def("Insert", 45, "Insert"),
    def("Delete", 46, "Delete"),
    def("Home", 36, "Home"),
    def("End", 35, "End"),
    def("ArrowLeft", 37, "ArrowLeft"),
    def("ArrowUp", 38, "ArrowUp"),
    def("ArrowRight", 39, "ArrowRight"),
    def("ArrowDown", 40, "ArrowDown"),
    // Media keys
    def("AudioVolumeMute", 173, "AudioVolumeMute"),
    def("AudioVolumeDown", 174, "AudioVolumeDown"),
    def("AudioVolumeUp", 175, "AudioVolumeUp"),
    def("MediaTrackNext", 176, "MediaTrackNext"),
    def("MediaTrackPrevious", 177, "MediaTrackPrevious"),
    def("MediaPlayPause", 179, "MediaPlayPause"),
    // Numpad
    def("NumLock", 144, "NumLock"),
    located("NumpadDivide", 111, None, "/", KEYPAD_LOCATION),
    located("NumpadMultiply", 106, None, "*", KEYPAD_LOCATION),
    located("NumpadSubtract", 109, None, "-", KEYPAD_LOCATION),
    numpad("Numpad7", 36, 103, "Home", "7"),
    numpad("Numpad8", 38, 104, "ArrowUp", "8"),
    numpad("Numpad9", 33, 105, "PageUp", "9"),
    numpad("Numpad4", 37, 100, "ArrowLeft", "4"),
    numpad("Numpad5", 12, 101, "Clear", "5"),
    numpad("Numpad6", 39, 102, "ArrowRight", "6"),
    located("NumpadAdd", 107, None, "+", KEYPAD_LOCATION),
    numpad("Numpad1", 35, 97, "End", "1"),
    numpad("Numpad2", 40, 98, "ArrowDown", "2"),
    numpad("Numpad3", 34, 99, "PageDown", "3"),
    numpad("Numpad0", 45, 96, "Insert", "0"),
    numpad("NumpadDecimal", 46, 110, "Delete", "."),
    KeyDefinition {
        text: Some("\r"),
        location: KEYPAD_LOCATION,
        ..def("NumpadEnter", 13, "Enter")
    },
];

/// `KeyDescription` of `input.ts`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct KeyDescription {
    pub(crate) key_code: i64,
    pub(crate) key_code_without_location: i64,
    pub(crate) key: String,
    pub(crate) text: String,
    pub(crate) code: String,
    pub(crate) location: i64,
    pub(crate) shifted: Option<Box<KeyDescription>>,
}

fn aliases(code: &str) -> &'static [&'static str] {
    match code {
        "ShiftLeft" => &["Shift"],
        "ControlLeft" => &["Control"],
        "AltLeft" => &["Alt"],
        "MetaLeft" => &["Meta"],
        "Enter" => &["\n", "\r"],
        _ => &[],
    }
}

/// `buildLayoutClosure(USKeyboardLayout)`.
fn us_keyboard_layout() -> &'static HashMap<String, KeyDescription> {
    static LAYOUT: OnceLock<HashMap<String, KeyDescription>> = OnceLock::new();
    LAYOUT.get_or_init(|| {
        let mut result = HashMap::new();
        for definition in US_KEYBOARD_LAYOUT {
            let mut description = KeyDescription {
                key: definition.key.to_string(),
                key_code: definition.key_code,
                key_code_without_location: definition
                    .key_code_without_location
                    .unwrap_or(definition.key_code),
                code: definition.code.to_string(),
                text: definition.text.unwrap_or("").to_string(),
                location: definition.location,
                shifted: None,
            };
            if definition.key.chars().count() == 1 {
                description.text = description.key.clone();
            }
            let shifted = definition.shift_key.map(|shift_key| {
                let mut shifted = description.clone();
                shifted.key = shift_key.to_string();
                shifted.text = shift_key.to_string();
                if let Some(code) = definition.shift_key_code {
                    shifted.key_code = code;
                    shifted.key_code_without_location = code;
                }
                shifted
            });
            result.insert(
                definition.code.to_string(),
                KeyDescription {
                    shifted: shifted.clone().map(Box::new),
                    ..description.clone()
                },
            );
            for alias in aliases(definition.code) {
                result.insert(alias.to_string(), description.clone());
            }
            // Do not use numpad when converting keys to codes.
            if definition.location != 0 {
                continue;
            }
            if description.key.chars().count() == 1 {
                result.insert(description.key.clone(), description.clone());
            }
            if let Some(shifted) = shifted {
                result.insert(shifted.key.clone(), shifted);
            }
        }
        result
    })
}

/// `resolveSmartModifierString`.
fn resolve_smart_modifier_string(key: &str) -> &str {
    if key == "ControlOrMeta" {
        if IS_MAC { "Meta" } else { "Control" }
    } else {
        key
    }
}

/// Split `Shift+a` style key combinations (`press`'s `split`).
pub(crate) fn split_key_combination(key: &str) -> Vec<String> {
    let mut keys = Vec::new();
    let mut building = String::new();
    for c in key.chars() {
        if c == '+' && !building.is_empty() {
            keys.push(std::mem::take(&mut building));
        } else {
            building.push(c);
        }
    }
    keys.push(building);
    keys
}

/// `toModifiersMask` of `crProtocolHelper.ts`.
pub(crate) fn modifiers_mask(modifiers: &[KeyboardModifier]) -> i64 {
    let mut mask = 0;
    if modifiers.contains(&KeyboardModifier::Alt) {
        mask |= 1;
    }
    if modifiers.contains(&KeyboardModifier::Control) {
        mask |= 2;
    }
    if modifiers.contains(&KeyboardModifier::Meta) {
        mask |= 4;
    }
    if modifiers.contains(&KeyboardModifier::Shift) {
        mask |= 8;
    }
    mask
}

/// `toButtonsMask` of `crProtocolHelper.ts`.
pub(crate) fn buttons_mask(buttons: &[MouseButton]) -> i64 {
    let mut mask = 0;
    if buttons.contains(&MouseButton::Left) {
        mask |= 1;
    }
    if buttons.contains(&MouseButton::Right) {
        mask |= 2;
    }
    if buttons.contains(&MouseButton::Middle) {
        mask |= 4;
    }
    mask
}

/// `_keyDescriptionForString`, given the currently pressed modifiers.
fn key_description_for_string(
    key: &str,
    pressed_modifiers: &[KeyboardModifier],
) -> Result<KeyDescription> {
    let key_string = resolve_smart_modifier_string(key);
    let Some(description) = us_keyboard_layout().get(key_string) else {
        return Err(Error::invalid_argument(format!(
            "Unknown key: \"{key_string}\""
        )));
    };
    let shift = pressed_modifiers.contains(&KeyboardModifier::Shift);
    let mut description = match (&description.shifted, shift) {
        (Some(shifted), true) => (**shifted).clone(),
        _ => description.clone(),
    };
    // If any modifiers besides shift are pressed, no text should be sent.
    if pressed_modifiers.len() > 1 || (!shift && pressed_modifiers.len() == 1) {
        description.text = String::new();
    }
    Ok(description)
}

/// `macEditingCommands` lookup, like `RawKeyboardImpl._commandsForCode`.
fn commands_for_code(code: &str, modifiers: &[KeyboardModifier]) -> Vec<String> {
    if !IS_MAC {
        return Vec::new();
    }
    let mut parts: Vec<&str> = Vec::new();
    for modifier in [
        KeyboardModifier::Shift,
        KeyboardModifier::Control,
        KeyboardModifier::Alt,
        KeyboardModifier::Meta,
    ] {
        if modifiers.contains(&modifier) {
            parts.push(modifier.as_str());
        }
    }
    parts.push(code);
    let shortcut = parts.join("+");
    mac_editing_commands(&shortcut)
        .iter()
        .filter(|c| !c.starts_with("insert"))
        .map(|c| c[..c.len() - 1].to_string())
        .collect()
}

/// `macEditingCommands` of `server/macEditingCommands.ts`.
fn mac_editing_commands(shortcut: &str) -> &'static [&'static str] {
    match shortcut {
        "Backspace" => &["deleteBackward:"],
        "Enter" => &["insertNewline:"],
        "NumpadEnter" => &["insertNewline:"],
        "Escape" => &["cancelOperation:"],
        "ArrowUp" => &["moveUp:"],
        "ArrowDown" => &["moveDown:"],
        "ArrowLeft" => &["moveLeft:"],
        "ArrowRight" => &["moveRight:"],
        "F5" => &["complete:"],
        "Delete" => &["deleteForward:"],
        "Home" => &["scrollToBeginningOfDocument:"],
        "End" => &["scrollToEndOfDocument:"],
        "PageUp" => &["scrollPageUp:"],
        "PageDown" => &["scrollPageDown:"],
        "Shift+Backspace" => &["deleteBackward:"],
        "Shift+Enter" => &["insertNewline:"],
        "Shift+NumpadEnter" => &["insertNewline:"],
        "Shift+Escape" => &["cancelOperation:"],
        "Shift+ArrowUp" => &["moveUpAndModifySelection:"],
        "Shift+ArrowDown" => &["moveDownAndModifySelection:"],
        "Shift+ArrowLeft" => &["moveLeftAndModifySelection:"],
        "Shift+ArrowRight" => &["moveRightAndModifySelection:"],
        "Shift+F5" => &["complete:"],
        "Shift+Delete" => &["deleteForward:"],
        "Shift+Home" => &["moveToBeginningOfDocumentAndModifySelection:"],
        "Shift+End" => &["moveToEndOfDocumentAndModifySelection:"],
        "Shift+PageUp" => &["pageUpAndModifySelection:"],
        "Shift+PageDown" => &["pageDownAndModifySelection:"],
        "Shift+Numpad5" => &["delete:"],
        "Control+Tab" => &["selectNextKeyView:"],
        "Control+Enter" => &["insertLineBreak:"],
        "Control+NumpadEnter" => &["insertLineBreak:"],
        "Control+Quote" => &["insertSingleQuoteIgnoringSubstitution:"],
        "Control+KeyA" => &["moveToBeginningOfParagraph:"],
        "Control+KeyB" => &["moveBackward:"],
        "Control+KeyD" => &["deleteForward:"],
        "Control+KeyE" => &["moveToEndOfParagraph:"],
        "Control+KeyF" => &["moveForward:"],
        "Control+KeyH" => &["deleteBackward:"],
        "Control+KeyK" => &["deleteToEndOfParagraph:"],
        "Control+KeyL" => &["centerSelectionInVisibleArea:"],
        "Control+KeyN" => &["moveDown:"],
        "Control+KeyO" => &["insertNewlineIgnoringFieldEditor:", "moveBackward:"],
        "Control+KeyP" => &["moveUp:"],
        "Control+KeyT" => &["transpose:"],
        "Control+KeyV" => &["pageDown:"],
        "Control+KeyY" => &["yank:"],
        "Control+Backspace" => &["deleteBackwardByDecomposingPreviousCharacter:"],
        "Control+ArrowUp" => &["scrollPageUp:"],
        "Control+ArrowDown" => &["scrollPageDown:"],
        "Control+ArrowLeft" => &["moveToLeftEndOfLine:"],
        "Control+ArrowRight" => &["moveToRightEndOfLine:"],
        "Shift+Control+Enter" => &["insertLineBreak:"],
        "Shift+Control+NumpadEnter" => &["insertLineBreak:"],
        "Shift+Control+Tab" => &["selectPreviousKeyView:"],
        "Shift+Control+Quote" => &["insertDoubleQuoteIgnoringSubstitution:"],
        "Shift+Control+KeyA" => &["moveToBeginningOfParagraphAndModifySelection:"],
        "Shift+Control+KeyB" => &["moveBackwardAndModifySelection:"],
        "Shift+Control+KeyE" => &["moveToEndOfParagraphAndModifySelection:"],
        "Shift+Control+KeyF" => &["moveForwardAndModifySelection:"],
        "Shift+Control+KeyN" => &["moveDownAndModifySelection:"],
        "Shift+Control+KeyP" => &["moveUpAndModifySelection:"],
        "Shift+Control+KeyV" => &["pageDownAndModifySelection:"],
        "Shift+Control+Backspace" => &["deleteBackwardByDecomposingPreviousCharacter:"],
        "Shift+Control+ArrowUp" => &["scrollPageUp:"],
        "Shift+Control+ArrowDown" => &["scrollPageDown:"],
        "Shift+Control+ArrowLeft" => &["moveToLeftEndOfLineAndModifySelection:"],
        "Shift+Control+ArrowRight" => &["moveToRightEndOfLineAndModifySelection:"],
        "Alt+Backspace" => &["deleteWordBackward:"],
        "Alt+Enter" => &["insertNewlineIgnoringFieldEditor:"],
        "Alt+NumpadEnter" => &["insertNewlineIgnoringFieldEditor:"],
        "Alt+Escape" => &["complete:"],
        "Alt+ArrowUp" => &["moveBackward:", "moveToBeginningOfParagraph:"],
        "Alt+ArrowDown" => &["moveForward:", "moveToEndOfParagraph:"],
        "Alt+ArrowLeft" => &["moveWordLeft:"],
        "Alt+ArrowRight" => &["moveWordRight:"],
        "Alt+Delete" => &["deleteWordForward:"],
        "Alt+PageUp" => &["pageUp:"],
        "Alt+PageDown" => &["pageDown:"],
        "Shift+Alt+Backspace" => &["deleteWordBackward:"],
        "Shift+Alt+Enter" => &["insertNewlineIgnoringFieldEditor:"],
        "Shift+Alt+NumpadEnter" => &["insertNewlineIgnoringFieldEditor:"],
        "Shift+Alt+Escape" => &["complete:"],
        "Shift+Alt+ArrowUp" => &["moveParagraphBackwardAndModifySelection:"],
        "Shift+Alt+ArrowDown" => &["moveParagraphForwardAndModifySelection:"],
        "Shift+Alt+ArrowLeft" => &["moveWordLeftAndModifySelection:"],
        "Shift+Alt+ArrowRight" => &["moveWordRightAndModifySelection:"],
        "Shift+Alt+Delete" => &["deleteWordForward:"],
        "Shift+Alt+PageUp" => &["pageUp:"],
        "Shift+Alt+PageDown" => &["pageDown:"],
        "Control+Alt+KeyB" => &["moveWordBackward:"],
        "Control+Alt+KeyF" => &["moveWordForward:"],
        "Control+Alt+Backspace" => &["deleteWordBackward:"],
        "Shift+Control+Alt+KeyB" => &["moveWordBackwardAndModifySelection:"],
        "Shift+Control+Alt+KeyF" => &["moveWordForwardAndModifySelection:"],
        "Shift+Control+Alt+Backspace" => &["deleteWordBackward:"],
        "Meta+NumpadSubtract" => &["cancel:"],
        "Meta+Backspace" => &["deleteToBeginningOfLine:"],
        "Meta+ArrowUp" => &["moveToBeginningOfDocument:"],
        "Meta+ArrowDown" => &["moveToEndOfDocument:"],
        "Meta+ArrowLeft" => &["moveToLeftEndOfLine:"],
        "Meta+ArrowRight" => &["moveToRightEndOfLine:"],
        "Shift+Meta+NumpadSubtract" => &["cancel:"],
        "Shift+Meta+Backspace" => &["deleteToBeginningOfLine:"],
        "Shift+Meta+ArrowUp" => &["moveToBeginningOfDocumentAndModifySelection:"],
        "Shift+Meta+ArrowDown" => &["moveToEndOfDocumentAndModifySelection:"],
        "Shift+Meta+ArrowLeft" => &["moveToLeftEndOfLineAndModifySelection:"],
        "Shift+Meta+ArrowRight" => &["moveToRightEndOfLineAndModifySelection:"],
        "Meta+KeyA" => &["selectAll:"],
        "Meta+KeyC" => &["copy:"],
        "Meta+KeyX" => &["cut:"],
        "Meta+KeyV" => &["paste:"],
        "Meta+KeyZ" => &["undo:"],
        "Shift+Meta+KeyZ" => &["redo:"],
        _ => &[],
    }
}

// ----------------------------------------------------------------------
// Input state
// ----------------------------------------------------------------------

#[derive(Default)]
struct KeyboardState {
    /// Insertion-ordered set, like the JS `Set` of `input.ts`.
    pressed_modifiers: Vec<KeyboardModifier>,
    pressed_keys: HashSet<String>,
}

#[derive(Default)]
struct MouseState {
    x: f64,
    y: f64,
    last_button: Option<MouseButton>,
    buttons: Vec<MouseButton>,
}

#[derive(Default)]
struct DragState {
    data: Option<DragData>,
    last_position: (f64, f64),
}

/// Input state of a page (pressed keys/modifiers, mouse position/buttons,
/// in-progress drag).
pub(crate) struct InputState {
    session: chromiumoxide::Page,
    keyboard: Mutex<KeyboardState>,
    mouse: Mutex<MouseState>,
    drag: Mutex<DragState>,
    page: OnceLock<Weak<PageInner>>,
}

impl InputState {
    pub(crate) fn new(session: chromiumoxide::Page) -> Arc<Self> {
        Arc::new(InputState {
            session,
            keyboard: Mutex::new(KeyboardState::default()),
            mouse: Mutex::new(MouseState::default()),
            drag: Mutex::new(DragState::default()),
            page: OnceLock::new(),
        })
    }

    /// Bind the owning page; needed by the drag manager to install its
    /// helpers in the page's frames. Later calls are ignored.
    pub(crate) fn bind_page(&self, page: Weak<PageInner>) {
        let _ = self.page.set(page);
    }

    async fn send<C: chromiumoxide::types::Command>(&self, cmd: C) -> Result<C::Response> {
        Ok(self.session.execute(cmd).await?.result)
    }

    fn modifiers(&self) -> Vec<KeyboardModifier> {
        self.keyboard.lock().unwrap().pressed_modifiers.clone()
    }

    // ---- keyboard ----

    /// `Keyboard.down`.
    pub(crate) async fn down(&self, key: &str) -> Result<()> {
        let (description, auto_repeat, modifiers) = {
            let mut state = self.keyboard.lock().unwrap();
            let description = key_description_for_string(key, &state.pressed_modifiers)?;
            let auto_repeat = !state.pressed_keys.insert(description.code.clone());
            if let Some(modifier) = KeyboardModifier::from_key(&description.key) {
                if !state.pressed_modifiers.contains(&modifier) {
                    state.pressed_modifiers.push(modifier);
                }
            }
            (description, auto_repeat, state.pressed_modifiers.clone())
        };
        self.raw_keydown(&modifiers, &description, auto_repeat)
            .await
    }

    /// `Keyboard.up`.
    pub(crate) async fn up(&self, key: &str) -> Result<()> {
        let (description, modifiers) = {
            let mut state = self.keyboard.lock().unwrap();
            let description = key_description_for_string(key, &state.pressed_modifiers)?;
            if let Some(modifier) = KeyboardModifier::from_key(&description.key) {
                state.pressed_modifiers.retain(|m| *m != modifier);
            }
            state.pressed_keys.remove(&description.code);
            (description, state.pressed_modifiers.clone())
        };
        self.raw_keyup(&modifiers, &description).await
    }

    /// `Keyboard.insertText`.
    pub(crate) async fn insert_text(&self, text: &str) -> Result<()> {
        self.send(InsertTextParams::new(text)).await?;
        Ok(())
    }

    /// `Keyboard.type`.
    pub(crate) async fn type_text(&self, text: &str, delay: Option<f64>) -> Result<()> {
        let delay = delay.filter(|d| *d > 0.0);
        let mut buf = [0u8; 4];
        for c in text.chars() {
            let s: &str = c.encode_utf8(&mut buf);
            if us_keyboard_layout().contains_key(s) {
                self.press(s, delay).await?;
            } else {
                if let Some(delay) = delay {
                    sleep_ms(delay).await;
                }
                self.insert_text(s).await?;
            }
        }
        Ok(())
    }

    /// `Keyboard.press`.
    pub(crate) async fn press(&self, key: &str, delay: Option<f64>) -> Result<()> {
        let tokens = split_key_combination(key);
        let (last, modifiers) = tokens.split_last().expect("at least one token");
        for token in modifiers {
            self.down(token).await?;
        }
        self.down(last).await?;
        if let Some(delay) = delay.filter(|d| *d > 0.0) {
            sleep_ms(delay).await;
        }
        self.up(last).await?;
        for token in modifiers.iter().rev() {
            self.up(token).await?;
        }
        Ok(())
    }

    /// `Keyboard.ensureModifiers`: press/release modifiers to match `mods`
    /// and return the previously pressed ones, to be restored afterwards.
    pub(crate) async fn ensure_modifiers(
        &self,
        mods: &[KeyboardModifier],
    ) -> Result<Vec<KeyboardModifier>> {
        let modifiers: Vec<KeyboardModifier> = mods.iter().map(|m| m.resolve()).collect();
        let restore = self.modifiers();
        for key in MODIFIERS {
            let need_down = modifiers.contains(&key);
            let is_down = self.modifiers().contains(&key);
            if need_down && !is_down {
                self.down(key.as_str()).await?;
            } else if !need_down && is_down {
                self.up(key.as_str()).await?;
            }
        }
        Ok(restore)
    }

    async fn raw_keydown(
        &self,
        modifiers: &[KeyboardModifier],
        description: &KeyDescription,
        auto_repeat: bool,
    ) -> Result<()> {
        if description.code == "Escape" && self.cancel_drag().await? {
            return Ok(());
        }
        let commands = commands_for_code(&description.code, modifiers);
        let text = &description.text;
        let mut params = DispatchKeyEventParams::new(if text.is_empty() {
            DispatchKeyEventType::RawKeyDown
        } else {
            DispatchKeyEventType::KeyDown
        });
        params.modifiers = Some(modifiers_mask(modifiers));
        params.windows_virtual_key_code = Some(description.key_code_without_location);
        params.code = Some(description.code.clone());
        params.commands = Some(commands);
        params.key = Some(description.key.clone());
        params.text = Some(text.clone());
        params.unmodified_text = Some(text.clone());
        params.auto_repeat = Some(auto_repeat);
        params.location = Some(description.location);
        params.is_keypad = Some(description.location == KEYPAD_LOCATION);
        self.send(params).await?;
        Ok(())
    }

    async fn raw_keyup(
        &self,
        modifiers: &[KeyboardModifier],
        description: &KeyDescription,
    ) -> Result<()> {
        let mut params = DispatchKeyEventParams::new(DispatchKeyEventType::KeyUp);
        params.modifiers = Some(modifiers_mask(modifiers));
        params.key = Some(description.key.clone());
        params.windows_virtual_key_code = Some(description.key_code_without_location);
        params.code = Some(description.code.clone());
        params.location = Some(description.location);
        params.is_keypad = Some(description.location == KEYPAD_LOCATION);
        self.send(params).await?;
        Ok(())
    }

    // ---- mouse ----

    fn mouse_event(
        kind: DispatchMouseEventType,
        x: f64,
        y: f64,
        button: Option<MouseButton>,
        buttons: &[MouseButton],
        modifiers: &[KeyboardModifier],
    ) -> DispatchMouseEventParams {
        let mut params = DispatchMouseEventParams::new(kind, x, y);
        params.button = Some(button.map(|b| b.to_cdp()).unwrap_or(CdpMouseButton::None));
        params.buttons = Some(buttons_mask(buttons));
        params.modifiers = Some(modifiers_mask(modifiers));
        params
    }

    fn move_params(
        x: f64,
        y: f64,
        button: Option<MouseButton>,
        buttons: &[MouseButton],
        modifiers: &[KeyboardModifier],
    ) -> DispatchMouseEventParams {
        let mut params = Self::mouse_event(
            DispatchMouseEventType::MouseMoved,
            x,
            y,
            button,
            buttons,
            modifiers,
        );
        params.force = Some(if buttons.is_empty() { 0.0 } else { 0.5 });
        params
    }

    /// `Mouse.move`.
    pub(crate) async fn mouse_move(
        &self,
        x: f64,
        y: f64,
        steps: u32,
        for_click: bool,
    ) -> Result<()> {
        let (from_x, from_y) = {
            let mut state = self.mouse.lock().unwrap();
            let from = (state.x, state.y);
            state.x = x;
            state.y = y;
            from
        };
        for i in 1..=steps {
            let middle_x = from_x + (x - from_x) * (i as f64 / steps as f64);
            let middle_y = from_y + (y - from_y) * (i as f64 / steps as f64);
            let (button, buttons) = {
                let state = self.mouse.lock().unwrap();
                (state.last_button, state.buttons.clone())
            };
            let modifiers = self.modifiers();
            self.raw_move(middle_x, middle_y, button, &buttons, &modifiers, for_click)
                .await?;
        }
        Ok(())
    }

    async fn raw_move(
        &self,
        x: f64,
        y: f64,
        button: Option<MouseButton>,
        buttons: &[MouseButton],
        modifiers: &[KeyboardModifier],
        for_click: bool,
    ) -> Result<()> {
        let params = Self::move_params(x, y, button, buttons, modifiers);
        if for_click {
            // Avoid extra protocol calls related to drag and drop, because
            // click relies on move-down-up being sent back to back.
            self.send(params).await?;
            return Ok(());
        }
        self.intercept_drag_caused_by_move(x, y, button, modifiers, params)
            .await
    }

    fn down_params(&self, button: MouseButton, click_count: u32) -> DispatchMouseEventParams {
        let (x, y, buttons) = {
            let mut state = self.mouse.lock().unwrap();
            state.last_button = Some(button);
            if !state.buttons.contains(&button) {
                state.buttons.push(button);
            }
            (state.x, state.y, state.buttons.clone())
        };
        let mut params = Self::mouse_event(
            DispatchMouseEventType::MousePressed,
            x,
            y,
            Some(button),
            &buttons,
            &self.modifiers(),
        );
        params.click_count = Some(click_count as i64);
        params.force = Some(if buttons.is_empty() { 0.0 } else { 0.5 });
        params
    }

    fn up_params(&self, button: MouseButton, click_count: u32) -> DispatchMouseEventParams {
        let (x, y, buttons) = {
            let mut state = self.mouse.lock().unwrap();
            state.last_button = None;
            state.buttons.retain(|b| *b != button);
            (state.x, state.y, state.buttons.clone())
        };
        let mut params = Self::mouse_event(
            DispatchMouseEventType::MouseReleased,
            x,
            y,
            Some(button),
            &buttons,
            &self.modifiers(),
        );
        params.click_count = Some(click_count as i64);
        params
    }

    /// `Mouse.down`.
    pub(crate) async fn mouse_down(&self, button: MouseButton, click_count: u32) -> Result<()> {
        let params = self.down_params(button, click_count);
        if self.is_dragging() {
            return Ok(());
        }
        self.send(params).await?;
        Ok(())
    }

    /// `Mouse.up`.
    pub(crate) async fn mouse_up(&self, button: MouseButton, click_count: u32) -> Result<()> {
        let params = self.up_params(button, click_count);
        if self.is_dragging() {
            let modifiers = self.modifiers();
            return self.drop_drag(params.x, params.y, &modifiers).await;
        }
        self.send(params).await?;
        Ok(())
    }

    /// `Mouse.click`.
    pub(crate) async fn mouse_click(
        &self,
        x: f64,
        y: f64,
        button: MouseButton,
        click_count: u32,
        delay: Option<f64>,
    ) -> Result<()> {
        self.mouse_click_with_steps(x, y, button, click_count, delay, None)
            .await
    }

    /// `Mouse.click` with the `steps` option.
    pub(crate) async fn mouse_click_with_steps(
        &self,
        x: f64,
        y: f64,
        button: MouseButton,
        click_count: u32,
        delay: Option<f64>,
        steps: Option<u32>,
    ) -> Result<()> {
        if let Some(delay) = delay.filter(|d| *d > 0.0) {
            self.mouse_move(x, y, steps.unwrap_or(1), true).await?;
            for cc in 1..=click_count {
                self.mouse_down(button, cc).await?;
                sleep_ms(delay).await;
                self.mouse_up(button, cc).await?;
                if cc < click_count {
                    sleep_ms(delay).await;
                }
            }
            return Ok(());
        }
        let steps = steps.unwrap_or(1);
        if steps > 1 || self.is_dragging() {
            self.mouse_move(x, y, steps, true).await?;
            if self.is_dragging() {
                for cc in 1..=click_count {
                    self.mouse_down(button, cc).await?;
                    self.mouse_up(button, cc).await?;
                }
                return Ok(());
            }
        }
        // Like Playwright's `Promise.all`: compute every command with the
        // state updates in order, then send them back to back.
        let mut commands = Vec::new();
        if steps <= 1 {
            let (button_before, buttons) = {
                let mut state = self.mouse.lock().unwrap();
                state.x = x;
                state.y = y;
                (state.last_button, state.buttons.clone())
            };
            if steps == 1 {
                commands.push(Self::move_params(
                    x,
                    y,
                    button_before,
                    &buttons,
                    &self.modifiers(),
                ));
            }
        }
        for cc in 1..=click_count {
            commands.push(self.down_params(button, cc));
            commands.push(self.up_params(button, cc));
        }
        let futures = commands
            .into_iter()
            .map(|params| self.session.command_future(params))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        futures::future::try_join_all(futures).await?;
        Ok(())
    }

    /// `Mouse.wheel`.
    pub(crate) async fn mouse_wheel(&self, dx: f64, dy: f64) -> Result<()> {
        let (x, y) = {
            let state = self.mouse.lock().unwrap();
            (state.x, state.y)
        };
        let mut params = DispatchMouseEventParams::new(DispatchMouseEventType::MouseWheel, x, y);
        params.modifiers = Some(modifiers_mask(&self.modifiers()));
        params.delta_x = Some(dx);
        params.delta_y = Some(dy);
        self.send(params).await?;
        Ok(())
    }

    // ---- touchscreen ----

    /// `Touchscreen.tap`.
    pub(crate) async fn tap(&self, x: f64, y: f64) -> Result<()> {
        let mask = modifiers_mask(&self.modifiers());
        let mut start = DispatchTouchEventParams::new(
            DispatchTouchEventType::TouchStart,
            vec![TouchPoint::new(x, y)],
        );
        start.modifiers = Some(mask);
        let mut end = DispatchTouchEventParams::new(DispatchTouchEventType::TouchEnd, vec![]);
        end.modifiers = Some(mask);
        let start = self.session.command_future(start)?;
        let end = self.session.command_future(end)?;
        futures::future::try_join(start, end).await?;
        Ok(())
    }

    // ---- drag and drop (crDragDrop.ts) ----

    fn is_dragging(&self) -> bool {
        self.drag.lock().unwrap().data.is_some()
    }

    async fn cancel_drag(&self) -> Result<bool> {
        let (data, (x, y)) = {
            let state = self.drag.lock().unwrap();
            (state.data.clone(), state.last_position)
        };
        if data.is_none() {
            return Ok(false);
        }
        self.send(DispatchDragEventParams::new(
            DispatchDragEventType::DragCancel,
            x,
            y,
            DragData::new(vec![], 0xFFFF),
        ))
        .await?;
        self.drag.lock().unwrap().data = None;
        Ok(true)
    }

    async fn drop_drag(&self, x: f64, y: f64, modifiers: &[KeyboardModifier]) -> Result<()> {
        let data = self.drag.lock().unwrap().data.clone();
        let Some(data) = data else {
            return Ok(());
        };
        let mut params = DispatchDragEventParams::new(DispatchDragEventType::Drop, x, y, data);
        params.modifiers = Some(modifiers_mask(modifiers));
        self.send(params).await?;
        self.drag.lock().unwrap().data = None;
        Ok(())
    }

    async fn evaluate_in_all_frames(page: &Arc<PageInner>, expression: &str) -> Vec<bool> {
        let frames = page.frames_dfs();
        let futures = frames.into_iter().map(|frame| {
            let expression = expression.to_string();
            async move {
                let eval = frame.evaluate_in(World::Utility, &expression, None, true);
                match tokio::time::timeout(Duration::from_secs(2), eval).await {
                    Ok(Ok(Evaluated::Value(JsValue::Bool(b)))) => b,
                    _ => false,
                }
            }
        });
        futures::future::join_all(futures).await
    }

    async fn intercept_drag_caused_by_move(
        &self,
        x: f64,
        y: f64,
        button: Option<MouseButton>,
        modifiers: &[KeyboardModifier],
        move_params: DispatchMouseEventParams,
    ) -> Result<()> {
        let dragging = {
            let mut state = self.drag.lock().unwrap();
            state.last_position = (x, y);
            state.data.clone()
        };
        if let Some(data) = dragging {
            let mut params =
                DispatchDragEventParams::new(DispatchDragEventType::DragOver, x, y, data);
            params.modifiers = Some(modifiers_mask(modifiers));
            self.send(params).await?;
            return Ok(());
        }
        let page = self.page.get().and_then(Weak::upgrade);
        let page = match (button, page) {
            (Some(MouseButton::Left), Some(page)) => page,
            _ => {
                self.send(move_params).await?;
                return Ok(());
            }
        };

        Self::evaluate_in_all_frames(&page, SETUP_DRAG_LISTENERS).await;
        let mut intercepted = self
            .session
            .event_listener::<EventDragIntercepted>()
            .await?;
        let result = async {
            self.send(SetInterceptDragsParams::new(true)).await?;
            let moved = self.send(move_params).await;
            let expecting = match moved {
                Ok(_) => Self::evaluate_in_all_frames(&page, CLEANUP_DRAG)
                    .await
                    .into_iter()
                    .any(|b| b),
                Err(_) => false,
            };
            let disabled = self.send(SetInterceptDragsParams::new(false)).await;
            moved?;
            disabled?;
            Ok::<_, Error>(expecting)
        }
        .await;
        let expecting = match result {
            Ok(expecting) => expecting,
            Err(err) => {
                // Cleanup without blocking.
                let page = page.clone();
                tokio::spawn(async move {
                    InputState::evaluate_in_all_frames(&page, CLEANUP_DRAG).await;
                });
                return Err(err);
            }
        };
        let data = if expecting {
            match tokio::time::timeout(Duration::from_secs(5), intercepted.next()).await {
                Ok(Some(event)) => Some(event.data.clone()),
                _ => None,
            }
        } else {
            None
        };
        self.drag.lock().unwrap().data = data.clone();
        if let Some(data) = data {
            let mut params =
                DispatchDragEventParams::new(DispatchDragEventType::DragEnter, x, y, data);
            params.modifiers = Some(modifiers_mask(modifiers));
            self.send(params).await?;
        }
        Ok(())
    }
}

/// `setupDragListeners` of `crDragDrop.ts`, installed in every frame's
/// utility world before a move with the left button pressed.
const SETUP_DRAG_LISTENERS: &str = r#"(() => {
  let didStartDrag = Promise.resolve(false);
  let dragEvent = null;
  const dragListener = (event) => dragEvent = event;
  const mouseListener = () => {
    didStartDrag = new Promise(callback => {
      window.addEventListener('dragstart', dragListener, { once: true, capture: true });
      setTimeout(() => callback(dragEvent ? !dragEvent.defaultPrevented : false), 0);
    });
  };
  window.addEventListener('mousemove', mouseListener, { once: true, capture: true });
  window.__cleanupDrag = async () => {
    const val = await didStartDrag;
    window.removeEventListener('mousemove', mouseListener, { capture: true });
    window.removeEventListener('dragstart', dragListener, { capture: true });
    delete window.__cleanupDrag;
    return val;
  };
})()"#;

const CLEANUP_DRAG: &str = "window.__cleanupDrag ? window.__cleanupDrag() : false";

async fn sleep_ms(ms: f64) {
    if ms > 0.0 {
        tokio::time::sleep(Duration::from_secs_f64(ms / 1000.0)).await;
    }
}

// ----------------------------------------------------------------------
// Public API
// ----------------------------------------------------------------------

impl Page {
    fn input(&self) -> Arc<InputState> {
        self.inner.input.bind_page(Arc::downgrade(&self.inner));
        self.inner.input.clone()
    }

    /// The page's keyboard.
    pub fn keyboard(&self) -> Keyboard {
        Keyboard {
            input: self.input(),
        }
    }

    /// The page's mouse.
    pub fn mouse(&self) -> Mouse {
        Mouse {
            input: self.input(),
        }
    }

    /// The page's touchscreen.
    pub fn touchscreen(&self) -> Touchscreen {
        Touchscreen {
            input: self.input(),
        }
    }
}

/// Keyboard of a page (`page.keyboard()`). Cheap to clone.
///
/// Key names follow Playwright: `"a"`, `"A"`, `"Enter"`, `"ArrowLeft"`,
/// `"Shift+KeyA"`, `"ControlOrMeta+c"`, ...
#[derive(Clone)]
pub struct Keyboard {
    input: Arc<InputState>,
}

impl fmt::Debug for Keyboard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Keyboard").finish_non_exhaustive()
    }
}

impl Keyboard {
    /// Dispatch a `keydown` event (and `keypress`/`input` for printable keys).
    pub async fn down(&self, key: &str) -> Result<()> {
        self.input.down(key).await.api("keyboard.down")
    }

    /// Dispatch a `keyup` event.
    pub async fn up(&self, key: &str) -> Result<()> {
        self.input.up(key).await.api("keyboard.up")
    }

    /// Insert text without key events (only `input` is dispatched).
    pub async fn insert_text(&self, text: &str) -> Result<()> {
        self.input
            .insert_text(text)
            .await
            .api("keyboard.insertText")
    }

    /// Press a key or a combination (`down` then `up`).
    pub fn press(&self, key: impl Into<String>) -> KeyboardPress {
        KeyboardPress {
            input: self.input.clone(),
            key: key.into(),
            delay: None,
        }
    }

    /// Type text character by character, sending key events for each.
    pub fn r#type(&self, text: impl Into<String>) -> KeyboardType {
        KeyboardType {
            input: self.input.clone(),
            text: text.into(),
            delay: None,
        }
    }

    /// Alias of [`Keyboard::r#type`].
    pub fn type_text(&self, text: impl Into<String>) -> KeyboardType {
        self.r#type(text)
    }
}

/// Builder returned by [`Keyboard::press`]; `.await` it.
#[must_use = "builders do nothing unless awaited"]
pub struct KeyboardPress {
    input: Arc<InputState>,
    key: String,
    delay: Option<f64>,
}

impl KeyboardPress {
    /// Time to wait between `keydown` and `keyup`, in milliseconds.
    pub fn delay(mut self, delay_ms: u64) -> Self {
        self.delay = Some(delay_ms as f64);
        self
    }
}

impl IntoFuture for KeyboardPress {
    type Output = Result<()>;
    type IntoFuture = BoxFuture<Result<()>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            self.input
                .press(&self.key, self.delay)
                .await
                .api("keyboard.press")
        })
    }
}

/// Builder returned by [`Keyboard::r#type`]; `.await` it.
#[must_use = "builders do nothing unless awaited"]
pub struct KeyboardType {
    input: Arc<InputState>,
    text: String,
    delay: Option<f64>,
}

impl KeyboardType {
    /// Time to wait between key presses, in milliseconds.
    pub fn delay(mut self, delay_ms: u64) -> Self {
        self.delay = Some(delay_ms as f64);
        self
    }
}

impl IntoFuture for KeyboardType {
    type Output = Result<()>;
    type IntoFuture = BoxFuture<Result<()>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            self.input
                .type_text(&self.text, self.delay)
                .await
                .api("keyboard.type")
        })
    }
}

/// Mouse of a page (`page.mouse()`), in CSS pixels relative to the main
/// frame's viewport. Cheap to clone.
#[derive(Clone)]
pub struct Mouse {
    input: Arc<InputState>,
}

impl fmt::Debug for Mouse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Mouse").finish_non_exhaustive()
    }
}

impl Mouse {
    /// Move the mouse to `(x, y)` (dispatches `mousemove`).
    pub fn r#move(&self, x: impl Into<f64>, y: impl Into<f64>) -> MouseMove {
        MouseMove {
            input: self.input.clone(),
            x: x.into(),
            y: y.into(),
            steps: 1,
        }
    }

    /// Alias of [`Mouse::r#move`].
    pub fn move_to(&self, x: impl Into<f64>, y: impl Into<f64>) -> MouseMove {
        self.r#move(x, y)
    }

    /// Press a button at the current position (dispatches `mousedown`).
    pub fn down(&self) -> MouseButtonAction {
        MouseButtonAction {
            input: self.input.clone(),
            button: MouseButton::Left,
            click_count: 1,
            up: false,
        }
    }

    /// Release a button at the current position (dispatches `mouseup`).
    pub fn up(&self) -> MouseButtonAction {
        MouseButtonAction {
            input: self.input.clone(),
            button: MouseButton::Left,
            click_count: 1,
            up: true,
        }
    }

    /// Move to `(x, y)` and click (`mousemove`, `mousedown`, `mouseup`).
    pub fn click(&self, x: impl Into<f64>, y: impl Into<f64>) -> MouseClick {
        MouseClick {
            input: self.input.clone(),
            x: x.into(),
            y: y.into(),
            button: MouseButton::Left,
            click_count: 1,
            delay: None,
            steps: None,
            api: "mouse.click",
        }
    }

    /// Move to `(x, y)` and double click.
    pub fn dblclick(&self, x: impl Into<f64>, y: impl Into<f64>) -> MouseClick {
        MouseClick {
            click_count: 2,
            api: "mouse.dblclick",
            ..self.click(x, y)
        }
    }

    /// Dispatch a `wheel` event at the current position.
    pub async fn wheel(&self, delta_x: impl Into<f64>, delta_y: impl Into<f64>) -> Result<()> {
        self.input
            .mouse_wheel(delta_x.into(), delta_y.into())
            .await
            .api("mouse.wheel")
    }
}

/// Builder returned by [`Mouse::r#move`]; `.await` it.
#[must_use = "builders do nothing unless awaited"]
pub struct MouseMove {
    input: Arc<InputState>,
    x: f64,
    y: f64,
    steps: u32,
}

impl MouseMove {
    /// Number of intermediate `mousemove` events (default 1).
    pub fn steps(mut self, steps: u32) -> Self {
        self.steps = steps;
        self
    }
}

impl IntoFuture for MouseMove {
    type Output = Result<()>;
    type IntoFuture = BoxFuture<Result<()>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            self.input
                .mouse_move(self.x, self.y, self.steps, false)
                .await
                .api("mouse.move")
        })
    }
}

/// Builder returned by [`Mouse::down`] / [`Mouse::up`]; `.await` it.
#[must_use = "builders do nothing unless awaited"]
pub struct MouseButtonAction {
    input: Arc<InputState>,
    button: MouseButton,
    click_count: u32,
    up: bool,
}

impl MouseButtonAction {
    /// The button (default [`MouseButton::Left`]).
    pub fn button(mut self, button: MouseButton) -> Self {
        self.button = button;
        self
    }

    /// The `detail` of the event (default 1).
    pub fn click_count(mut self, click_count: u32) -> Self {
        self.click_count = click_count;
        self
    }
}

impl IntoFuture for MouseButtonAction {
    type Output = Result<()>;
    type IntoFuture = BoxFuture<Result<()>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            if self.up {
                self.input
                    .mouse_up(self.button, self.click_count)
                    .await
                    .api("mouse.up")
            } else {
                self.input
                    .mouse_down(self.button, self.click_count)
                    .await
                    .api("mouse.down")
            }
        })
    }
}

/// Builder returned by [`Mouse::click`] / [`Mouse::dblclick`]; `.await` it.
#[must_use = "builders do nothing unless awaited"]
pub struct MouseClick {
    input: Arc<InputState>,
    x: f64,
    y: f64,
    button: MouseButton,
    click_count: u32,
    delay: Option<f64>,
    steps: Option<u32>,
    api: &'static str,
}

impl MouseClick {
    /// The button (default [`MouseButton::Left`]).
    pub fn button(mut self, button: MouseButton) -> Self {
        self.button = button;
        self
    }

    /// Number of clicks (default 1, 2 for `dblclick`).
    pub fn click_count(mut self, click_count: u32) -> Self {
        self.click_count = click_count;
        self
    }

    /// Time to wait between `mousedown` and `mouseup`, in milliseconds.
    pub fn delay(mut self, delay_ms: u64) -> Self {
        self.delay = Some(delay_ms as f64);
        self
    }

    /// Number of intermediate `mousemove` events before clicking.
    pub fn steps(mut self, steps: u32) -> Self {
        self.steps = Some(steps);
        self
    }
}

impl IntoFuture for MouseClick {
    type Output = Result<()>;
    type IntoFuture = BoxFuture<Result<()>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            self.input
                .mouse_click_with_steps(
                    self.x,
                    self.y,
                    self.button,
                    self.click_count,
                    self.delay,
                    self.steps,
                )
                .await
                .api(self.api)
        })
    }
}

/// Touchscreen of a page (`page.touchscreen()`). Cheap to clone.
#[derive(Clone)]
pub struct Touchscreen {
    input: Arc<InputState>,
}

impl fmt::Debug for Touchscreen {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Touchscreen").finish_non_exhaustive()
    }
}

impl Touchscreen {
    /// Dispatch `touchstart` and `touchend` at `(x, y)`.
    pub async fn tap(&self, x: impl Into<f64>, y: impl Into<f64>) -> Result<()> {
        self.input
            .tap(x.into(), y.into())
            .await
            .api("touchscreen.tap")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn desc(key: &str) -> KeyDescription {
        key_description_for_string(key, &[]).unwrap()
    }

    #[test]
    fn key_lookup_by_code_key_and_alias() {
        let a = desc("KeyA");
        assert_eq!(
            (a.key.as_str(), a.code.as_str(), a.text.as_str()),
            ("a", "KeyA", "a")
        );
        assert_eq!(a.key_code, 65);
        let upper = desc("A");
        assert_eq!((upper.key.as_str(), upper.code.as_str()), ("A", "KeyA"));
        assert!(upper.shifted.is_none());
        let shift = desc("Shift");
        assert_eq!((shift.code.as_str(), shift.location), ("ShiftLeft", 1));
        assert_eq!(shift.key_code_without_location, 16);
        assert_eq!(shift.key_code, 160);
        let enter = desc("\n");
        assert_eq!((enter.code.as_str(), enter.text.as_str()), ("Enter", "\r"));
        assert_eq!(desc("\r").code, "Enter");
        // Numpad keys are only reachable by code.
        assert_eq!(desc("-").code, "Minus");
        let numpad = desc("NumpadSubtract");
        assert_eq!((numpad.key.as_str(), numpad.location), ("-", 3));
        assert_eq!(desc("+").code, "Equal");
        assert_eq!(desc(" ").code, "Space");
    }

    #[test]
    fn shifted_keys() {
        let shifted = key_description_for_string("Digit3", &[KeyboardModifier::Shift]).unwrap();
        assert_eq!((shifted.key.as_str(), shifted.text.as_str()), ("#", "#"));
        let numpad = key_description_for_string("Numpad1", &[KeyboardModifier::Shift]).unwrap();
        assert_eq!((numpad.key.as_str(), numpad.key_code), ("1", 97));
        let decimal = desc("NumpadDecimal");
        assert_eq!(
            (decimal.key.as_str(), decimal.text.as_str()),
            ("Delete", "")
        );
        // Shift alone keeps the text, other modifiers drop it.
        let text = key_description_for_string("a", &[KeyboardModifier::Alt]).unwrap();
        assert_eq!(text.text, "");
        let text =
            key_description_for_string("a", &[KeyboardModifier::Shift, KeyboardModifier::Control])
                .unwrap();
        assert_eq!(text.text, "");
    }

    #[test]
    fn unknown_key() {
        let err = key_description_for_string("NotARealKey", &[]).unwrap_err();
        assert_eq!(err.message(), "Unknown key: \"NotARealKey\"");
        assert!(key_description_for_string("ё", &[]).is_err());
    }

    #[test]
    fn masks() {
        assert_eq!(modifiers_mask(&[]), 0);
        assert_eq!(
            modifiers_mask(&[KeyboardModifier::Alt, KeyboardModifier::Shift]),
            9
        );
        assert_eq!(
            modifiers_mask(&[KeyboardModifier::Control, KeyboardModifier::Meta]),
            6
        );
        assert_eq!(buttons_mask(&[MouseButton::Left, MouseButton::Middle]), 5);
        assert_eq!(buttons_mask(&[MouseButton::Right]), 2);
    }

    #[test]
    fn control_or_meta() {
        let resolved = KeyboardModifier::ControlOrMeta.resolve();
        if IS_MAC {
            assert_eq!(resolved, KeyboardModifier::Meta);
            assert_eq!(desc("ControlOrMeta").key, "Meta");
        } else {
            assert_eq!(resolved, KeyboardModifier::Control);
            assert_eq!(desc("ControlOrMeta").key, "Control");
        }
        assert_eq!(
            "ControlOrMeta".parse::<KeyboardModifier>().unwrap(),
            KeyboardModifier::ControlOrMeta
        );
        assert!("Hyper".parse::<KeyboardModifier>().is_err());
        assert_eq!(
            "middle".parse::<MouseButton>().unwrap(),
            MouseButton::Middle
        );
    }

    #[test]
    fn split_combinations() {
        assert_eq!(split_key_combination("Shift+a"), vec!["Shift", "a"]);
        assert_eq!(split_key_combination("+"), vec!["+"]);
        assert_eq!(split_key_combination("Shift++"), vec!["Shift", "+"]);
        assert_eq!(
            split_key_combination("Control+Shift+~"),
            vec!["Control", "Shift", "~"]
        );
    }

    #[test]
    fn mac_commands_filter_insert() {
        let commands = mac_editing_commands("Control+KeyO");
        assert_eq!(commands.len(), 2);
        let filtered = commands_for_code("KeyO", &[KeyboardModifier::Control]);
        if IS_MAC {
            assert_eq!(filtered, vec!["moveBackward".to_string()]);
        } else {
            assert!(filtered.is_empty());
        }
    }
}
