// SPDX-License-Identifier: MIT OR Apache-2.0

//! `egui-command-binding` — keyboard shortcut → [`CommandId`] dispatch for egui apps.
//!
//! Wraps `egui-command` types with egui-specific input handling.
//! `ShortcutManager<C>` scans egui `Key` events and returns a `Vec<C>` of
//! triggered commands — it never executes business logic directly.
//! When an application also keeps command metadata in [`CommandRegistry`],
//! [`ShortcutManager::fill_shortcut_hints`] can copy the global shortcut map into
//! each registered [`egui_command::CommandSpec::shortcut_hint`] field so menus,
//! toolbars, and help overlays show the same display text as the active bindings.
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
//!
//! // Optional: populate display-only shortcut hints in a command registry.
//! manager.fill_shortcut_hints(&mut registry);
//! ```

pub use egui_command;
use {
    egui::{Context, Key, KeyboardShortcut, Modifiers},
    egui_command::{CommandId, CommandRegistry, CommandSource, CommandTriggered},
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
/// When `consume = true`, a match in this scope stops propagation to lower scopes
/// and to the global map.
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
/// Lookup order: extra scope → scoped stack (top → bottom) → global.
/// Non-consuming scopes continue propagation; consuming scopes stop lower scopes
/// and the global map. Within one scope/map, the most specific logical shortcut
/// wins.
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

    /// Populates [`CommandRegistry`] shortcut hints from the global shortcut map.
    ///
    /// For each `(Shortcut, C)` entry in the global map, formats the shortcut as a
    /// human-readable string (e.g. `"Ctrl+S"`, `"F1"`) and writes it into the
    /// corresponding [`CommandSpec::shortcut_hint`] in `registry`.
    ///
    /// Commands that have a shortcut binding but are not registered in `registry`
    /// are silently skipped.  Commands registered in `registry` that have no
    /// shortcut binding are left unchanged.
    pub fn fill_shortcut_hints<R>(&self, registry: &mut CommandRegistry<R>)
    where
        C: Into<CommandId> + Copy,
        R: Copy + std::hash::Hash + Eq + Into<CommandId>,
    {
        let global = self.global.read();
        for (shortcut, cmd) in global.iter() {
            let id: CommandId = (*cmd).into();
            if let Some(spec) = registry.spec_by_id_mut(id) {
                spec.shortcut_hint = Some(format_shortcut(shortcut));
            }
        }
    }

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

    /// Check whether a specific shortcut was pressed this frame, consuming it if so.
    ///
    /// Returns `Some(cmd)` if `sc` appears in the global shortcut map and was
    /// pressed this frame; `None` otherwise.
    ///
    /// Unlike [`dispatch`] / [`dispatch_raw`], this does **not** check
    /// `wants_keyboard_input` — use it only when you intentionally want to
    /// intercept a key even while a text field has focus.
    pub fn try_shortcut(&self, ctx: &Context, sc: Shortcut) -> Option<C> {
        let global = self.global.read();
        let cmd = global.get(&sc)?.clone();
        if ctx.input_mut(|i| i.consume_shortcut(&sc.to_keyboard_shortcut())) {
            Some(cmd)
        } else {
            None
        }
    }

    /// Shared implementation for all dispatch variants.
    ///
    /// Does **not** check `wants_keyboard_input`; callers are responsible for
    /// that guard. `extra`, when provided, is checked first and always consumes
    /// (a match there skips the scoped stack and global map for that key).
    fn dispatch_raw_inner(&self, ctx: &Context, extra: Option<&ShortcutMap<C>>) -> Vec<C> {
        let mut triggered: Vec<C> = Vec::new();
        let global = self.global.read();

        ctx.input_mut(|input| {
            let mut consumed: Vec<KeyboardShortcut> = Vec::new();

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

                // Extra scope has highest priority and is always consuming.
                if let Some(extra_map) = extra
                    && let Some((shortcut, cmd)) = best_shortcut_match(extra_map, *key, *modifiers)
                {
                    triggered.push(cmd.clone());
                    consumed.push(shortcut.to_keyboard_shortcut());
                    continue;
                }

                let mut stop_propagation = false;
                for scope in self.stack.iter().rev() {
                    if let Some((shortcut, cmd)) = best_shortcut_match(&scope.shortcuts, *key, *modifiers) {
                        triggered.push(cmd.clone());
                        consumed.push(shortcut.to_keyboard_shortcut());
                        if scope.consume {
                            stop_propagation = true;
                            break;
                        }
                    }
                }
                if stop_propagation {
                    continue;
                }

                // Fall back to global map.
                if let Some((shortcut, cmd)) = best_shortcut_match(&global, *key, *modifiers) {
                    triggered.push(cmd.clone());
                    consumed.push(shortcut.to_keyboard_shortcut());
                }
            }

            for shortcut in consumed {
                input.consume_shortcut(&shortcut);
            }
        });

        triggered
    }
}

fn best_shortcut_match<C>(
    map: &ShortcutMap<C>,
    key: Key,
    pressed_modifiers: Modifiers,
) -> Option<(Shortcut, &C)> {
    map.iter()
        .filter(|(shortcut, _)| shortcut.key == key && pressed_modifiers.matches_logically(shortcut.mods))
        .max_by_key(|(shortcut, _)| shortcut.specificity())
        .map(|(shortcut, command)| (*shortcut, command))
}

fn format_shortcut(sc: &Shortcut) -> String {
    let mut parts: Vec<String> = Vec::new();
    if sc.mods.ctrl { parts.push("Ctrl".into()); }
    if sc.mods.alt { parts.push("Alt".into()); }
    if sc.mods.shift { parts.push("Shift".into()); }
    if sc.mods.command { parts.push("Cmd".into()); }
    if sc.mods.mac_cmd { parts.push("Meta".into()); }
    parts.push(format!("{:?}", sc.key));
    parts.join("+")
}

impl Shortcut {
    fn specificity(self) -> u8 {
        self.mods.alt as u8
            + self.mods.shift as u8
            + self.mods.ctrl as u8
            + self.mods.command as u8
            + self.mods.mac_cmd as u8
    }

    fn to_keyboard_shortcut(self) -> KeyboardShortcut {
        KeyboardShortcut::new(self.mods, self.key)
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
            "META" => mods.mac_cmd = true,
            "CMD" | "COMMAND" => mods.command = true,
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
        egui::{Event, Key, Modifiers, RawInput},
    };

    fn key_event(key: Key, modifiers: Modifiers) -> Event {
        Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    fn dispatch_raw_events(manager: &ShortcutManager<u32>, events: Vec<Event>) -> Vec<u32> {
        let ctx = Context::default();
        let mut triggered = None;

        let _ = ctx.run(
            RawInput {
                events,
                ..RawInput::default()
            },
            |ctx| {
                triggered = Some(manager.dispatch_raw(ctx));
            },
        );

        triggered.expect("dispatch should run exactly once")
    }

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
    fn shortcut_command_sets_logical_command_modifier() {
        let sc = shortcut("Cmd+S");
        assert_eq!(sc.key, Key::S);
        assert!(sc.mods.command);
        assert!(!sc.mods.mac_cmd);
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

    #[test]
    fn non_consuming_scope_still_allows_global_fallback() {
        let global = Arc::new(RwLock::new(shortcut_map!["Ctrl+S" => 1u32]));
        let mut manager = ShortcutManager::new(global);
        manager.push_scope(ShortcutScope::new(
            "editor",
            shortcut_map!["Ctrl+S" => 2u32],
            false,
        ));

        let triggered = dispatch_raw_events(&manager, vec![key_event(Key::S, Modifiers::CTRL)]);
        assert_eq!(triggered, vec![2, 1]);
    }

    #[test]
    fn consuming_scope_blocks_global_fallback() {
        let global = Arc::new(RwLock::new(shortcut_map!["Ctrl+S" => 1u32]));
        let mut manager = ShortcutManager::new(global);
        manager.push_scope(ShortcutScope::new(
            "editor",
            shortcut_map!["Ctrl+S" => 2u32],
            true,
        ));

        let triggered = dispatch_raw_events(&manager, vec![key_event(Key::S, Modifiers::CTRL)]);
        assert_eq!(triggered, vec![2]);
    }

    #[test]
    fn logical_command_shortcut_matches_command_input() {
        let global = Arc::new(RwLock::new(shortcut_map!["Cmd+S" => 7u32]));
        let manager = ShortcutManager::new(global);

        let triggered = dispatch_raw_events(&manager, vec![key_event(Key::S, Modifiers::COMMAND)]);
        assert_eq!(triggered, vec![7]);
    }

    #[test]
    fn more_specific_shortcut_wins_with_logical_matching() {
        let global = Arc::new(RwLock::new(shortcut_map![
            "Ctrl+S" => 1u32,
            "Ctrl+Shift+S" => 2u32,
        ]));
        let manager = ShortcutManager::new(global);

        let triggered = dispatch_raw_events(
            &manager,
            vec![key_event(Key::S, Modifiers::CTRL | Modifiers::SHIFT)],
        );
        assert_eq!(triggered, vec![2]);
    }

    #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
    enum TestCmd { Save, Help, Quit }

    impl From<TestCmd> for egui_command::CommandId {
        fn from(c: TestCmd) -> Self { egui_command::CommandId::new(c) }
    }

    #[test]
    fn fill_shortcut_hints_writes_to_registered_commands() {
        let global = Arc::new(RwLock::new(shortcut_map![
            "Ctrl+S" => TestCmd::Save,
            "F1" => TestCmd::Help,
        ]));
        let manager = ShortcutManager::new(global);

        let mut reg = egui_command::CommandRegistry::new()
            .with(TestCmd::Save, egui_command::CommandSpec::new(TestCmd::Save.into(), "Save"))
            .with(TestCmd::Help, egui_command::CommandSpec::new(TestCmd::Help.into(), "Help"))
            .with(TestCmd::Quit, egui_command::CommandSpec::new(TestCmd::Quit.into(), "Quit"));

        manager.fill_shortcut_hints(&mut reg);

        let save_hint = reg.spec(TestCmd::Save).unwrap().shortcut_hint.as_deref();
        let help_hint = reg.spec(TestCmd::Help).unwrap().shortcut_hint.as_deref();
        let quit_hint = reg.spec(TestCmd::Quit).unwrap().shortcut_hint.as_deref();

        assert!(save_hint.is_some(), "Save should have a shortcut hint");
        assert!(save_hint.unwrap().contains("S"), "Save hint should mention S key");
        assert!(help_hint.is_some(), "Help should have a shortcut hint");
        assert!(help_hint.unwrap().contains("F1"), "Help hint should contain F1");
        assert!(quit_hint.is_none(), "Quit has no binding, hint should be None");
    }

    #[test]
    fn fill_shortcut_hints_unregistered_command_is_skipped() {
        let global = Arc::new(RwLock::new(shortcut_map!["F9" => TestCmd::Quit]));
        let manager = ShortcutManager::new(global);

        let mut reg = egui_command::CommandRegistry::new()
            .with(TestCmd::Save, egui_command::CommandSpec::new(TestCmd::Save.into(), "Save"));

        manager.fill_shortcut_hints(&mut reg);

        assert!(reg.spec(TestCmd::Save).unwrap().shortcut_hint.is_none());
    }
}
