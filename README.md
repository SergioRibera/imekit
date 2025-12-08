# imekit

A cross-platform Rust library for implementing Input Method Engines (IME) using native protocols.

## Overview

imekit provides native protocol implementations for creating input methods:

- **Linux/Wayland**: `zwp_input_method_v2` and `zwp_text_input_v3` protocols
- **Linux/X11**: XIM (X Input Method) protocol
- **Linux/IBus**: IBus D-Bus interface (fallback when Wayland protocol unavailable)
- **Windows**: Text Services Framework (TSF)
- **macOS**: Input Method Kit (IMK)

This is **not** a text insertion library. It implements the actual IME protocols that allow you to:
- Register as an input method with the system
- Receive text input context (surrounding text, content type)
- Commit text and preedit strings
- Create popup surfaces for candidate windows

## Features

| Feature | Description |
|---------|-------------|
| `log` | Enable logging via the `log` crate |
| `tracing` | Enable logging via the `tracing` crate |
| `ibus` | Enable IBus support for Linux (via zbus D-Bus interface) |

## Platform Support

| Platform | Protocol | Status |
|----------|----------|--------|
| Linux/Wayland | `zwp_input_method_v2` | ✅ Full support |
| Linux/Wayland | `zwp_text_input_v3` | ✅ Full support |
| Linux/X11 | XIM | ✅ Full support |
| Linux/IBus | D-Bus | ✅ Full support (with `ibus` feature) |
| Windows | TSF (Text Services Framework) | ✅ Full support |
| macOS | CGEvent/NSTextInputClient | ✅ Full support |

### Wayland Compositor Support

| Compositor | Support |
|------------|---------|
| sway | ✅ Full support |
| Hyprland | ✅ Full support |
| KDE Plasma | ✅ Good support |
| GNOME/Mutter | ⚠️ Uses IBus (enable `ibus` feature) |
| wlroots-based | ✅ Generally supported |

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
imekit = "0.1"

# Optional: Enable logging
imekit = { version = "0.1", features = ["log"] }

# Optional: Enable IBus fallback for GNOME/Mutter
imekit = { version = "0.1", features = ["ibus"] }
```

### Basic Input Method

```rust
use imekit::{InputMethod, InputMethodEvent};

fn main() -> Result<(), imekit::Error> {
    // Auto-detects Wayland vs X11 on Linux
    // Falls back to IBus if Wayland protocol unavailable (with `ibus` feature)
    let mut im = InputMethod::new()?;

    loop {
        while let Some(event) = im.next_event() {
            match event {
                InputMethodEvent::Activate { serial } => {
                    println!("IME activated");
                    // Commit text when activated
                    im.commit_string("Hello! 👋")?;
                    im.commit(serial)?;
                }
                InputMethodEvent::SurroundingText { text, cursor, anchor } => {
                    println!("Context: {} (cursor: {})", text, cursor);
                }
                InputMethodEvent::Deactivate => {
                    println!("IME deactivated");
                }
                _ => {}
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}
```

### Text Input Client (receiving IME input)

```rust
use imekit::wayland_impl::{TextInput, TextInputEvent};

fn main() -> Result<(), imekit::Error> {
    let mut ti = TextInput::new()?;
    ti.enable();
    ti.commit();

    loop {
        while let Some(event) = ti.next_event() {
            match event {
                TextInputEvent::CommitString { text } => {
                    if let Some(text) = text {
                        println!("Received: {}", text);
                    }
                }
                TextInputEvent::PreeditString { text, .. } => {
                    if let Some(text) = text {
                        println!("Composing: {}", text);
                    }
                }
                _ => {}
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}
```

### Using IBus Directly

```rust
#[cfg(feature = "ibus")]
use imekit::ibus_impl::IBusInputMethod;

fn main() -> Result<(), imekit::Error> {
    #[cfg(feature = "ibus")]
    {
        // Create IBus input method directly
        let im = IBusInputMethod::new()?;
        // Use im...
    }
    Ok(())
}
```

## Requirements

### Linux/Wayland
- Compositor with `zwp_input_method_v2` support
- `libwayland-dev` for Wayland client libraries

### Linux/X11
- X11 display server with XTest extension
- `libx11-dev` and `libxtst-dev` for X11 libraries

### Linux/IBus (with `ibus` feature)
- IBus daemon running
- D-Bus session bus available

### Windows
- Windows 8+ with TSF support

### macOS
- macOS 10.5+ with Accessibility permissions

## License

Licensed under either of:
- Apache License, Version 2.0
- MIT license
