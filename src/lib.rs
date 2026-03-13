//! `egui-command-binding` — keyboard shortcut → [`CommandId`] dispatch for egui apps.
//!
//! Wraps `egui-command` types with egui-specific input handling.
//! `ShortcutManager<C>` scans egui `Key` events and returns a `Vec<C>` of
//! triggered commands — it never executes business logic directly.
//!
//! # Quick-start
//! ```rust,ignore
//! // Define your command type (typically a C: From<CommandId> enum).
//! #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
//! enum AppCmd { ShowHelp, PrevProfile, NextProfile }
//!
//! // Build the global map once (e.g. in a lazy_static):
//! let mut global: ShortcutMap<AppCmd> = ShortcutMap::new();
//! global.insert(shortcut("F1"),  AppCmd::ShowHelp);
//! global.insert(shortcut("F7"),  AppCmd::PrevProfile);
//! global.insert(shortcut("F8"),  AppCmd::NextProfile);
//!
//! // Each frame, collect triggered commands:
//! let triggered = manager.dispatch(ctx);
//! for cmd in triggered { handle(cmd); }
//! ```

pub use egui_command;
use {
    egui::{Context, Key, Modifiers},
    egui_command::{CommandId, CommandSource, CommandTriggered},
    parking_lot::RwLock,
    std::{collections::HashMap, sync::Arc},
};

/// A keyboard shortcut: a key plus zero-or-more modifier keys.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Shortcut {
    pub key: Key,
    pub mods: Modifiers,
}

/// Maps `Shortcut → C` (an app-defined command value).
pub type ShortcutMap<C> = HashMap<Shortcut, C>;

/// A named, optionally-consuming scope of shortcuts.
///
/// Scopes are pushed/popped by context (e.g. while an editor view is active).
/// When `consume = true`, a match in this scope stops propagation to lower scopes.
pub struct ShortcutScope<C> {
    pub name: &'static str,
    pub shortcuts: ShortcutMap<C>,
    pub consume: bool,
}

impl<C> ShortcutScope<C> {
    /// Creates a new scope with the given name, shortcut map, and consume flag.
    pub fn new(name: &'static str, shortcuts: ShortcutMap<C>, consume: bool) -> Self {
        Self {
            name,
            shortcuts,
            consume,
        }
    }
}

/// Scans egui key events each frame and returns triggered commands.
///
/// Lookup order: scoped stack (top → bottom) → global.
/// The first consuming scope that matches stops propagation.
pub struct ShortcutManager<C> {
    global: Arc<RwLock<ShortcutMap<C>>>,
    stack: Vec<ShortcutScope<C>>,
}

impl<C: Clone> ShortcutManager<C> {
    pub fn new(global: Arc<RwLock<ShortcutMap<C>>>) -> Self {
        Self {
            global,
            stack: Vec::new(),
        }
    }

    /// Pushes a new scope onto the stack. Scopes are checked top-to-bottom during dispatch.
    pub fn push_scope(&mut self, scope: ShortcutScope<C>) { self.stack.push(scope); }

    /// Removes the top scope from the stack.
    pub fn pop_scope(&mut self) { self.stack.pop(); }

    /// Inserts or replaces a shortcut in the shared global map.
    pub fn register_global(&mut self, sc: Shortcut, cmd: C) { self.global.write().insert(sc, cmd); }

    /// Scan egui key events and return all triggered commands this frame.
    ///
    /// Matched key events are consumed from the egui input queue so that
    /// egui widgets don't double-handle them.
    ///
    /// Returns an empty `Vec` when [`Context::wants_keyboard_input`] is `true`
    /// (i.e. a text-edit widget has focus) so that typing never fires shortcuts.
    pub fn dispatch(&self, ctx: &Context) -> Vec<CommandTriggered>
    where
        C: Into<CommandId>,
    {
        if ctx.wants_keyboard_input() {
            return Vec::new();
        }

        self.dispatch_raw_inner(ctx, None)
            .into_iter()
            .map(|cmd| CommandTriggered::new(cmd.into(), CommandSource::Keyboard))
            .collect()
    }

    /// Dispatch with an optional extra scope checked before global shortcuts.
    ///
    /// Use this when a context-specific shortcut map (e.g. editor scope) should
    /// take priority without needing `push_scope`/`pop_scope` on a mutable static.
    /// The extra scope is always consuming: a match there skips the global map.
    ///
    /// Returns an empty `Vec` when [`Context::wants_keyboard_input`] is `true`.
    pub fn dispatch_raw_with_extra(&self, ctx: &Context, extra: Option<&ShortcutMap<C>>) -> Vec<C> {
        if ctx.wants_keyboard_input() {
            return Vec::new();
        }

        self.dispatch_raw_inner(ctx, extra)
    }

    /// Dispatch without converting to `CommandTriggered` — returns raw `C` values.
    ///
    /// Returns an empty `Vec` when [`Context::wants_keyboard_input`] is `true`.
    pub fn dispatch_raw(&self, ctx: &Context) -> Vec<C> {
        if ctx.wants_keyboard_input() {
            return Vec::new();
        }

        self.dispatch_raw_inner(ctx, None)
    }

    /// Shared implementation for all dispatch variants.
    ///
    /// Does **not** check `wants_keyboard_input`; callers are responsible for
    /// that guard.  `extra`, when provided, is checked first and always consumes
    /// (a match there skips the scoped stack and global map for that key).
    fn dispatch_raw_inner(&self, ctx: &Context, extra: Option<&ShortcutMap<C>>) -> Vec<C> {
        let mut triggered: Vec<C> = Vec::new();

        ctx.input_mut(|input| {
            let mut consumed: Vec<Shortcut> = Vec::new();

            for event in &input.events {
                let egui::Event::Key {
                    key,
                    pressed: true,
                    repeat: false,
                    modifiers,
                    ..
                } = event
                else {
                    continue;
                };
                let sc = Shortcut {
                    key: *key,
                    mods: *modifiers,
                };

                // Extra scope has highest priority and is always consuming.
                if let Some(extra_map) = extra
                    && let Some(cmd) = extra_map.get(&sc)
                {
                    triggered.push(cmd.clone());
                    consumed.push(sc);
                    continue;
                }

                // Scoped stack: top of stack first, stop at first consuming scope.
                let mut matched = false;
                'scopes: for scope in self.stack.iter().rev() {
                    if let Some(cmd) = scope.shortcuts.get(&sc) {
                        triggered.push(cmd.clone());
                        consumed.push(sc);
                        matched = true;
                        if scope.consume {
                            break 'scopes;
                        }
                    }
                }
                if matched {
                    continue;
                }

                // Fall back to global map.
                if let Some(cmd) = self.global.read().get(&sc) {
                    triggered.push(cmd.clone());
                    consumed.push(sc);
                }
            }

            for sc in consumed {
                input.consume_key(sc.mods, sc.key);
            }
        });

        triggered
    }
}

/// Parse a shortcut string like `"Ctrl+S"`, `"F2"`, `"Alt+Shift+X"`.
///
/// Token matching is case-insensitive.  Panics if the key token is unrecognised.
pub fn shortcut(sc: &str) -> Shortcut {
    let mut mods = Modifiers::default();
    let mut key = None;

    for part in sc.split('+') {
        let part = part.trim();
        match part.to_uppercase().as_str() {
            "CTRL" | "CONTROL" => mods.ctrl = true,
            "ALT" => mods.alt = true,
            "SHIFT" => mods.shift = true,
            "META" | "CMD" | "COMMAND" => mods.mac_cmd = true,
            // Key::from_name is case-sensitive (egui uses PascalCase, e.g. "Escape", "F1", "A").
            // Pass the original part (trimmed) so "Escape" stays "Escape", not "ESCAPE".
            _ => key = Key::from_name(part),
        }
    }

    Shortcut {
        key: key.expect("Invalid key in shortcut string"),
        mods,
    }
}

/// Build a [`ShortcutMap`] from `shortcut_string => command` pairs.
///
/// # Example
/// ```rust,ignore
/// let map = shortcut_map![
///     "F1" => AppCmd::ShowHelp,
///     "F7" => AppCmd::PrevProfile,
/// ];
/// ```
#[macro_export]
macro_rules! shortcut_map {
    ($($key:expr => $cmd:expr),* $(,)?) => {{
        #[allow(unused_mut)]
        let mut map = $crate::ShortcutMap::new();
        $(map.insert($crate::shortcut($key), $cmd);)*
        map
    }};
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        egui::{Key, Modifiers},
    };

    #[test]
    fn shortcut_single_key() {
        let sc = shortcut("F1");
        assert_eq!(sc.key, Key::F1);
        assert_eq!(sc.mods, Modifiers::default());
    }

    #[test]
    fn shortcut_ctrl_s() {
        let sc = shortcut("Ctrl+S");
        assert_eq!(sc.key, Key::S);
        assert!(sc.mods.ctrl);
        assert!(!sc.mods.alt);
        assert!(!sc.mods.shift);
    }

    #[test]
    fn shortcut_alt_shift_x() {
        let sc = shortcut("Alt+Shift+X");
        assert_eq!(sc.key, Key::X);
        assert!(sc.mods.alt);
        assert!(sc.mods.shift);
        assert!(!sc.mods.ctrl);
    }

    #[test]
    fn shortcut_control_alias() {
        let sc = shortcut("Control+A");
        assert!(sc.mods.ctrl);
        assert_eq!(sc.key, Key::A);
    }

    #[test]
    #[should_panic]
    fn shortcut_invalid_key_panics() { shortcut("Ctrl+NotAKey"); }

    #[test]
    fn shortcut_map_macro_builds_correctly() {
        let map = shortcut_map![
            "F1" => 1u32,
            "F2" => 2u32,
        ];
        assert_eq!(map.get(&shortcut("F1")), Some(&1u32));
        assert_eq!(map.get(&shortcut("F2")), Some(&2u32));
        assert_eq!(map.get(&shortcut("F3")), None);
    }

    #[test]
    fn shortcut_map_macro_empty() {
        let map: ShortcutMap<u32> = shortcut_map![];
        assert!(map.is_empty());
    }

    #[test]
    fn shortcut_equality_and_hash() {
        use std::collections::HashMap;
        let mut m: HashMap<Shortcut, &str> = HashMap::new();
        m.insert(shortcut("Ctrl+S"), "save");
        assert_eq!(m[&shortcut("Ctrl+S")], "save");
        assert!(!m.contains_key(&shortcut("Ctrl+Z")));
    }
}
