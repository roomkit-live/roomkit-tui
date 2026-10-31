//! Terminal colors and the semantic palettes built from them.

/// A terminal color. The terminal maps named colors to its own palette;
/// `Rgb` needs true-color support and `Ansi` selects one of 256 entries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
#[allow(missing_docs, reason = "Standard terminal color names")]
pub enum Color {
    /// The terminal's default foreground or background.
    Reset,
    Black,
    DarkGrey,
    Red,
    DarkRed,
    Green,
    DarkGreen,
    Yellow,
    DarkYellow,
    Blue,
    DarkBlue,
    Magenta,
    DarkMagenta,
    Cyan,
    DarkCyan,
    White,
    Grey,
    /// A true color.
    Rgb {
        r: u8,
        g: u8,
        b: u8,
    },
    /// An entry of the 256-color palette.
    Ansi(u8),
}

impl Color {
    /// A true color.
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self::Rgb { r, g, b }
    }

    pub(crate) fn to_crossterm(self) -> crossterm::style::Color {
        use crossterm::style::Color as C;
        match self {
            Self::Black => C::Black,
            Self::DarkGrey => C::DarkGrey,
            Self::Red => C::Red,
            Self::DarkRed => C::DarkRed,
            Self::Green => C::Green,
            Self::DarkGreen => C::DarkGreen,
            Self::Yellow => C::Yellow,
            Self::DarkYellow => C::DarkYellow,
            Self::Blue => C::Blue,
            Self::DarkBlue => C::DarkBlue,
            Self::Magenta => C::Magenta,
            Self::DarkMagenta => C::DarkMagenta,
            Self::Cyan => C::Cyan,
            Self::DarkCyan => C::DarkCyan,
            Self::White => C::White,
            Self::Grey => C::Grey,
            Self::Rgb { r, g, b } => C::Rgb { r, g, b },
            Self::Ansi(value) => C::AnsiValue(value),
            Self::Reset => C::Reset,
        }
    }
}

/// Colors describe presentation roles, never domain tools or providers.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Theme {
    /// Normal conversation text; reset inherits the terminal foreground.
    pub text: Color,
    /// Brand, agent headings and focused borders.
    pub primary: Color,
    /// Secondary brand and activity accents.
    pub accent: Color,
    /// Secondary labels and keyboard hints.
    pub muted: Color,
    /// Unfocused outlines.
    pub border: Color,
    /// Composer background; reset inherits the terminal background.
    pub surface: Color,
    /// Composer foreground, paired with its filled surface for contrast.
    pub input_text: Color,
    /// Selected menu text.
    pub selection_text: Color,
    /// Selected menu background.
    pub selection: Color,
    /// Confirmed successful completion.
    pub success: Color,
    /// Pending decisions and refusals.
    pub warning: Color,
    /// Confirmed execution failures.
    pub error: Color,
    /// Markdown heading color.
    pub heading: Color,
    /// Literal code and file paths.
    pub code: Color,
    /// Link labels and visible destinations.
    pub link: Color,
    /// Syntax keywords.
    pub keyword: Color,
    /// Syntax string literals.
    pub string: Color,
    /// Syntax numeric literals.
    pub number: Color,
    /// Syntax comments.
    pub comment: Color,
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}

impl Theme {
    /// RoomKit indigo/cyan accents for a dark terminal, without changing its canvas.
    pub fn dark() -> Self {
        Self {
            heading: Color::rgb(165, 180, 252),
            code: Color::rgb(103, 232, 249),
            link: Color::rgb(96, 165, 250),
            keyword: Color::rgb(192, 132, 252),
            string: Color::rgb(134, 239, 172),
            number: Color::rgb(253, 186, 116),
            comment: Color::rgb(148, 163, 184),
            text: Color::Reset,
            primary: Color::rgb(129, 140, 248),
            accent: Color::rgb(34, 211, 238),
            muted: Color::rgb(148, 163, 184),
            border: Color::rgb(71, 85, 105),
            surface: Color::rgb(30, 41, 59),
            input_text: Color::rgb(226, 232, 240),
            selection_text: Color::White,
            selection: Color::rgb(55, 48, 107),
            success: Color::rgb(74, 222, 128),
            warning: Color::rgb(251, 191, 36),
            error: Color::rgb(251, 113, 133),
        }
    }

    /// Higher-contrast accents for a light terminal.
    pub fn light() -> Self {
        Self {
            surface: Color::rgb(226, 232, 240),
            input_text: Color::rgb(30, 41, 59),
            heading: Color::rgb(67, 56, 202),
            code: Color::rgb(14, 116, 144),
            link: Color::rgb(29, 78, 216),
            keyword: Color::rgb(126, 34, 206),
            string: Color::rgb(21, 128, 61),
            number: Color::rgb(154, 52, 18),
            comment: Color::rgb(100, 116, 139),
            primary: Color::rgb(79, 70, 229),
            accent: Color::rgb(14, 116, 144),
            muted: Color::rgb(71, 85, 105),
            border: Color::rgb(148, 163, 184),
            selection: Color::rgb(79, 70, 229),
            success: Color::rgb(21, 128, 61),
            warning: Color::rgb(146, 64, 14),
            error: Color::rgb(190, 18, 60),
            ..Self::dark()
        }
    }

    /// Use the terminal's own ANSI palette, including its default background.
    pub fn terminal() -> Self {
        Self {
            heading: Color::Blue,
            code: Color::Cyan,
            link: Color::Blue,
            keyword: Color::Magenta,
            string: Color::Green,
            number: Color::Yellow,
            comment: Color::DarkGrey,
            text: Color::Reset,
            primary: Color::Blue,
            accent: Color::Cyan,
            muted: Color::Reset,
            border: Color::DarkGrey,
            surface: Color::DarkGrey,
            input_text: Color::White,
            selection_text: Color::Black,
            selection: Color::Grey,
            success: Color::Green,
            warning: Color::Yellow,
            error: Color::Red,
        }
    }
}
