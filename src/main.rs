use std::hash::Hasher;
use std::io::{BufReader, Read, Write};
use std::sync::mpsc::TryRecvError;
use std::sync::{mpsc, Arc};
use std::thread;
use std::thread::sleep;
use std::time::Duration;

use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use termwiz::caps::{Capabilities, ProbeHints};
use termwiz::cell::AttributeChange;
use termwiz::color::{AnsiColor, ColorAttribute, ColorSpec};
use termwiz::input::{InputEvent, KeyCode, KeyEvent};
use termwiz::surface::{Change, CursorVisibility, Position, SequenceNo, Surface};
use termwiz::terminal::buffered::BufferedTerminal;
use termwiz::terminal::{new_terminal, ScreenSize, Terminal};
use termwiz::widgets::layout::{ChildOrientation, Constraints};
use termwiz::widgets::{CursorShapeAndPosition, RenderArgs, Ui, UpdateArgs, Widget, WidgetEvent};
use termwiz::Error;
use wezterm_term::color::ColorPalette;
use wezterm_term::{CellAttributes, TerminalConfiguration, TerminalSize};

type Procfile = Vec<ProcessGroup>;

#[derive(Debug, Clone)]
struct ProcessGroup {
    title: String,
    members: Vec<Process>,
}

#[derive(Debug, Clone)]
enum Process {
    Null,
    Command { title: String, argv: String },
}

struct UiState {
    procfile: Procfile,
    focused_window_index: usize,
    windows: Vec<UiWindow>,
    surface: Surface,
    min_window_height: usize,
}

impl UiState {
    pub fn new(procfile: Procfile, dimension: (usize, usize)) -> UiState {
        UiState {
            procfile: procfile.to_vec(),
            focused_window_index: 0,
            windows: procfile
                .into_iter()
                .map(|it| UiWindow { process_group: it })
                .collect(),
            surface: Surface::new(dimension.0, dimension.1),
            min_window_height: 2,
        }
    }

    pub fn set_focus(&mut self, delta: i32) {
        self.focused_window_index =
            ((self.focused_window_index as i32 + delta) % self.windows.len() as i32) as usize
    }

    pub fn render_to_screen(&self, screen: &mut Surface) {
        let (width, height) = screen.dimensions();
        // Render from scratch into a fresh screen buffer
        let mut alt_screen = Surface::new(width, height);

        let unfocused_height = self.windows.len().saturating_sub(1) * (1 + self.min_window_height);
        let focused_height = height - unfocused_height;
        self.windows.iter().enumerate().fold(0usize, |y, (i, it)| {
            let h = if i == self.focused_window_index {
                focused_height
            } else {
                1 + self.min_window_height
            };
            it.render(&mut alt_screen, y, h);
            y + h
        });

        // Now compute a delta and apply it to the actual screen
        let diff = screen.diff_screens(&alt_screen);
        screen.add_changes(diff);
    }
}

struct UiWindow {
    process_group: ProcessGroup,
}

impl UiWindow {
    pub fn render(&self, screen: &mut Surface, y: usize, h: usize) {
        screen.add_changes(vec![
            Change::CursorPosition {
                x: Position::Absolute(0),
                y: Position::Absolute(y),
            },
            Change::Text(self.process_group.title.clone()),
            Change::ClearToEndOfLine(ColorAttribute::from(AnsiColor::Green)),
        ]);
        screen.flush_changes_older_than(SequenceNo::max_value());
    }
}

struct MainScreen {}

impl MainScreen {
    pub fn new() -> Self {
        Self {}
    }
}

impl Widget for MainScreen {
    fn render(&mut self, args: &mut RenderArgs) {}

    fn get_size_constraints(&self) -> Constraints {
        let mut c = Constraints::default();
        c.child_orientation = ChildOrientation::Vertical;
        c
    }
}

struct Window<'a> {
    text: &'a mut String,
}

impl<'a> Window<'a> {
    /// Initialize the widget
    pub fn new(text: &'a mut String) -> Self {
        Self { text }
    }
}

impl<'a> Widget for Window<'a> {
    fn render(&mut self, args: &mut RenderArgs) {
        let d = args.surface.dimensions();
        // args.surface.add_change(Change::CursorPosition {
        //     x: Position::Absolute(0),
        //     y: Position::Absolute(0),
        // });
        args.surface.add_change(format!("size is {:?}\r\n", d));
        *args.cursor = CursorShapeAndPosition {
            visibility: CursorVisibility::Hidden,
            ..Default::default()
        }
    }

    fn get_size_constraints(&self) -> Constraints {
        Constraints::default().set_fixed_height(5).clone()
    }

    fn process_event(&mut self, event: &WidgetEvent, _args: &mut UpdateArgs) -> bool {
        match event {
            WidgetEvent::Input(InputEvent::Key(KeyEvent {
                key: KeyCode::Char(c),
                ..
            })) => match c.to_digit(10) {
                Some(i) => true,
                None => false,
            },
            _ => false,
        }
    }
}

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
    let procfile = vec![
        ProcessGroup {
            title: String::from("foo"),
            members: vec![
                Process::Null,
                Process::Command {
                    title: "cloudflare".to_string(),
                    argv: "ping 1.1.1.1".to_string(),
                },
                Process::Command {
                    title: "google".to_string(),
                    argv: "ping 8.8.8.8".to_string(),
                },
            ],
        },
        ProcessGroup {
            title: String::from("bar"),
            members: vec![
                Process::Null,
                Process::Command {
                    title: "cloudflare".to_string(),
                    argv: "ping 1.1.1.1".to_string(),
                },
                Process::Command {
                    title: "google".to_string(),
                    argv: "ping 8.8.8.8".to_string(),
                },
            ],
        },
    ];

    let mut term = wezterm_term::Terminal::new(
        TerminalSize {
            rows: 10,
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
        rows: 10,
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
    let mut tmp0 = String::new();
    let mut tmp1 = String::new();

    let caps =
        Capabilities::new_with_hints(ProbeHints::new_from_env().mouse_reporting(Some(false)))?;

    let mut buf = BufferedTerminal::new(new_terminal(caps)?)?;
    buf.terminal().set_raw_mode()?;
    //buf.terminal().enter_alternate_screen()?;
    buf.add_change("\x1b[?1000l");
    buf.add_change("\x1b[?1003l");

    // buf.terminal().set_screen_size(ScreenSize {
    //     rows: 10,
    //     cols: 80,
    //     xpixel: 0,
    //     ypixel: 0,
    // })?;

    let mut ui_state = UiState::new(procfile, (buf.dimensions()));

    loop {
        match buf.terminal().poll_input(Some(Duration::ZERO)) {
            Ok(Some(InputEvent::Resized { rows, cols })) => {
                // FIXME: this is working around a bug where we don't realize
                // that we should redraw everything on resize in BufferedTerminal.
                buf.add_change(Change::ClearScreen(Default::default()));
                buf.resize(cols, rows);
            }
            Ok(Some(input)) => match input {
                InputEvent::Mouse(m) => {
                    print!("mouse {:?}\r\n", m);
                }
                input @ InputEvent::Key(KeyEvent {
                    key: KeyCode::Escape,
                    ..
                }) => {
                    print!("escaped {:?}\r\n", input);
                    break;
                }
                InputEvent::Key(KeyEvent {
                    key: KeyCode::Char('n'),
                    ..
                }) => {
                    ui_state.set_focus(1);
                }
                InputEvent::Key(KeyEvent {
                    key: KeyCode::Char('p'),
                    ..
                }) => {
                    ui_state.set_focus(-1);
                }
                input @ _ => {}
            },
            Ok(None) => {}
            Err(e) => {
                print!("{:?}\r\n", e);
                break;
            }
        }

        /*
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

                //                buf.add_change(Change::ClearScreen(ColorAttribute::Default));
                buf.add_change(Change::CursorPosition {
                    x: Position::Absolute(20),
                    y: Position::Absolute(20),
                });

                let screen = term.screen();
                let (_, changes) = screen
                    .lines_in_phys_range(screen.phys_range(&(0..10)))
                    .iter()
                    .fold(
                        (CellAttributes::default(), Vec::<Change>::new()),
                        |(a, mut xs), l| {
                            l.visible_cells()
                                .last()
                                .map(|c| {
                                    //let ys = &mut xs;
                                    xs.extend(l.changes(&a));
                                    xs.push(Change::ClearToEndOfLine(ColorAttribute::Default));
                                    xs.push(Change::CursorPosition {
                                        x: Position::Absolute(0),
                                        y: Position::Relative(1),
                                    });
                                    // TODO: c.attrs().wrapped() ?
                                    (c.attrs().clone(), xs.to_vec())
                                })
                                .unwrap_or({
                                    xs.push(Change::ClearToEndOfLine(ColorAttribute::Default));
                                    xs.push(Change::CursorPosition {
                                        x: Position::Absolute(0),
                                        y: Position::Relative(1),
                                    });
                                    (a, xs)
                                })
                        },
                    );

                buf.add_changes(changes.to_vec());

                //print!("{} ", bytes.len());
                buf.flush().unwrap();
            }
        }
         */

        ui_state.render_to_screen(&mut buf);
        buf.flush();

        sleep(Duration::from_millis(10));
    }

    Ok(())
}
