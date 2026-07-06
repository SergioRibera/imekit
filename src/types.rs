//! Common types for IME operations

/// Events from the input method system
#[derive(Debug, Clone)]
pub enum InputMethodEvent {
    /// IME has been activated
    Activate {
        /// Serial number for this activation
        serial: u32,
    },
    /// IME has been deactivated
    Deactivate,
    /// Surrounding text context received
    SurroundingText {
        /// The text around the cursor
        text: String,
        /// Cursor position in bytes
        cursor: u32,
        /// Anchor position in bytes (for selection)
        anchor: u32,
    },
    /// Text change cause
    TextChangeCause(ChangeCause),
    /// Content type hint
    ContentType {
        /// Hint about the content type
        hint: ContentHint,
        /// Purpose of the text field
        purpose: ContentPurpose,
    },
    /// Done event - all pending state has been sent
    Done,
    /// Request to show popup at given position
    PopupSurfaceCreated {
        /// X position relative to cursor
        x: i32,
        /// Y position relative to cursor  
        y: i32,
        /// Width of the text cursor area
        width: i32,
        /// Height of the text cursor area
        height: i32,
    },
    /// Unavailable - compositor doesn't support protocol
    Unavailable,
}

/// Cause of a text change
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChangeCause {
    /// Change caused by input method
    InputMethod,
    /// Change caused by something else (user, application)
    #[default]
    Other,
}

/// Hints about the content type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ContentHint {
    /// Completion should be shown
    pub completion: bool,
    /// Spellcheck should be performed
    pub spellcheck: bool,
    /// Auto-capitalization should be performed
    pub auto_capitalization: bool,
    /// Input is lowercase
    pub lowercase: bool,
    /// Input is uppercase  
    pub uppercase: bool,
    /// Title case
    pub titlecase: bool,
    /// Hidden text (password)
    pub hidden_text: bool,
    /// Sensitive data
    pub sensitive_data: bool,
    /// Latin characters
    pub latin: bool,
    /// Multiline text
    pub multiline: bool,
}

/// Purpose of the text input
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ContentPurpose {
    /// Normal text
    #[default]
    Normal,
    /// Alphabetic characters
    Alpha,
    /// Digits
    Digits,
    /// Number (including decimal)
    Number,
    /// Phone number
    Phone,
    /// URL
    Url,
    /// Email address
    Email,
    /// Person name
    Name,
    /// Password
    Password,
    /// PIN
    Pin,
    /// Date
    Date,
    /// Time
    Time,
    /// Date and time
    Datetime,
    /// Terminal/command line
    Terminal,
}

/// Preedit style for composing text
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PreeditStyle {
    /// Default styling
    #[default]
    Default,
    /// No styling
    None,
    /// Active/focused region
    Active,
    /// Inactive region
    Inactive,
    /// Highlighted region
    Highlight,
    /// Underlined region
    Underline,
    /// Selection region
    Selection,
    /// Incorrect/error region
    Incorrect,
}

/// Status of the input method
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Input method is active (focused on a text field)
    Active,
    /// Input method is inactive (no text field focused)
    Inactive,
    /// Compositor withdrew the input method; cannot be used until reconnected
    Unavailable,
}

/// Rectangle for cursor/anchor positioning
#[derive(Debug, Clone, Copy, Default)]
pub struct CursorRect {
    /// X position
    pub x: i32,
    /// Y position
    pub y: i32,
    /// Width
    pub width: i32,
    /// Height
    pub height: i32,
}

/// State of the input method (platform-agnostic)
#[derive(Debug, Default, Clone)]
pub struct InputMethodState {
    /// Whether the input method is active
    pub active: bool,
    /// Current serial number
    pub serial: u32,
    /// Surrounding text
    pub surrounding_text: Option<String>,
    /// Cursor position in surrounding text
    pub cursor: u32,
    /// Anchor position in surrounding text
    pub anchor: u32,
    /// Content hint
    pub content_hint: ContentHint,
    /// Content purpose
    pub content_purpose: ContentPurpose,
    /// Change cause
    pub change_cause: ChangeCause,
    /// Pending preedit text
    pub preedit_text: Option<String>,
    /// Pending preedit cursor position
    pub preedit_cursor: i32,
    /// Pending commit text
    pub commit_text: Option<String>,
    /// Delete surrounding text before cursor
    pub delete_before: u32,
    /// Delete surrounding text after cursor
    pub delete_after: u32,
}

impl InputMethodState {
    /// Create a new input method state
    pub fn new() -> Self {
        Self::default()
    }

    /// Reset the state
    pub fn reset(&mut self) {
        self.active = false;
        self.surrounding_text = None;
        self.cursor = 0;
        self.anchor = 0;
        self.preedit_text = None;
        self.preedit_cursor = 0;
        self.commit_text = None;
        self.delete_before = 0;
        self.delete_after = 0;
    }

    /// Clear pending changes
    pub fn clear_pending(&mut self) {
        self.preedit_text = None;
        self.preedit_cursor = 0;
        self.commit_text = None;
        self.delete_before = 0;
        self.delete_after = 0;
    }
}
