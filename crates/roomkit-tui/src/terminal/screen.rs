//! Terminal ownership: raw mode, the alternate screen, drawing and restoration,
//! including when the terminal thread panics.

use std::io::{self, Write};
use std::sync::Arc;
use std::thread::{self, ThreadId};

use crossterm::{cursor, event, execute, queue, style, terminal};

use crate::render::{self, Tone};
use crate::{ColorMode, SelectionMode, UiOptions};

type PanicHook = dyn Fn(&std::panic::PanicHookInfo<'_>) + Sync + Send + 'static;

pub(super) struct Terminal {
    pub stdout: io::Stdout,
    pub options: UiOptions,
    pub status: Option<crate::status::Reporter>,
    previous: Option<render::Frame>,
    clear: bool,
    colors: bool,
    previous_colors_disabled: bool,
    previous_hook: Option<Arc<PanicHook>>,
}

impl Terminal {
    pub fn enter(mut options: UiOptions) -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        let colors = colors_enabled(options.color);
        // The renderer stays pure: it reads the resolved policy, not the
        // environment, to draw glyphs where colors would tell parts apart.
        options.color = if colors {
            ColorMode::Always
        } else {
            ColorMode::Never
        };
        // Crossterm has its own memoized NO_COLOR filter. Align it with the
        // host's explicit policy while we own this terminal, then restore it.
        let previous_colors_disabled = style::Colored::ansi_color_disabled_memoized();
        style::force_color_output(colors);
        let mouse = options.selection == SelectionMode::CopyOnSelect;
        let mut terminal = Self {
            status: options
                .terminal_status
                .clone()
                .filter(|_| std::env::var_os("TERM").is_none_or(|value| value != "dumb"))
                .map(crate::status::Reporter::new),
            stdout: io::stdout(),
            previous: None,
            clear: false,
            options,
            colors,
            previous_colors_disabled,
            previous_hook: Some(install_panic_hook(mouse)),
        };
        execute!(
            terminal.stdout,
            terminal::EnterAlternateScreen,
            event::EnableBracketedPaste,
            cursor::Hide
        )?;
        if mouse {
            execute!(terminal.stdout, event::EnableMouseCapture)?;
        }
        Ok(terminal)
    }

    pub fn acknowledge_status(&mut self) -> io::Result<()> {
        if let Some(status) = &mut self.status {
            status.acknowledge(&mut self.stdout)?;
        }
        Ok(())
    }

    /// Repaint everything on the next draw, clearing what a resize left behind.
    pub fn invalidate(&mut self) {
        self.previous = None;
        self.clear = true;
    }

    pub fn draw(&mut self, frame: &render::Frame) -> io::Result<()> {
        if self.previous.as_ref() == Some(frame) {
            return Ok(());
        }
        queue!(self.stdout, cursor::Hide)?;
        if self.clear
            || self
                .previous
                .as_ref()
                .is_some_and(|old| old.lines.len() > frame.lines.len())
        {
            queue!(self.stdout, terminal::Clear(terminal::ClearType::All))?;
            self.previous = None;
            self.clear = false;
        }
        for (row, spans) in frame.spans.iter().enumerate() {
            if self.previous.as_ref().and_then(|old| old.spans.get(row)) == Some(spans) {
                continue;
            }
            queue!(
                self.stdout,
                cursor::MoveTo(0, u16::try_from(row).unwrap_or(u16::MAX)),
                terminal::Clear(terminal::ClearType::CurrentLine)
            )?;
            for span in spans {
                self.span(span)?;
            }
        }
        if let Some((col, row)) = frame.cursor {
            queue!(self.stdout, cursor::MoveTo(col, row), cursor::Show)?;
        }
        self.stdout.flush()?;
        self.previous = Some(frame.clone());
        Ok(())
    }

    fn span(&mut self, span: &render::Span) -> io::Result<()> {
        let mut appearance = appearance(span.tone, &self.options.theme, self.colors);
        for (enabled, attribute) in [
            (span.emphasis.bold, style::Attribute::Bold),
            (span.emphasis.italic, style::Attribute::Italic),
            (span.emphasis.underline, style::Attribute::Underlined),
            (span.emphasis.strike, style::Attribute::CrossedOut),
        ] {
            if enabled {
                appearance.attributes.set(attribute);
            }
        }
        queue!(
            self.stdout,
            style::SetStyle(appearance),
            style::Print(&span.text),
            style::SetAttribute(style::Attribute::Reset)
        )
    }
}

/// The application's explicit color policy, or else the environment's:
/// a nonempty `NO_COLOR` or `TERM=dumb` disables colors.
fn colors_enabled(mode: ColorMode) -> bool {
    match mode {
        ColorMode::Always => true,
        ColorMode::Never => false,
        _ => {
            std::env::var_os("NO_COLOR").is_none_or(|value| value.is_empty())
                && std::env::var_os("TERM").is_none_or(|value| value != "dumb")
        }
    }
}

fn appearance(tone: Tone, theme: &crate::Theme, colors: bool) -> style::ContentStyle {
    let mut result = style::ContentStyle::default();
    if colors {
        result.foreground_color = Some(foreground(tone, theme).to_crossterm());
        result.background_color = Some(match tone {
            Tone::Selected => theme.selection.to_crossterm(),
            Tone::Input | Tone::InputPrompt | Tone::Placeholder => theme.surface.to_crossterm(),
            _ => style::Color::Reset,
        });
    }
    if matches!(tone, Tone::Primary | Tone::Selected) {
        result.attributes.set(style::Attribute::Bold);
    }
    if !colors
        && matches!(
            tone,
            Tone::Selected | Tone::Input | Tone::InputPrompt | Tone::Placeholder
        )
    {
        result.attributes.set(style::Attribute::Reverse);
    }
    result
}

/// The theme's foreground color for a semantic tone.
fn foreground(tone: Tone, theme: &crate::Theme) -> crate::Color {
    match tone {
        Tone::Primary => theme.primary,
        Tone::Accent | Tone::InputPrompt => theme.accent,
        Tone::Muted | Tone::Placeholder => theme.muted,
        Tone::Border => theme.border,
        Tone::Success => theme.success,
        Tone::Warning => theme.warning,
        Tone::Error => theme.error,
        Tone::Selected => theme.selection_text,
        Tone::Input => theme.input_text,
        Tone::Heading => theme.heading,
        Tone::Code => theme.code,
        Tone::Link => theme.link,
        Tone::Keyword => theme.keyword,
        Tone::String => theme.string,
        Tone::Number => theme.number,
        Tone::Comment => theme.comment,
        Tone::Series(slot) => theme.series[usize::from(slot) % theme.series.len()],
        _ => theme.text,
    }
}

/// Leave raw mode and the alternate screen before the panic message is
/// printed, so that it stays readable. Panics on other threads, including
/// ones the application catches, leave the terminal alone.
fn install_panic_hook(mouse: bool) -> Arc<PanicHook> {
    let previous: Arc<PanicHook> = Arc::from(std::panic::take_hook());
    let chained = previous.clone();
    let owner: ThreadId = thread::current().id();
    std::panic::set_hook(Box::new(move |info| {
        if thread::current().id() == owner {
            restore(&mut io::stdout(), mouse);
        }
        chained(info);
    }));
    previous
}

fn restore(stdout: &mut io::Stdout, mouse: bool) {
    if mouse {
        let _ = execute!(stdout, event::DisableMouseCapture);
    }
    let _ = execute!(
        stdout,
        style::ResetColor,
        cursor::Show,
        event::DisableBracketedPaste,
        terminal::LeaveAlternateScreen
    );
    let _ = terminal::disable_raw_mode();
}

impl Drop for Terminal {
    fn drop(&mut self) {
        if let Some(status) = &mut self.status {
            let _ = status.clear(&mut self.stdout);
        }
        restore(
            &mut self.stdout,
            self.options.selection == SelectionMode::CopyOnSelect,
        );
        style::Colored::set_ansi_color_disabled(self.previous_colors_disabled);
        // Replacing a hook while unwinding would abort; the installed hook
        // already restored the terminal and still delegates to the previous one.
        if !thread::panicking()
            && let Some(previous) = self.previous_hook.take()
        {
            let _ = std::panic::take_hook();
            std::panic::set_hook(Box::new(move |info| previous(info)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meter_slots_take_the_theme_series_and_vanish_without_colors() {
        let theme = crate::Theme::dark();
        let colored = appearance(Tone::Series(9), &theme, true);
        assert_eq!(
            colored.foreground_color,
            Some(theme.series[1].to_crossterm()),
            "Slot 9 wraps to slot 1"
        );
        for slot in 0..=u8::MAX {
            let plain = appearance(Tone::Series(slot), &theme, false);
            assert_eq!(
                (plain.foreground_color, plain.background_color),
                (None, None)
            );
        }
        assert!(!colors_enabled(ColorMode::Never) && colors_enabled(ColorMode::Always));
    }
}
