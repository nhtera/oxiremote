/// Input injection: translates normalized InputEvent values to enigo calls.
use anyhow::Context;
use enigo::{
    Axis, Button, Coordinate, Direction, Enigo, Key, Keyboard, Mouse, Settings,
};
use tracing::warn;

use crate::encode::QualityTier;

/// Press or release action for a button or key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PressAction {
    Press,
    Release,
    Click,
}

/// Which mouse button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseBtn {
    Left,
    Right,
    Middle,
}

/// All input events that the remote client can send.
#[derive(Debug, Clone)]
pub enum InputEvent {
    /// Absolute mouse position, normalized to [0.0, 1.0] of screen dimensions.
    MouseMove { x: f32, y: f32 },
    /// Mouse button press / release / click.
    MouseButton { btn: MouseBtn, action: PressAction },
    /// Scroll wheel delta in logical units.
    Scroll { dx: i32, dy: i32 },
    /// Keyboard key event. `code` is a DOM `KeyboardEvent.code` string.
    Key {
        code: String,
        action: PressAction,
        ctrl: bool,
        shift: bool,
        alt: bool,
        meta: bool,
    },
    /// Client requests a quality change for the capture loop.
    QualityChange { tier: QualityTier },
}

/// Translates [`InputEvent`] values into real OS-level input via enigo.
///
/// One `InputInjector` per desktop session; create with `InputInjector::new()`.
pub struct InputInjector {
    enigo: Enigo,
}

impl InputInjector {
    /// Create a new injector. Returns an error if the OS denies accessibility
    /// permissions or if enigo cannot initialise (e.g., headless Linux).
    pub fn new() -> anyhow::Result<Self> {
        let enigo = Enigo::new(&Settings::default()).context("enigo init")?;
        Ok(InputInjector { enigo })
    }

    /// Apply one input event.
    ///
    /// `screen_w` / `screen_h` are the pixel dimensions of the capture source
    /// (after quality resize) so that normalized coordinates map correctly.
    pub fn apply(
        &mut self,
        event: InputEvent,
        screen_w: u32,
        screen_h: u32,
    ) -> anyhow::Result<()> {
        match event {
            InputEvent::MouseMove { x, y } => {
                let px = (x.clamp(0.0, 1.0) * screen_w as f32) as i32;
                let py = (y.clamp(0.0, 1.0) * screen_h as f32) as i32;
                self.enigo
                    .move_mouse(px, py, Coordinate::Abs)
                    .context("mouse move")?;
            }

            InputEvent::MouseButton { btn, action } => {
                let button = map_mouse_button(btn);
                let dir = map_direction(action);
                self.enigo.button(button, dir).context("mouse button")?;
            }

            InputEvent::Scroll { dx, dy } => {
                if dy != 0 {
                    self.enigo
                        .scroll(dy, Axis::Vertical)
                        .context("scroll vertical")?;
                }
                if dx != 0 {
                    self.enigo
                        .scroll(dx, Axis::Horizontal)
                        .context("scroll horizontal")?;
                }
            }

            InputEvent::Key {
                code,
                action,
                ctrl,
                shift,
                alt,
                meta,
            } => {
                // Press modifiers before the key, release after.
                if ctrl {
                    let _ = self.enigo.key(Key::Control, Direction::Press);
                }
                if shift {
                    let _ = self.enigo.key(Key::Shift, Direction::Press);
                }
                if alt {
                    let _ = self.enigo.key(Key::Alt, Direction::Press);
                }
                if meta {
                    let _ = self.enigo.key(Key::Meta, Direction::Press);
                }

                match dom_code_to_key(&code) {
                    Some(key) => {
                        let dir = map_direction(action);
                        self.enigo.key(key, dir).context("key event")?;
                    }
                    None => {
                        warn!(code = %code, "unmapped DOM key code — ignored");
                    }
                }

                if meta {
                    let _ = self.enigo.key(Key::Meta, Direction::Release);
                }
                if alt {
                    let _ = self.enigo.key(Key::Alt, Direction::Release);
                }
                if shift {
                    let _ = self.enigo.key(Key::Shift, Direction::Release);
                }
                if ctrl {
                    let _ = self.enigo.key(Key::Control, Direction::Release);
                }
            }

            // QualityChange is handled by the caller (CaptureLoop restart) —
            // the injector only handles OS-level input events.
            InputEvent::QualityChange { .. } => {}
        }
        Ok(())
    }
}

// ─── Mapping helpers ─────────────────────────────────────────────────────────

fn map_mouse_button(btn: MouseBtn) -> Button {
    match btn {
        MouseBtn::Left => Button::Left,
        MouseBtn::Right => Button::Right,
        MouseBtn::Middle => Button::Middle,
    }
}

fn map_direction(action: PressAction) -> Direction {
    match action {
        PressAction::Press => Direction::Press,
        PressAction::Release => Direction::Release,
        PressAction::Click => Direction::Click,
    }
}

/// Map a DOM `KeyboardEvent.code` string to an enigo `Key`.
///
/// Covers US-layout codes (layout-independent `code` values, not `key`).
/// International keyboards: only the physical position is mapped; the
/// produced character may differ. This is intentional for v1.
fn dom_code_to_key(code: &str) -> Option<Key> {
    // Letters A–Z
    if let Some(ch) = code.strip_prefix("Key")
        && ch.len() == 1 {
            let c = ch.chars().next()?;
            if c.is_ascii_alphabetic() {
                return Some(Key::Unicode(c.to_ascii_lowercase()));
            }
        }

    // Digits 0–9
    if let Some(ch) = code.strip_prefix("Digit")
        && ch.len() == 1 {
            let c = ch.chars().next()?;
            if c.is_ascii_digit() {
                return Some(Key::Unicode(c));
            }
        }

    // Function keys F1–F12
    if let Some(num) = code.strip_prefix("F") {
        match num {
            "1" => return Some(Key::F1),
            "2" => return Some(Key::F2),
            "3" => return Some(Key::F3),
            "4" => return Some(Key::F4),
            "5" => return Some(Key::F5),
            "6" => return Some(Key::F6),
            "7" => return Some(Key::F7),
            "8" => return Some(Key::F8),
            "9" => return Some(Key::F9),
            "10" => return Some(Key::F10),
            "11" => return Some(Key::F11),
            "12" => return Some(Key::F12),
            _ => {}
        }
    }

    // Navigation, editing, and modifier keys.
    match code {
        "ArrowUp" => Some(Key::UpArrow),
        "ArrowDown" => Some(Key::DownArrow),
        "ArrowLeft" => Some(Key::LeftArrow),
        "ArrowRight" => Some(Key::RightArrow),
        "Home" => Some(Key::Home),
        "End" => Some(Key::End),
        "PageUp" => Some(Key::PageUp),
        "PageDown" => Some(Key::PageDown),
        "Enter" | "NumpadEnter" => Some(Key::Return),
        "Backspace" => Some(Key::Backspace),
        "Delete" => Some(Key::Delete),
        "Tab" => Some(Key::Tab),
        "Escape" => Some(Key::Escape),
        "Space" => Some(Key::Space),
        "ShiftLeft" | "ShiftRight" => Some(Key::Shift),
        "ControlLeft" | "ControlRight" => Some(Key::Control),
        "AltLeft" | "AltRight" => Some(Key::Alt),
        "MetaLeft" | "MetaRight" => Some(Key::Meta),
        "CapsLock" => Some(Key::CapsLock),
        // Insert not mapped in enigo 0.6 Key enum; skip silently.
        "Insert" => None,
        // Numpad digits
        "Numpad0" => Some(Key::Unicode('0')),
        "Numpad1" => Some(Key::Unicode('1')),
        "Numpad2" => Some(Key::Unicode('2')),
        "Numpad3" => Some(Key::Unicode('3')),
        "Numpad4" => Some(Key::Unicode('4')),
        "Numpad5" => Some(Key::Unicode('5')),
        "Numpad6" => Some(Key::Unicode('6')),
        "Numpad7" => Some(Key::Unicode('7')),
        "Numpad8" => Some(Key::Unicode('8')),
        "Numpad9" => Some(Key::Unicode('9')),
        _ => None,
    }
}
