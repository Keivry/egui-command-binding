# egui-command-binding

[![Crates.io](https://img.shields.io/crates/v/egui-command-binding.svg)](https://crates.io/crates/egui-command-binding)
[![Docs.rs](https://docs.rs/egui-command-binding/badge.svg)](https://docs.rs/egui-command-binding)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](https://github.com/Keivry/egui-command-binding)

`egui-command-binding` is the egui-specific keyboard shortcut dispatch layer for [`egui-command`](https://crates.io/crates/egui-command). 

It scans `egui::Context` key events each frame, matches them against defined shortcut maps, and returns triggered commands. While `egui-command` provides the pure, UI-agnostic command model, this crate adds the necessary input handling to trigger those commands via keyboard in an egui application.

## Quick Start

Define your commands, set up a global shortcut map, and call `dispatch` in your frame update loop.

```rust
use egui_command::CommandId;
use egui_command_binding::{ShortcutManager, shortcut_map};
use parking_lot::RwLock;
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum AppCmd {
    Save,
    ShowHelp,
}

impl From<AppCmd> for CommandId {
    fn from(cmd: AppCmd) -> Self {
        CommandId::new(cmd)
    }
}

// 1. Define global shortcuts
let global_map = Arc::new(RwLock::new(shortcut_map![
    "Ctrl+S" => AppCmd::Save,
    "F1" => AppCmd::ShowHelp,
]));

// 2. Initialize the manager
let manager = ShortcutManager::new(global_map);

// 3. Inside your eframe update loop:
// fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
//     let triggered = self.manager.dispatch(ctx);
//     for cmd in triggered {
//         // cmd is CommandTriggered { id: CommandId, source: CommandSource::Keyboard }
//         handle(cmd);
//     }
// }
```

## Dispatch Order and Scopes

You can push context-sensitive shortcut scopes onto the manager. This allows temporary UI states (like modal dialogs or specific panels) to override global shortcuts.

```text
extra scope (if provided, always consuming)
  |
  v
scoped stack (top to bottom)
  |-- non-consuming scope: match fires, propagation continues
  |-- consuming scope: match fires, propagation stops
  |
  v
global map (fallback)
```

Within any single scope or map, **specificity wins**. If both `"Ctrl+S"` and `"Ctrl+Shift+S"` match the current keys, the one with more modifiers fires. The crate uses `egui::Modifiers::matches_logically`, meaning `"Cmd+S"` correctly handles the command key on macOS and the control key on Windows/Linux.

### Using Scopes

```rust
use egui_command_binding::{ShortcutScope, shortcut_map};

manager.push_scope(ShortcutScope::new(
    "modal_dialog",
    shortcut_map!["Escape" => AppCmd::CloseModal],
    true, // consuming: stops "Escape" from reaching lower scopes
));

// ... later ...
manager.pop_scope();
```

## Text Input Safety

By default, the `dispatch`, `dispatch_raw`, and `dispatch_raw_with_extra` methods respect keyboard focus. They return empty results when `ctx.wants_keyboard_input()` is true. This prevents accidental command triggers when the user is typing in a text field.

If you need to bypass this check (for example, to use the Escape key to unfocus a text field), use `try_shortcut`:

```rust
use egui_command_binding::shortcut;

if let Some(cmd) = manager.try_shortcut(ctx, shortcut("Escape")) {
    // This fires even if a text field has focus
    handle_command(cmd);
}
```

## Synchronizing with CommandRegistry

If you use `CommandRegistry` from `egui-command` to manage command metadata (labels, descriptions), you can automatically populate the human-readable shortcut hints from your active bindings.

```rust
// Reads the current global map and writes string hints (like "Ctrl+S") 
// into the corresponding CommandSpec::shortcut_hint fields.
manager.fill_shortcut_hints(&mut my_command_registry);
```

Commands without a defined binding remain unchanged.

## License

Licensed under either of MIT License or Apache License, Version 2.0 at your option.
