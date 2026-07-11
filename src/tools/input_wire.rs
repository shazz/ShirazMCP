//! Pure key/event parsing for `send_key` — everything here is validated
//! CLIENT-SIDE because `Control_ProcessBuffer` aborts the WHOLE read batch
//! at the first failing command (v2.4.1 `control.c:371`'s
//! `while (ok && cmdend && *cmd)`, P27 §1.2): we must never send an event
//! Hatari could reject.

/// `control.c`-verbatim wire tokens (ADR-11 §2.1 convention: never build
/// these ad hoc). Sources (v2.4.1 tag): `control.c:341` (`hatari-event`
/// dispatch), `:71/:74/:78` (keypress/keydown/keyup), `:133/:137/:141`
/// (mouse events).
pub(crate) const EVENT_CMD: &str = "hatari-event";
pub(crate) const ACTION_PRESS: &str = "keypress";
pub(crate) const ACTION_DOWN: &str = "keydown";
pub(crate) const ACTION_UP: &str = "keyup";
pub(crate) const MOUSE_DOUBLECLICK: &str = "doubleclick";
pub(crate) const MOUSE_RIGHTDOWN: &str = "rightdown";
pub(crate) const MOUSE_RIGHTUP: &str = "rightup";

/// Longest wire line this module ever emits:
/// `"hatari-event keypress 255\n"` = 26 bytes.
const WIRE_LINE_MAX: usize = 26;
/// Hatari v2.4.1's `Control_CheckUpdates()` reads at most `sizeof(buffer)-1`
/// = 399 bytes per poll (`char buffer[400]`, `control.c:401`; the `read()`,
/// `control.c:409`) and never reassembles a command split across two reads
/// (`control.c:408`'s own "assume whole command can be read in one go").
const FIFO_READ_CAP: usize = 399;
/// Headroom for one confirm-sentinel line (`"hatari-debug evaluate
/// $deadNNNN"`-shaped) coalescing into the same read window as a queued
/// batch.
const SENTINEL_HEADROOM: usize = 35;

/// Max entries per `send_key` call — bounded by Hatari's fifo read window,
/// not by UX (P27/ADR-44). **Deviation from the phase spec**: P27.md §3.2
/// pinned `KEYS_MAX = 32`, justified by "Hatari's 4096-byte read buffer
/// (control.c:428)" — verified against the real v2.4.1 tag, that citation
/// is wrong: the buffer is 400 bytes (399 usable), an order of magnitude
/// smaller, and `Control_ProcessBuffer`'s batch-abort means a split command
/// doesn't just corrupt itself — it drops everything queued behind it in
/// that read. At 32 worst-case entries (832 B) the batch exceeds the window
/// on every `send_key` path, including the Paused/queued path where lines
/// coalesce in the pipe with nothing draining them until resume. `KEYS_MAX`
/// is derived from the real window instead: 14 worst-case lines (26 B each)
/// plus one confirm sentinel fit in one poll — `14*26 + 35 == 399`,
/// const-asserted below so a token-table change can't silently break the
/// bound. Still covers an 8.3 GEMDOS filename plus `.` (scancode) and enter
/// (13 entries); longer sequences are multiple calls (the tool is cheap and
/// stateless). See ADR-44's Consequences for the accepted residual risk
/// (stacking several *queued* `send_key` calls while Paused can still
/// exceed the window — per-call bounding cannot close that).
pub(crate) const KEYS_MAX: usize = 14;
const _: () = assert!(KEYS_MAX * WIRE_LINE_MAX + SENTINEL_HEADROOM <= FIFO_READ_CAP);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
    Press,
    Down,
    Up,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum KeyEvent {
    /// Single ASCII alphanumeric char — `control.c:104`'s `isalnum` gate.
    Char(char),
    /// ST scancode 0..=255 — `control.c:92`'s range check.
    Scancode(u8),
    /// One of the three `control.c` mouse events (complete events; only
    /// valid with `Action::Press`).
    Mouse(&'static str),
}

/// Case-insensitive alias table pinned from `keymap.c` (v2.4.1 tag,
/// `Keymap_PcToStScanCode`) plus the three `Control_InsertEvent` mouse
/// tokens. Do not add entries not backed by that source.
const ALIASES: &[(&str, KeyEvent)] = &[
    ("space", KeyEvent::Scancode(0x39)),
    ("enter", KeyEvent::Scancode(0x1c)),
    ("return", KeyEvent::Scancode(0x1c)),
    ("esc", KeyEvent::Scancode(0x01)),
    ("backspace", KeyEvent::Scancode(0x0e)),
    ("tab", KeyEvent::Scancode(0x0f)),
    ("delete", KeyEvent::Scancode(0x53)),
    ("up", KeyEvent::Scancode(0x48)),
    ("down", KeyEvent::Scancode(0x50)),
    ("left", KeyEvent::Scancode(0x4b)),
    ("right", KeyEvent::Scancode(0x4d)),
    ("f1", KeyEvent::Scancode(0x3b)),
    ("f2", KeyEvent::Scancode(0x3c)),
    ("f3", KeyEvent::Scancode(0x3d)),
    ("f4", KeyEvent::Scancode(0x3e)),
    ("f5", KeyEvent::Scancode(0x3f)),
    ("f6", KeyEvent::Scancode(0x40)),
    ("f7", KeyEvent::Scancode(0x41)),
    ("f8", KeyEvent::Scancode(0x42)),
    ("f9", KeyEvent::Scancode(0x43)),
    ("f10", KeyEvent::Scancode(0x44)),
    ("help", KeyEvent::Scancode(0x62)),
    ("undo", KeyEvent::Scancode(0x61)),
    ("insert", KeyEvent::Scancode(0x52)),
    ("home", KeyEvent::Scancode(0x47)),
    ("capslock", KeyEvent::Scancode(0x3a)),
    ("control", KeyEvent::Scancode(0x1d)),
    ("lshift", KeyEvent::Scancode(0x2a)),
    ("rshift", KeyEvent::Scancode(0x36)),
    ("alternate", KeyEvent::Scancode(0x38)),
    ("alt", KeyEvent::Scancode(0x38)),
    ("doubleclick", KeyEvent::Mouse(MOUSE_DOUBLECLICK)),
    ("rightdown", KeyEvent::Mouse(MOUSE_RIGHTDOWN)),
    ("rightup", KeyEvent::Mouse(MOUSE_RIGHTUP)),
];

pub(crate) fn parse_action(s: Option<&str>) -> Result<Action, String> {
    match s {
        None => Ok(Action::Press),
        Some(a) if a.eq_ignore_ascii_case("press") => Ok(Action::Press),
        Some(a) if a.eq_ignore_ascii_case("down") => Ok(Action::Down),
        Some(a) if a.eq_ignore_ascii_case("up") => Ok(Action::Up),
        Some(other) => Err(format!("action must be press, down, or up (got '{other}')")),
    }
}

/// §3.4 rule order: (1) empty ⇒ err, (2) exactly one char ⇒ ALWAYS the
/// character path (digits included — mirrors `control.c:87`'s `key[1]`
/// check, which only takes the scancode branch when a SECOND character is
/// present), (3) alias table, (4) numeric scancode (hex or decimal, leading
/// zero rejected as ambiguous octal), (5) else err.
pub(crate) fn parse_key_entry(entry: &str) -> Result<KeyEvent, String> {
    let mut chars = entry.chars();
    let first = chars.next().ok_or_else(|| "empty key entry".to_string())?;
    if chars.next().is_none() {
        return if first.is_ascii_alphanumeric() {
            Ok(KeyEvent::Char(first))
        } else {
            Err(format!(
                "non-alphanumeric character '{first}' must be given as an ST scancode \
                 (e.g. space is 57, enter is 28)"
            ))
        };
    }
    if let Some((_, event)) = ALIASES
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(entry))
    {
        return Ok(event.clone());
    }
    parse_scancode(entry)
}

fn parse_scancode(entry: &str) -> Result<KeyEvent, String> {
    let value = if let Some(hex) = entry
        .strip_prefix("0x")
        .or_else(|| entry.strip_prefix("0X"))
    {
        u32::from_str_radix(hex, 16).map_err(|_| bad_key(entry))?
    } else if entry.chars().all(|c| c.is_ascii_digit()) {
        // Hatari's scancode parse is `strtol(key, &endptr, 0)` — base 0
        // reads a leading zero as octal; refuse the ambiguity instead of
        // silently re-basing (§3.4 rule 4).
        if entry.len() > 1 && entry.starts_with('0') {
            return Err(bad_key(entry));
        }
        entry.parse::<u32>().map_err(|_| bad_key(entry))?
    } else {
        return Err(bad_key(entry));
    };
    u8::try_from(value)
        .map(KeyEvent::Scancode)
        .map_err(|_| bad_key(entry))
}

fn bad_key(entry: &str) -> String {
    format!(
        "'{entry}' is not a valid key: use a single ASCII alphanumeric character, a decimal or \
         0x-hex ST scancode 0..=255, a named alias (space, enter, esc, f1..f10, \
         up/down/left/right, help, undo, ...), or a mouse event (doubleclick, rightdown, \
         rightup)"
    )
}

/// One wire line per event, e.g. `"hatari-event keypress 57"`. `Err` if a
/// `Mouse` event is combined with `Down`/`Up` — mouse events carry no
/// action word (`control.c:133-141`) and are complete presses.
pub(crate) fn wire_lines(events: &[KeyEvent], action: Action) -> Result<Vec<String>, String> {
    events.iter().map(|e| wire_line(e, action)).collect()
}

fn wire_line(event: &KeyEvent, action: Action) -> Result<String, String> {
    match event {
        KeyEvent::Char(c) => Ok(format!("{EVENT_CMD} {} {c}", action_word(action))),
        // Scancodes are emitted decimal, no leading zeros — unambiguous for
        // Hatari's base-0 `strtol`.
        KeyEvent::Scancode(n) => Ok(format!("{EVENT_CMD} {} {n}", action_word(action))),
        KeyEvent::Mouse(token) if action == Action::Press => Ok(format!("{EVENT_CMD} {token}")),
        KeyEvent::Mouse(token) => Err(format!(
            "mouse event '{token}' has no down/up state — only \"press\" is valid"
        )),
    }
}

fn action_word(action: Action) -> &'static str {
    match action {
        Action::Press => ACTION_PRESS,
        Action::Down => ACTION_DOWN,
        Action::Up => ACTION_UP,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_alnum_char() {
        assert_eq!(parse_key_entry("a"), Ok(KeyEvent::Char('a')));
        assert_eq!(parse_key_entry("Z"), Ok(KeyEvent::Char('Z')));
        assert_eq!(parse_key_entry("5"), Ok(KeyEvent::Char('5')));
        assert!(parse_key_entry("!").is_err());
        assert!(parse_key_entry(" ").is_err());
        assert!(parse_key_entry("").is_err());
    }

    #[test]
    fn parse_scancode_forms() {
        assert_eq!(parse_key_entry("57"), Ok(KeyEvent::Scancode(57)));
        assert_eq!(parse_key_entry("0x39"), Ok(KeyEvent::Scancode(57)));
        // Single-char precedence: "0" is ALWAYS the character path, never
        // scancode 0 (pins §3.4 rule 2 over rule 4).
        assert_eq!(parse_key_entry("0"), Ok(KeyEvent::Char('0')));
        assert_eq!(parse_key_entry("0x00"), Ok(KeyEvent::Scancode(0)));
        assert_eq!(parse_key_entry("255"), Ok(KeyEvent::Scancode(255)));
        assert!(parse_key_entry("256").is_err());
        assert!(parse_key_entry("-1").is_err());
        assert!(parse_key_entry("057").is_err());
        assert!(parse_key_entry("1x").is_err());
    }

    #[test]
    fn parse_aliases() {
        assert_eq!(parse_key_entry("space"), Ok(KeyEvent::Scancode(57)));
        assert_eq!(parse_key_entry("ENTER"), Ok(KeyEvent::Scancode(28)));
        assert_eq!(parse_key_entry("f10"), Ok(KeyEvent::Scancode(0x44)));
        assert_eq!(
            parse_key_entry("doubleclick"),
            Ok(KeyEvent::Mouse(MOUSE_DOUBLECLICK))
        );
    }

    #[test]
    fn wire_lines_verbatim() {
        assert_eq!(
            wire_lines(&[KeyEvent::Char('a')], Action::Press),
            Ok(vec!["hatari-event keypress a".to_string()])
        );
        assert_eq!(
            wire_lines(&[KeyEvent::Scancode(57)], Action::Down),
            Ok(vec!["hatari-event keydown 57".to_string()])
        );
        assert_eq!(
            wire_lines(&[KeyEvent::Mouse(MOUSE_DOUBLECLICK)], Action::Press),
            Ok(vec!["hatari-event doubleclick".to_string()])
        );
        assert!(wire_lines(&[KeyEvent::Mouse(MOUSE_DOUBLECLICK)], Action::Up).is_err());
    }

    #[test]
    fn parse_action_default_and_reject() {
        assert_eq!(parse_action(None), Ok(Action::Press));
        assert_eq!(parse_action(Some("down")), Ok(Action::Down));
        assert_eq!(parse_action(Some("up")), Ok(Action::Up));
        assert_eq!(parse_action(Some("PRESS")), Ok(Action::Press));
        assert!(parse_action(Some("click")).is_err());
    }
}
