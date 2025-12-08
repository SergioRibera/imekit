//! Error types for imekit-core

use std::fmt;

/// Result type alias for imekit-core operations
pub type Result<T> = std::result::Result<T, Error>;

/// Errors that can occur during IME operations
#[derive(Debug)]
pub enum Error {
    /// Failed to connect to the display server
    ConnectionFailed(String),

    /// The required protocol is not supported by the compositor/system
    ProtocolNotSupported(String),

    /// IME registration failed
    RegistrationFailed(String),

    /// IME is not active
    NotActive,

    /// Failed to create popup surface
    PopupCreationFailed(String),

    /// Text commit failed
    CommitFailed(String),

    /// Platform not supported
    PlatformNotSupported,

    /// Wayland specific error
    #[cfg(target_os = "linux")]
    Wayland(wayland_client::ConnectError),

    /// Wayland dispatch error
    #[cfg(target_os = "linux")]
    WaylandDispatch(wayland_client::DispatchError),

    /// IBus D-Bus error
    #[cfg(target_os = "linux")]
    IBus(String),

    /// Windows error
    #[cfg(target_os = "windows")]
    Windows(windows::core::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::ConnectionFailed(msg) => {
                write!(f, "Failed to connect to display server: {}", msg)
            }
            Error::ProtocolNotSupported(msg) => write!(f, "Protocol not supported: {}", msg),
            Error::RegistrationFailed(msg) => write!(f, "IME registration failed: {}", msg),
            Error::NotActive => write!(f, "IME is not active"),
            Error::PopupCreationFailed(msg) => write!(f, "Failed to create popup surface: {}", msg),
            Error::CommitFailed(msg) => write!(f, "Failed to commit text: {}", msg),
            Error::PlatformNotSupported => write!(f, "Platform not supported"),
            #[cfg(target_os = "linux")]
            Error::Wayland(e) => write!(f, "Wayland error: {}", e),
            #[cfg(target_os = "linux")]
            Error::WaylandDispatch(e) => write!(f, "Wayland dispatch error: {}", e),
            #[cfg(target_os = "linux")]
            Error::IBus(msg) => write!(f, "IBus error: {}", msg),
            #[cfg(target_os = "windows")]
            Error::Windows(e) => write!(f, "Windows error: {}", e),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            #[cfg(target_os = "linux")]
            Error::Wayland(e) => Some(e),
            #[cfg(target_os = "linux")]
            Error::WaylandDispatch(e) => Some(e),
            #[cfg(target_os = "windows")]
            Error::Windows(e) => Some(e),
            _ => None,
        }
    }
}

#[cfg(target_os = "linux")]
impl From<wayland_client::ConnectError> for Error {
    fn from(e: wayland_client::ConnectError) -> Self {
        Error::Wayland(e)
    }
}

#[cfg(target_os = "linux")]
impl From<wayland_client::DispatchError> for Error {
    fn from(e: wayland_client::DispatchError) -> Self {
        Error::WaylandDispatch(e)
    }
}

#[cfg(target_os = "windows")]
impl From<windows::core::Error> for Error {
    fn from(e: windows::core::Error) -> Self {
        Error::Windows(e)
    }
}
