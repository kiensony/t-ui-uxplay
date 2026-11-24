use crate::{
    settings::Dialog,
    state::Player,
    ui::{self, Screen},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture},
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use image::{DynamicImage, ImageEncoder, codecs::png::PngEncoder};
use ratatui::{Terminal, backend::CrosstermBackend, layout::Rect};
use std::{
    env,
    io::{self, IsTerminal, Stdout, Write},
    time::Instant,
};

pub struct PlayerTerminal {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    graphics: Graphics,
    _mode: TerminalMode,
}

impl PlayerTerminal {
    pub fn open() -> io::Result<Self> {
        if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
            return Err(io::Error::other(
                "Open t-ui-uxplay in an interactive terminal",
            ));
        }
        terminal::enable_raw_mode()?;
        let mode = TerminalMode;
        execute!(io::stdout(), EnterAlternateScreen, EnableMouseCapture, Hide)?;
        if let Err(error) = execute!(io::stdout(), EnableBracketedPaste)
            && error.kind() != io::ErrorKind::Unsupported
        {
            return Err(error);
        }
        let terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
        Ok(Self {
            terminal,
            graphics: Graphics::detect(),
            _mode: mode,
        })
    }

    pub fn draw(
        &mut self,
        player: &Player,
        name: &str,
        now: Instant,
        dialog: Option<&Dialog>,
    ) -> io::Result<Screen> {
        let mut screen = Screen::default();
        self.terminal.draw(|frame| {
            screen = ui::draw(frame, player, name, self.graphics.enabled, now, dialog);
        })?;
        self.graphics.present(
            player.artwork.as_ref(),
            player.artwork_revision,
            screen.cover,
        )?;
        Ok(screen)
    }

    pub fn resized(&mut self) -> io::Result<()> {
        self.graphics.clear()
    }
}

struct TerminalMode;

impl Drop for TerminalMode {
    fn drop(&mut self) {
        let mut output = io::stdout();
        let _ = execute!(output, Show);
        let _ = execute!(output, DisableBracketedPaste);
        let _ = execute!(output, DisableMouseCapture);
        let _ = execute!(output, LeaveAlternateScreen);
        let _ = terminal::disable_raw_mode();
    }
}

struct Graphics {
    enabled: bool,
    id: u32,
    placement: Option<(u64, Rect)>,
}

impl Graphics {
    fn detect() -> Self {
        let term = env::var("TERM").unwrap_or_default();
        let multiplexer =
            env::var_os("TMUX").is_some() || term.starts_with("screen") || term.starts_with("tmux");
        let kitty = term == "xterm-kitty" || env::var_os("KITTY_WINDOW_ID").is_some();
        let ghostty = term == "xterm-ghostty"
            || env::var("TERM_PROGRAM").is_ok_and(|program| program == "ghostty");
        Self {
            enabled: !multiplexer && (kitty || ghostty),
            id: std::process::id(),
            placement: None,
        }
    }

    fn present(
        &mut self,
        image: Option<&DynamicImage>,
        revision: u64,
        area: Option<Rect>,
    ) -> io::Result<()> {
        let wanted = area.map(|area| (revision, area));
        if !self.enabled || wanted == self.placement {
            return Ok(());
        }
        self.clear()?;
        if let (Some(area), Some(image)) = (area, image) {
            let mut output = io::stdout().lock();
            execute!(output, MoveTo(area.x, area.y))?;
            send_image(&mut output, self.id, image, area)?;
            output.flush()?;
            self.placement = wanted;
        }
        Ok(())
    }

    fn clear(&mut self) -> io::Result<()> {
        if self.placement.take().is_some() {
            let mut output = io::stdout().lock();
            write!(output, "\x1b_Ga=d,d=I,i={},q=2\x1b\\", self.id)?;
            output.flush()?;
        }
        Ok(())
    }
}

impl Drop for Graphics {
    fn drop(&mut self) {
        let _ = self.clear();
    }
}

fn send_image(
    output: &mut impl Write,
    id: u32,
    image: &DynamicImage,
    area: Rect,
) -> io::Result<()> {
    let image = image.to_rgba8();
    let mut bytes = Vec::new();
    PngEncoder::new(&mut bytes)
        .write_image(
            image.as_raw(),
            image.width(),
            image.height(),
            image::ExtendedColorType::Rgba8,
        )
        .map_err(io::Error::other)?;
    let encoded = STANDARD.encode(bytes);
    let mut chunks = encoded.as_bytes().chunks(4096).peekable();
    let mut first = true;
    while let Some(chunk) = chunks.next() {
        let more = u8::from(chunks.peek().is_some());
        if first {
            write!(
                output,
                "\x1b_Ga=T,t=d,f=100,i={id},c={},r={},C=1,q=2,m={more};",
                area.width, area.height
            )?;
            first = false;
        } else {
            write!(output, "\x1b_Gm={more};")?;
        }
        output.write_all(chunk)?;
        output.write_all(b"\x1b\\")?;
    }
    Ok(())
}
