# imekit

A cross-platform Rust library for implementing Input Method Engines (IME) using native protocols.

## Overview

imekit provides native protocol implementations for creating input methods:

- **Linux/Wayland**: `zwp_input_method_v2` and `zwp_text_input_v3` protocols
- **Linux/X11**: XIM (X Input Method) protocol
- **Linux/IBus**: Registered IBus engine via D-Bus (fallback when Wayland protocol unavailable)
- **Windows**: Text Services Framework (TSF) via `ITfInsertAtSelection` / `ITfContextComposition`
- **macOS**: Input Method Kit (IMK) — no Accessibility permissions required

This is **not** a text insertion library. It implements the actual IME protocols that allow you to:
- Register as an input method with the system
- Receive text input context (surrounding text, content type)
- Commit text and preedit (composing) strings
- Grab keyboard input and forward/consume key events
- Create popup surfaces for candidate windows (Wayland)

## Features

| Feature | Description |
|---------|-------------|
| `log` | Enable logging via the `log` crate |
| `tracing` | Enable logging via the `tracing` crate |
| `ibus` | Enable IBus engine support for Linux (via `zbus`) |
| `async` | Enable `futures::Stream` API backed by `async-io` |

## Platform Support

| Platform | Protocol | Status |
|----------|----------|--------|
| Linux/Wayland | `zwp_input_method_v2` | ✅ Full support |
| Linux/Wayland | `zwp_text_input_v3` | ✅ Full support |
| Linux/X11 | XIM + XTest | ✅ Full support |
| Linux/IBus | D-Bus engine | ✅ Full support (with `ibus` feature) |
| Windows | TSF (`ITfInsertAtSelection` / `ITfContextComposition`) | ✅ Full support |
| macOS | Input Method Kit (IMK) | ✅ Full support |

### Wayland Compositor Support

| Compositor | Support |
|------------|---------|
| sway | ✅ Full support |
| Hyprland | ✅ Full support |
| KDE Plasma | ✅ Good support |
| GNOME/Mutter | ⚠️ Uses IBus engine (enable `ibus` feature) |
| wlroots-based | ✅ Generally supported |

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
imekit = "0.1"

# With logging
imekit = { version = "0.1", features = ["log"] }

# With IBus fallback for GNOME/Mutter
imekit = { version = "0.1", features = ["ibus"] }

# With async Stream API
imekit = { version = "0.1", features = ["async"] }
```

### Basic Input Method

```rust
use imekit::{InputMethod, InputMethodEvent};

fn main() -> Result<(), imekit::Error> {
    // Auto-detects Wayland → X11 on Linux.
    // Falls back to IBus if zwp_input_method_v2 unavailable (needs `ibus` feature).
    let mut im = InputMethod::new()?;

    loop {
        while let Some(event) = im.next_event() {
            match event {
                InputMethodEvent::Activate { serial } => {
                    im.commit_string("Hello!")?;
                    im.commit(serial)?;
                }
                InputMethodEvent::SurroundingText { text, cursor, anchor } => {
                    println!("Context: {} (cursor: {})", text, cursor);
                }
                InputMethodEvent::Done => {
                    // All pending state for this frame has arrived.
                    let state = im.state();
                    println!("Active: {}", state.active);
                }
                InputMethodEvent::Unavailable => return Ok(()),
                _ => {}
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}
```

### Preedit (Composing Text)

```rust
use imekit::{InputMethod, InputMethodEvent};

fn main() -> Result<(), imekit::Error> {
    let mut im = InputMethod::new()?;

    loop {
        while let Some(event) = im.next_event() {
            if let InputMethodEvent::Activate { serial } = event {
                // Show "にほん" as composing text, cursor at end.
                im.set_preedit_string("にほん", 0, 9)?;
                im.commit(serial)?;

                // Later: commit the final text.
                im.commit_string("日本")?;
                im.commit(serial)?;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}
```

### Keyboard Grab (Wayland-only)

After grabbing, all key events are delivered via `InputMethodEvent::KeyEvent`. Decide
whether to consume each key or forward it back to the application.

```rust
use imekit::{InputMethodEvent, KeyState};
use imekit::wayland_impl::WaylandInputMethod;

fn main() -> Result<(), imekit::Error> {
    let mut im = WaylandInputMethod::new()?;
    im.grab_keyboard()?;

    loop {
        while let Some(event) = im.next_event() {
            match event {
                InputMethodEvent::KeyEvent { keycode, keysym, state, modifiers } => {
                    if state == KeyState::Pressed {
                        // Consume the key (don't forward to app) and commit text instead.
                        im.commit_string("x")?;
                        // Or forward the key unchanged:
                        // im.forward_key(keycode, state)?;
                    }
                }
                InputMethodEvent::RepeatInfo { rate, delay } => {
                    println!("Repeat: {}cps after {}ms", rate, delay);
                }
                _ => {}
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}
```

### Status and Availability

```rust
use imekit::{InputMethod, Status};

fn main() -> Result<(), imekit::Error> {
    let mut im = InputMethod::new()?;

    loop {
        im.next_event(); // drive the event loop

        match im.status() {
            Status::Active    => { /* text field focused */ }
            Status::Inactive  => { /* no text field focused */ }
            Status::Unavailable => {
                eprintln!("Compositor withdrew the input method.");
                break;
            }
        }
    }
    Ok(())
}
```

### Cross-Thread Commit (Wayland)

`InputMethodHandle` is `Clone + Send`. Use it to commit from background threads
while the main thread drives the event loop.

```rust
use imekit::wayland_impl::WaylandInputMethod;
use std::thread;

fn main() -> Result<(), imekit::Error> {
    let mut im = WaylandInputMethod::new()?;
    let handle = im.handle(); // clone-able, Send

    thread::spawn(move || {
        // Commit from a worker thread.
        handle.commit_string("from another thread").unwrap();
        handle.commit(0).unwrap();
    });

    loop {
        im.next_event();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}
```

### Async / Stream API (requires `async` feature)

```rust
use futures::StreamExt;
use imekit::{InputMethodEvent, wayland_impl::WaylandInputMethod};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let im = WaylandInputMethod::new()?;
    let mut stream = im.into_stream()?;

    while let Some(event) = stream.next().await {
        if let InputMethodEvent::Activate { serial } = event {
            // handle...
        }
    }
    Ok(())
}
```

### Explicit Backend Selection (Linux)

```rust
use imekit::InputMethod;

// Auto-detect (recommended)
let im = InputMethod::new()?;

// Explicit backends
let im = InputMethod::wayland()?;
let im = InputMethod::x11()?;

// Specific Wayland seat (multi-seat setups)
let im = InputMethod::wayland_for_seat("seat-1")?;

// IBus (requires `ibus` feature)
#[cfg(feature = "ibus")]
let im = InputMethod::ibus()?;
```

### Text Input Client (receiving IME input)

For apps that *receive* text from an IME rather than *acting as* an IME:

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

### Surrounding Text and Delete

```rust
use imekit::{InputMethod, InputMethodEvent};

fn main() -> Result<(), imekit::Error> {
    let mut im = InputMethod::new()?;

    loop {
        while let Some(event) = im.next_event() {
            match event {
                InputMethodEvent::SurroundingText { text, cursor, .. } => {
                    // Delete 1 char before cursor, 0 after.
                    im.delete_surrounding_text(1, 0)?;
                    im.commit_string("replacement")?;
                    im.commit(im.state().serial)?;
                }
                _ => {}
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}
```

## Requirements

### Linux/Wayland
- Compositor with `zwp_input_method_v2` support

### Linux/X11
- X11 display server with XTest extension

### Linux/IBus (with `ibus` feature)
- IBus daemon running
- D-Bus session bus available

### Windows
- Windows 8+ (TSF is built into the OS; no extra installation needed)
- Apps using the `SendInput` fallback require no special setup

### macOS
- macOS 10.5+ — no Accessibility permissions required (uses Input Method Kit)

## License

Licensed under either of:
- Apache License, Version 2.0
- MIT license
