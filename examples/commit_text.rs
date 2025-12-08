//! Text Commit Example
//!
//! Demonstrates using imekit to commit text when the IME is activated.
//! When you focus a text input field, this example will commit "Hello World! 👋"
//!
//! Then focus a text input in any application. The IME will automatically
//! commit text when activated.

use imekit::{InputMethod, InputMethodEvent};

fn main() -> Result<(), imekit::Error> {
    env_logger::init();

    println!("imekit Text Commit Example");
    println!("==========================");
    println!();

    // Create the input method (auto-detects Wayland vs X11)
    println!("Creating input method...");
    let mut im = InputMethod::new()?;

    #[cfg(target_os = "linux")]
    {
        if im.is_wayland() {
            println!("Using Wayland backend (zwp_input_method_v2)");
        } else if im.is_x11() {
            println!("Using X11 backend (XIM)");
        }
        #[cfg(feature = "ibus")]
        if im.is_ibus() {
            println!("Using IBus backend (D-Bus)");
        }
    }

    #[cfg(target_os = "windows")]
    println!("Using Windows backend (TSF/SendInput)");

    #[cfg(target_os = "macos")]
    println!("Using macOS backend (IMK/CGEvent)");

    println!("Input method created successfully!");
    println!();
    println!("Focus a text input field to commit text...");
    println!();

    // Text to commit when activated
    let text_to_commit = "Hello World! 👋";

    // Main event loop
    loop {
        while let Some(event) = im.next_event() {
            match event {
                InputMethodEvent::Activate { serial } => {
                    println!("IME ACTIVATED (serial: {})", serial);

                    // Commit the text
                    println!("Committing text: {}", text_to_commit);
                    if let Err(e) = im.commit_string(text_to_commit) {
                        eprintln!("Failed to commit string: {}", e);
                    }

                    // Finalize the commit
                    if let Err(e) = im.commit(serial) {
                        eprintln!("Failed to finalize commit: {}", e);
                    }

                    println!("Text committed successfully!");
                }
                InputMethodEvent::Deactivate => {
                    println!("IME DEACTIVATED");
                }
                InputMethodEvent::SurroundingText {
                    text,
                    cursor,
                    anchor,
                } => {
                    println!(
                        "Surrounding text: {:?} (cursor: {}, anchor: {})",
                        text, cursor, anchor
                    );
                }
                InputMethodEvent::Unavailable => {
                    println!("IME protocol unavailable!");
                    return Ok(());
                }
                _ => {}
            }
        }

        // Small sleep to avoid busy-waiting
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}
