use std::io::Write;
use termwiz::caps::Capabilities;
use termwiz::cell::AttributeChange;
use termwiz::color::AnsiColor;
use termwiz::input::{InputEvent, KeyCode, KeyEvent};
use termwiz::surface::{Change, Position, Surface};
use termwiz::terminal::buffered::BufferedTerminal;
use termwiz::terminal::{new_terminal, Terminal};
use termwiz::Error;

fn main() -> Result<(), Error> {
    let caps = Capabilities::new_from_env()?;

    let terminal = new_terminal(caps)?;

    let mut buf = BufferedTerminal::new(terminal)?;
    buf.terminal().set_raw_mode()?;
    buf.terminal().enter_alternate_screen()?;

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

    //buf.terminal().write("\x1B[1mHOGEHOGE".as_ref())?;

    loop {
        match buf.terminal().poll_input(None) {
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
    }

    Ok(())
}
