use std::io::{BufReader, Read};
use std::ops::Range;
use std::sync::mpsc::TryRecvError;
use std::sync::{mpsc, Arc};
use std::thread;
use std::thread::sleep;
use std::time::Duration;

use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use termwiz::caps::Capabilities;
use termwiz::cell::AttributeChange;
use termwiz::color::{AnsiColor, ColorAttribute};
use termwiz::input::{InputEvent, KeyCode, KeyEvent};
use termwiz::surface::{Change, Position, Surface};
use termwiz::terminal::buffered::BufferedTerminal;
use termwiz::terminal::{new_terminal, ScreenSize, Terminal};
use termwiz::Error;
use wezterm_term::color::ColorPalette;
use wezterm_term::{CellAttributes, TerminalConfiguration, TerminalSize};

#[derive(Debug)]
struct TermConfig {
    scroll_back: usize,
}
impl TerminalConfiguration for TermConfig {
    fn scrollback_size(&self) -> usize {
        self.scroll_back
    }

    fn color_palette(&self) -> ColorPalette {
        ColorPalette::default()
    }
}

fn main() -> Result<(), Error> {
    let mut term = wezterm_term::Terminal::new(
        TerminalSize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
            dpi: 0,
        },
        Arc::new(TermConfig { scroll_back: 1000 }),
        "sudare",
        "0.1.0",
        Box::new(Vec::new()),
    );

    let pty_system = NativePtySystem::default();

    let pty = pty_system.openpty(PtySize {
        rows: 24,
        cols: 80,
        pixel_width: 0,
        pixel_height: 0,
    })?;

    let mut cmd = CommandBuilder::new("sh");
    cmd.args(["-c", "curl -s https://gist.githubusercontent.com/HaleTom/89ffe32783f89f403bba96bd7bcd1263/raw/e50a28ec54188d2413518788de6c6367ffcea4f7/print256colours.sh | bash"]);
    let mut child = pty.slave.spawn_command(cmd).unwrap();
    drop(pty.slave);

    let (tx, rx) = mpsc::channel();
    let reader = pty.master.try_clone_reader().unwrap();

    thread::spawn(move || {
        let mut br = BufReader::with_capacity(1024, reader);
        let mut buffer = [0u8; 1024];
        loop {
            let n = br.read(&mut buffer[..]).unwrap();
            if n == 0 {
                //reader.try_wait().unwrap();
                child.try_wait().unwrap();
                break;
            }

            tx.send(Vec::from(&buffer[..n])).unwrap();
        }
    });

    /////////////////
    let caps = Capabilities::new_from_env()?;

    let mut buf = BufferedTerminal::new(new_terminal(caps)?)?;
    buf.terminal().set_raw_mode()?;
    buf.terminal().enter_alternate_screen()?;
    buf.terminal().set_screen_size(ScreenSize {
        rows: 24,
        cols: 80,
        xpixel: 0,
        ypixel: 0,
    })?;

    let mut block = Surface::new(5, 5);
    block.add_change(Change::ClearScreen(AnsiColor::Blue.into()));
    block.add_change("1234567890");
    buf.draw_from_screen(&block, 10, 10);

    buf.add_change(Change::Attribute(AttributeChange::Foreground(
        AnsiColor::Maroon.into(),
    )));
    buf.add_change("Hello world\r\n");
    buf.add_change(Change::Attribute(AttributeChange::Foreground(
        AnsiColor::Red.into(),
    )));
    buf.add_change("and in red here\r\n");
    buf.add_change(Change::CursorPosition {
        x: Position::Absolute(0),
        y: Position::Absolute(20),
    });

    buf.flush()?;

    loop {
        match buf.terminal().poll_input(Some(Duration::ZERO)) {
            Ok(Some(input)) => match input {
                InputEvent::Key(KeyEvent {
                    key: KeyCode::Escape,
                    ..
                }) => {
                    break;
                }
                InputEvent::Key(KeyEvent {
                    key: KeyCode::Char(c),
                    ..
                }) => {
                    buf.add_change(format!("{}", c));
                    buf.flush()?;
                }
                _ => {
                    print!("{:?}\r\n", input);
                }
            },
            Ok(None) => {}
            Err(e) => {
                print!("{:?}\r\n", e);
                break;
            }
        }
        {
            let mut buffer = Vec::<u8>::new();

            loop {
                match rx.try_recv() {
                    Ok(mut bytes) => {
                        buffer.append(&mut bytes);
                        if buffer.len() > 1024 {
                            break;
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => break,
                }
            }

            if !buffer.is_empty() {
                term.advance_bytes(&buffer);
                buffer.clear();

                buf.add_change(Change::ClearScreen(ColorAttribute::Default));

                let changes = term
                    .screen()
                    .lines_in_phys_range(Range { start: 0, end: 24 })
                    .iter()
                    .flat_map(|l| {
                        let mut xs = l.changes(&CellAttributes::default());
                        xs.append(&mut vec![
                            Change::ClearToEndOfLine(ColorAttribute::Default),
                            Change::Text("\r\n".into()),
                        ]);
                        xs
                    })
                    .collect();

                buf.add_changes(changes);

                //print!("{} ", bytes.len());
                buf.flush().unwrap();
            }
        }

        sleep(Duration::from_millis(10));
    }

    Ok(())
}
