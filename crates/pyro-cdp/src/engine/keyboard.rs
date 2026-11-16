//! Key names as Playwright's keyboard takes them (its US layout,
//! `usKeyboardLayout.ts`): `KeyboardEvent.code` names (`KeyD`, `Digit1`,
//! `Minus`, `Numpad1`, `ShiftRight`...), the keys they type (`d`, `D`, `!`,
//! `/`...) and the key names `Shift`, `Control`, `Alt`, `Meta`, `Enter`
//! (also `\n` and `\r`), `Escape`, `ArrowDown`, `F5`... A chord
//! (`Control+Shift+KeyZ`, `Shift++`) holds each key before the last while
//! the last is pressed. Shift picks a code's shifted key (`Shift+KeyA` is
//! `A`, `Shift+Digit1` is `!`); a key named by what it types stays that key
//! (`Shift+a` is `a`). With Control, Alt or Meta held a key types nothing.
//! A name the layout does not have is an error (`Unknown key: "Clear"`), as
//! are characters outside it (`é`): `type` inserts those as text instead.

use pyro_webapi::input::{KeyInput, KeyKind, Modifiers};

/// One key of the layout: its code, the `key` it gives (and shifted), the
/// key code (Chromium's `windowsVirtualKeyCode`, the DOM `keyCode`), the text
/// it types when that is not its key, and its location (1 left, 2 right, 3
/// numpad).
struct Def {
    code: &'static str,
    key: &'static str,
    key_code: u32,
    shift_key: Option<&'static str>,
    text: Option<&'static str>,
    location: u32,
}

const fn k(code: &'static str, key_code: u32, key: &'static str) -> Def {
    Def { code, key, key_code, shift_key: None, text: None, location: 0 }
}

const fn s(code: &'static str, key_code: u32, key: &'static str, shifted: &'static str) -> Def {
    Def { code, key, key_code, shift_key: Some(shifted), text: None, location: 0 }
}

const fn at(code: &'static str, key_code: u32, key: &'static str, location: u32) -> Def {
    Def { code, key, key_code, shift_key: None, text: None, location }
}

const fn pad(code: &'static str, key_code: u32, key: &'static str, shifted: &'static str) -> Def {
    Def { code, key, key_code, shift_key: Some(shifted), text: None, location: 3 }
}

/// Playwright's `USKeyboardLayout`, in its order (a later key wins a name).
/// Key codes are the ones Chromium reports (`keyCodeWithoutLocation`: 16 for
/// both Shift keys).
const LAYOUT: &[Def] = &[
    // Functions row
    k("Escape", 27, "Escape"),
    k("F1", 112, "F1"),
    k("F2", 113, "F2"),
    k("F3", 114, "F3"),
    k("F4", 115, "F4"),
    k("F5", 116, "F5"),
    k("F6", 117, "F6"),
    k("F7", 118, "F7"),
    k("F8", 119, "F8"),
    k("F9", 120, "F9"),
    k("F10", 121, "F10"),
    k("F11", 122, "F11"),
    k("F12", 123, "F12"),
    // Numbers row
    s("Backquote", 192, "`", "~"),
    s("Digit1", 49, "1", "!"),
    s("Digit2", 50, "2", "@"),
    s("Digit3", 51, "3", "#"),
    s("Digit4", 52, "4", "$"),
    s("Digit5", 53, "5", "%"),
    s("Digit6", 54, "6", "^"),
    s("Digit7", 55, "7", "&"),
    s("Digit8", 56, "8", "*"),
    s("Digit9", 57, "9", "("),
    s("Digit0", 48, "0", ")"),
    s("Minus", 189, "-", "_"),
    s("Equal", 187, "=", "+"),
    s("Backslash", 220, "\\", "|"),
    k("Backspace", 8, "Backspace"),
    // First row
    k("Tab", 9, "Tab"),
    s("KeyQ", 81, "q", "Q"),
    s("KeyW", 87, "w", "W"),
    s("KeyE", 69, "e", "E"),
    s("KeyR", 82, "r", "R"),
    s("KeyT", 84, "t", "T"),
    s("KeyY", 89, "y", "Y"),
    s("KeyU", 85, "u", "U"),
    s("KeyI", 73, "i", "I"),
    s("KeyO", 79, "o", "O"),
    s("KeyP", 80, "p", "P"),
    s("BracketLeft", 219, "[", "{"),
    s("BracketRight", 221, "]", "}"),
    // Second row
    k("CapsLock", 20, "CapsLock"),
    s("KeyA", 65, "a", "A"),
    s("KeyS", 83, "s", "S"),
    s("KeyD", 68, "d", "D"),
    s("KeyF", 70, "f", "F"),
    s("KeyG", 71, "g", "G"),
    s("KeyH", 72, "h", "H"),
    s("KeyJ", 74, "j", "J"),
    s("KeyK", 75, "k", "K"),
    s("KeyL", 76, "l", "L"),
    s("Semicolon", 186, ";", ":"),
    s("Quote", 222, "'", "\""),
    Def { code: "Enter", key: "Enter", key_code: 13, shift_key: None, text: Some("\r"), location: 0 },
    // Third row
    at("ShiftLeft", 16, "Shift", 1),
    s("KeyZ", 90, "z", "Z"),
    s("KeyX", 88, "x", "X"),
    s("KeyC", 67, "c", "C"),
    s("KeyV", 86, "v", "V"),
    s("KeyB", 66, "b", "B"),
    s("KeyN", 78, "n", "N"),
    s("KeyM", 77, "m", "M"),
    s("Comma", 188, ",", "<"),
    s("Period", 190, ".", ">"),
    s("Slash", 191, "/", "?"),
    at("ShiftRight", 16, "Shift", 2),
    // Last row
    at("ControlLeft", 17, "Control", 1),
    at("MetaLeft", 91, "Meta", 1),
    at("AltLeft", 18, "Alt", 1),
    k("Space", 32, " "),
    at("AltRight", 18, "Alt", 2),
    k("AltGraph", 225, "AltGraph"),
    at("MetaRight", 92, "Meta", 2),
    k("ContextMenu", 93, "ContextMenu"),
    at("ControlRight", 17, "Control", 2),
    // Center block
    k("PrintScreen", 44, "PrintScreen"),
    k("ScrollLock", 145, "ScrollLock"),
    k("Pause", 19, "Pause"),
    k("PageUp", 33, "PageUp"),
    k("PageDown", 34, "PageDown"),
    k("Insert", 45, "Insert"),
    k("Delete", 46, "Delete"),
    k("Home", 36, "Home"),
    k("End", 35, "End"),
    k("ArrowLeft", 37, "ArrowLeft"),
    k("ArrowUp", 38, "ArrowUp"),
    k("ArrowRight", 39, "ArrowRight"),
    k("ArrowDown", 40, "ArrowDown"),
    // Numpad
    k("NumLock", 144, "NumLock"),
    at("NumpadDivide", 111, "/", 3),
    at("NumpadMultiply", 106, "*", 3),
    at("NumpadSubtract", 109, "-", 3),
    pad("Numpad7", 36, "Home", "7"),
    pad("Numpad8", 38, "ArrowUp", "8"),
    pad("Numpad9", 33, "PageUp", "9"),
    pad("Numpad4", 37, "ArrowLeft", "4"),
    pad("Numpad5", 12, "Clear", "5"),
    pad("Numpad6", 39, "ArrowRight", "6"),
    at("NumpadAdd", 107, "+", 3),
    pad("Numpad1", 35, "End", "1"),
    pad("Numpad2", 40, "ArrowDown", "2"),
    pad("Numpad3", 34, "PageDown", "3"),
    pad("Numpad0", 45, "Insert", "0"),
    pad("NumpadDecimal", 46, "\u{0}", "."),
    Def { code: "NumpadEnter", key: "Enter", key_code: 13, shift_key: None, text: Some("\r"), location: 3 },
];

/// What a key name stands for: a key of the layout, named by its code
/// (`KeyD`: Shift picks its shifted key) or by what it types (`d`, or `D`,
/// its shifted key: Shift changes neither).
#[derive(Clone, Copy)]
struct Named {
    def: &'static Def,
    /// Named by its code: Shift picks the shifted key.
    shiftable: bool,
    /// Named by its shifted key.
    shifted: bool,
}

/// The layout key `name` names (Playwright's `buildLayoutClosure`): a code,
/// an alias (`Shift`, `Control`, `Alt`, `Meta`, `\n`, `\r`), or the key or
/// shifted key of a key off the numpad. `ControlOrMeta` is Meta on macOS.
fn lookup(name: &str) -> Option<Named> {
    let name = match name {
        "ControlOrMeta" if cfg!(target_os = "macos") => "Meta",
        "ControlOrMeta" => "Control",
        n => n,
    };
    let alias = match name {
        "Shift" => Some("ShiftLeft"),
        "Control" => Some("ControlLeft"),
        "Alt" => Some("AltLeft"),
        "Meta" => Some("MetaLeft"),
        "\n" | "\r" => Some("Enter"),
        _ => None,
    };
    if let Some(code) = alias {
        let def = LAYOUT.iter().find(|d| d.code == code)?;
        return Some(Named { def, shiftable: false, shifted: false });
    }
    if let Some(def) = LAYOUT.iter().find(|d| d.code == name) {
        return Some(Named { def, shiftable: true, shifted: false });
    }
    // By what a key types: the last key of the layout giving it wins.
    let mut found = None;
    for def in LAYOUT.iter().filter(|d| d.location == 0) {
        if def.key.chars().count() == 1 && def.key == name {
            found = Some(Named { def, shiftable: false, shifted: false });
        }
        if def.shift_key == Some(name) {
            found = Some(Named { def, shiftable: false, shifted: true });
        }
    }
    found
}

/// Whether `name` is a key the layout has.
pub(super) fn is_key(name: &str) -> bool {
    lookup(name).is_some()
}

/// The keys of a chord, split as Playwright splits it: at each `+` that
/// follows a key name (`+`, `Shift++` and `Control++` end with the `+` key).
pub(super) fn split_chord(chord: &str) -> Vec<String> {
    let mut keys = Vec::new();
    let mut building = String::new();
    for c in chord.chars() {
        if c == '+' && !building.is_empty() {
            keys.push(std::mem::take(&mut building));
        } else {
            building.push(c);
        }
    }
    keys.push(building);
    keys
}

/// The first key of `chord` the layout does not have.
pub(super) fn unknown_key(chord: &str) -> Option<String> {
    split_chord(chord).into_iter().find(|key| !is_key(key))
}

fn modifier_bit(key: &str, m: &mut Modifiers) -> Option<&'static str> {
    match key {
        "Shift" => {
            m.shift = true;
            Some("Shift")
        }
        "Control" => {
            m.ctrl = true;
            Some("Control")
        }
        "Alt" => {
            m.alt = true;
            Some("Alt")
        }
        "Meta" => {
            m.meta = true;
            Some("Meta")
        }
        _ => None,
    }
}

fn clear_modifier(key: &str, m: &mut Modifiers) {
    match key {
        "Shift" => m.shift = false,
        "Control" => m.ctrl = false,
        "Alt" => m.alt = false,
        "Meta" => m.meta = false,
        _ => {}
    }
}

/// The key event of `named` with `modifiers` held (before it goes down):
/// Shift picks a code's shifted key; with any modifier but Shift held, no text.
fn key_input(named: Named, modifiers: Modifiers) -> KeyInput {
    let def = named.def;
    let shifted = named.shifted || (named.shiftable && modifiers.shift && def.shift_key.is_some());
    let key = if shifted { def.shift_key.unwrap_or(def.key) } else { def.key };
    let mut text = match def.text {
        Some(t) => t.to_owned(),
        None if key.chars().count() == 1 && key != "\u{0}" => key.to_owned(),
        None => String::new(),
    };
    if modifiers.ctrl || modifiers.alt || modifiers.meta {
        text.clear();
    }
    KeyInput {
        key: key.to_owned(),
        code: def.code.to_owned(),
        text,
        modifiers,
        location: def.location,
        key_code: def.key_code,
        ..Default::default()
    }
}

/// The key events of pressing `chord` while `held` (modifier keys
/// `keyboard.down` holds, no events of their own): each key down in order (a
/// modifier key holds its modifier from its own keydown on), the last one
/// up, then the others up in reverse (a modifier released before its keyup).
/// `Err`: the first key the layout does not have.
pub(super) fn chord(chord: &str, held: Modifiers) -> Result<Vec<(KeyKind, KeyInput)>, String> {
    let keys = split_chord(chord);
    let mut named = Vec::with_capacity(keys.len());
    for key in &keys {
        named.push(lookup(key).ok_or_else(|| key.clone())?);
    }
    let mut modifiers = held;
    let mut downs = Vec::with_capacity(named.len());
    let mut events = Vec::with_capacity(named.len() * 2);
    for n in &named {
        let mut input = key_input(*n, modifiers);
        modifier_bit(n.def.key, &mut modifiers);
        input.modifiers = modifiers;
        let kind = if input.text.is_empty() { KeyKind::RawKeyDown } else { KeyKind::KeyDown };
        events.push((kind, input.clone()));
        downs.push(input);
    }
    for input in downs.into_iter().rev() {
        clear_modifier(&input.key, &mut modifiers);
        let mut up = input;
        up.modifiers = modifiers;
        up.text.clear();
        events.push((KeyKind::KeyUp, up));
    }
    Ok(events)
}

/// The key events that type `c` while `held`, as Playwright's
/// `keyboard.type` types it: a press of the key that types it, or `None`
/// when no key of the layout does (it is inserted as text).
pub(super) fn type_char(c: char, held: Modifiers) -> Option<Vec<(KeyKind, KeyInput)>> {
    chord(c.encode_utf8(&mut [0; 4]), held).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(chord_text: &str) -> Vec<String> {
        chord(chord_text, Modifiers::default())
            .unwrap()
            .into_iter()
            .map(|(kind, k)| {
                let kind = match kind {
                    KeyKind::KeyDown => "down",
                    KeyKind::RawKeyDown => "rawdown",
                    KeyKind::Char => "char",
                    KeyKind::KeyUp => "up",
                };
                let m = k.modifiers;
                let mods = [(m.shift, "S"), (m.ctrl, "C"), (m.alt, "A"), (m.meta, "M")]
                    .iter()
                    .filter(|(on, _)| *on)
                    .map(|(_, l)| *l)
                    .collect::<String>();
                format!("{kind} {}|{}|{}|{:?}|loc{}|{mods}", k.key, k.code, k.key_code, k.text, k.location)
            })
            .collect()
    }

    #[test]
    fn codes_press_their_keys_as_playwright_does() {
        assert_eq!(keys("KeyD"), ["down d|KeyD|68|\"d\"|loc0|", "up d|KeyD|68|\"\"|loc0|"]);
        assert_eq!(
            keys("Alt+KeyD"),
            [
                "rawdown Alt|AltLeft|18|\"\"|loc1|A",
                "rawdown d|KeyD|68|\"\"|loc0|A",
                "up d|KeyD|68|\"\"|loc0|A",
                "up Alt|AltLeft|18|\"\"|loc1|",
            ]
        );
        assert_eq!(
            keys("Shift+KeyA"),
            [
                "rawdown Shift|ShiftLeft|16|\"\"|loc1|S",
                "down A|KeyA|65|\"A\"|loc0|S",
                "up A|KeyA|65|\"\"|loc0|S",
                "up Shift|ShiftLeft|16|\"\"|loc1|",
            ]
        );
        // A key named by what it types is not shifted.
        assert_eq!(keys("Shift+d")[1], "down d|KeyD|68|\"d\"|loc0|S");
        assert_eq!(keys("Shift+Digit1")[1], "down !|Digit1|49|\"!\"|loc0|S");
        assert_eq!(keys("!")[0], "down !|Digit1|49|\"!\"|loc0|");
        assert_eq!(keys("Digit1")[0], "down 1|Digit1|49|\"1\"|loc0|");
        assert_eq!(keys("Shift+Minus")[1], "down _|Minus|189|\"_\"|loc0|S");
        assert_eq!(keys("Backquote")[0], "down `|Backquote|192|\"`\"|loc0|");
        assert_eq!(keys("Quote")[0], "down '|Quote|222|\"'\"|loc0|");
        assert_eq!(keys("+")[0], "down +|Equal|187|\"+\"|loc0|");
        assert_eq!(keys("Control++")[1], "rawdown +|Equal|187|\"\"|loc0|C");
        assert_eq!(keys("Numpad1")[0], "rawdown End|Numpad1|35|\"\"|loc3|");
        assert_eq!(keys("NumpadAdd")[0], "down +|NumpadAdd|107|\"+\"|loc3|");
        assert_eq!(keys("NumpadEnter")[0], "down Enter|NumpadEnter|13|\"\\r\"|loc3|");
        assert_eq!(keys("NumpadDecimal")[0], "rawdown \u{0}|NumpadDecimal|46|\"\"|loc3|");
        assert_eq!(keys("\n")[0], "down Enter|Enter|13|\"\\r\"|loc0|");
        assert_eq!(keys("Space")[0], "down  |Space|32|\" \"|loc0|");
        assert_eq!(keys("ShiftRight")[0], "rawdown Shift|ShiftRight|16|\"\"|loc2|S");
        assert_eq!(keys("Control+Shift+KeyZ")[2], "rawdown Z|KeyZ|90|\"\"|loc0|SC");
        assert!(keys("ControlOrMeta+KeyA")[1].ends_with(if cfg!(target_os = "macos") { "|M" } else { "|C" }));
    }

    #[test]
    fn unknown_keys_are_named() {
        assert_eq!(chord("NoSuchKey", Modifiers::default()).err().as_deref(), Some("NoSuchKey"));
        assert_eq!(chord("Shift+NoSuch", Modifiers::default()).err().as_deref(), Some("NoSuch"));
        // Numpad keys are reached by code only; characters off the layout are unknown.
        assert_eq!(unknown_key("Clear").as_deref(), Some("Clear"));
        assert_eq!(unknown_key("é").as_deref(), Some("é"));
        assert_eq!(unknown_key("Alt+KeyD"), None);
        assert_eq!(split_chord("Shift++"), ["Shift", "+"]);
        assert_eq!(split_chord("+"), ["+"]);
        assert!(type_char('é', Modifiers::default()).is_none());
        assert!(type_char('\n', Modifiers::default()).is_some());
        // Modifiers `keyboard.down` holds: no events of their own, kept on the key's.
        let held = Modifiers { shift: true, ..Default::default() };
        let events = chord("KeyA", held).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!((events[0].1.key.as_str(), events[0].1.modifiers.shift), ("A", true));
        assert_eq!((events[1].1.key.as_str(), events[1].1.modifiers.shift), ("A", true));
    }
}
