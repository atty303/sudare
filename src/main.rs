use std::collections::BTreeMap;
use std::fmt::Debug;
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::ops::Range;
use std::path::Path;
use std::string::ToString;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{mpsc, Arc};
use std::thread;
use std::thread::{sleep, JoinHandle};
use std::time::Duration;

use portable_pty::{
    Child, CommandBuilder, ExitStatus, NativePtySystem, PtyPair, PtySize, PtySystem,
};
use regex::Regex;
use termwiz::caps::{Capabilities, ProbeHints};
use termwiz::cell::{AttributeChange, CellAttributes};
use termwiz::color::{AnsiColor, ColorAttribute};
use termwiz::input::{InputEvent, KeyCode, KeyEvent};
use termwiz::surface::{Change, Position, SequenceNo, Surface};
use termwiz::terminal::buffered::BufferedTerminal;
use termwiz::terminal::{new_terminal, Terminal};
use termwiz::Error;
use wezterm_term::color::ColorPalette;
use wezterm_term::{ScrollbackOrVisibleRowIndex, TerminalConfiguration, TerminalSize};

type Procfile = Vec<ProcessGroup>;

fn parse_procfile(path: &Path) -> std::io::Result<Procfile> {
    let re: Regex = Regex::new(r"^(.+)\[(.+)\]$").unwrap();
    let reader = BufReader::new(File::open(path)?);
    let (ordered, map) = reader
        .lines()
        .map(|l| l.unwrap())
        .filter(|l| !l.starts_with("#") || l.contains(":"))
        .map(|l| {
            let (title, cmd) = {
                let mut it = l.splitn(2, ":");
                (it.next().unwrap(), it.next().unwrap())
            };
            let (a, b) = re
                .captures(title)
                .map(|cap| (cap.get(1).unwrap().as_str(), cap.get(2).unwrap().as_str()))
                .unwrap_or((title, "default"));
            (a.to_string(), b.to_string(), cmd.trim().to_string())
        })
        .fold(
            (
                Vec::<String>::new(),
                BTreeMap::<String, Vec<(String, String)>>::new(),
            ),
            |mut acc, (a, b, c)| {
                if !acc.0.contains(&a) {
                    acc.0.push(a.clone());
                }
                acc.1.entry(a).or_default().push((b, c));
                acc
            },
        );
    let r: Procfile = ordered
        .iter()
        .map(|title| (title, map.get(title).unwrap()))
        .map(|(title, members)| ProcessGroup {
            title: title.clone(),
            members: vec![Process::Null]
                .into_iter()
                .chain(members.iter().map(|(label, cmd)| Process::Command {
                    title: label.clone(),
                    argv: cmd.clone(),
                }))
                .collect(),
        })
        .collect();
    Ok(r)
}

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

const DEFAULT_TITLE: &str = "disable";

impl Process {
    pub fn title(&self) -> String {
        match self {
            Process::Null => DEFAULT_TITLE.to_string(),
            Process::Command { title, argv: _ } => title.to_string(),
        }
    }
}

struct UiState {
    focused_window_index: usize,
    windows: Vec<UiWindow>,
    surface: Surface,
    min_window_height: usize,
}

impl UiState {
    pub fn new(procfile: Procfile, dimension: (usize, usize)) -> UiState {
        UiState {
            focused_window_index: 0,
            windows: procfile.into_iter().map(|it| UiWindow::new(it)).collect(),
            surface: Surface::new(dimension.0, dimension.1),
            min_window_height: 2,
        }
    }

    pub fn previous_window(&mut self) {
        if let Some(group) = self.windows.get_mut(self.focused_window_index) {
            group.reset_scroll();
        }
        self.focused_window_index = if self.focused_window_index > 0 {
            self.focused_window_index - 1
        } else {
            0
        }
    }

    pub fn next_window(&mut self) {
        if let Some(group) = self.windows.get_mut(self.focused_window_index) {
            group.reset_scroll();
        }
        self.focused_window_index =
            if self.focused_window_index < self.windows.len().saturating_sub(1) {
                self.focused_window_index + 1
            } else {
                self.focused_window_index
            };
    }

    pub fn select_process(&mut self, pty_system: &dyn PtySystem, index: usize) {
        if let Some(group) = self.windows.get_mut(self.focused_window_index) {
            group.set_active(pty_system, self.surface.dimensions(), index);
        }
    }

    pub fn scroll_up(&mut self) {
        if let Some(group) = self.windows.get_mut(self.focused_window_index) {
            group.scroll_up();
        }
    }

    pub fn scroll_down(&mut self) {
        if let Some(group) = self.windows.get_mut(self.focused_window_index) {
            group.scroll_down();
        }
    }

    pub fn render_to_screen(&mut self, screen: &mut Surface) {
        let (width, height) = screen.dimensions();

        // Render from scratch into a fresh screen buffer
        let mut alt_screen = Surface::new(width, height);

        let unfocused_height = self.windows.len().saturating_sub(1) * (1 + self.min_window_height);
        let focused_height = height - unfocused_height;

        self.windows
            .iter_mut()
            .enumerate()
            .fold(0usize, |y, (i, it)| {
                let focused = i == self.focused_window_index;
                let h = if focused {
                    focused_height
                } else {
                    1 + self.min_window_height
                };
                it.render(&mut alt_screen, width, y, h, focused);
                y + h
            });

        // TODO: これをするとチラつくのでやらないで済む方法がないか
        screen.add_change(Change::ClearScreen(ColorAttribute::Default));

        // Now compute a delta and apply it to the actual screen
        let diff = screen.diff_screens(&alt_screen);
        screen.add_changes(diff);
    }
}

struct UiWindow {
    process_group: ProcessGroup,
    active_process_index: usize,
    pty_terminal: Option<PtyTerminal>,
}

impl UiWindow {
    pub fn new(process_group: ProcessGroup) -> Self {
        Self {
            process_group,
            active_process_index: 0,
            pty_terminal: None,
        }
    }

    pub fn set_active(
        &mut self,
        pty_system: &dyn PtySystem,
        dimension: (usize, usize),
        index: usize,
    ) {
        if let Some(process) = self.process_group.members.get(index) {
            // if let Some(t) = &mut self.pty_terminal {
            //     t.pty_process.kill().unwrap();
            // }
            self.pty_terminal = None;

            self.active_process_index = index;

            if let Process::Command { title: _, argv } = process {
                if let Ok(pp) = PtyProcess::new(pty_system, dimension, argv) {
                    self.pty_terminal = Some(PtyTerminal::new(pp, dimension));
                }
            }
        }
    }

    pub fn scroll_up(&mut self) {
        if let Some(t) = &mut self.pty_terminal {
            t.scroll_up();
        }
    }

    pub fn scroll_down(&mut self) {
        if let Some(t) = &mut self.pty_terminal {
            t.scroll_down();
        }
    }

    pub fn reset_scroll(&mut self) {
        if let Some(t) = &mut self.pty_terminal {
            t.reset_scroll();
        }
    }

    pub fn render(&mut self, screen: &mut Surface, w: usize, y: usize, h: usize, focused: bool) {
        if let Some(t) = &mut self.pty_terminal {
            t.resize_soft(w, h - 1);
        }

        let status_color = if focused {
            AnsiColor::Fuchsia
        } else {
            AnsiColor::Grey
        };
        let mut changes = vec![
            Change::CursorPosition {
                x: Position::Absolute(0),
                y: Position::Absolute(y),
            },
            Change::Attribute(AttributeChange::Background(ColorAttribute::from(
                status_color,
            ))),
            Change::Attribute(AttributeChange::Foreground(ColorAttribute::from(
                AnsiColor::White,
            ))),
            Change::Text(self.process_group.title.clone()),
            Change::Text(" | ".to_string()),
        ];
        let line = self
            .process_group
            .members
            .iter()
            .enumerate()
            .map(|(i, it)| {
                let indicator = if i == self.active_process_index {
                    "*"
                } else {
                    ""
                };
                format!("{}{}:{}", indicator, i, it.title())
            })
            .collect::<Vec<_>>()
            .join(" ");
        changes.push(Change::Text(line));
        changes.push(Change::ClearToEndOfLine(ColorAttribute::from(status_color)));
        changes.push(Change::AllAttributes(CellAttributes::default()));
        changes.push(Change::CursorPosition {
            x: Position::Absolute(0),
            y: Position::Relative(1),
        });

        if let Some(pt) = &mut self.pty_terminal {
            if let Some(mut xs) = pt.poll() {
                changes.append(&mut xs);
            }
        }

        screen.add_changes(changes);
        screen.flush_changes_older_than(SequenceNo::MAX);
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

struct PtyTerminal {
    terminal: wezterm_term::Terminal,
    pty_process: PtyProcess,
    scroll_offset: isize,
}

impl PtyTerminal {
    pub fn new(pty_process: PtyProcess, dimension: (usize, usize)) -> Self {
        let terminal = wezterm_term::Terminal::new(
            TerminalSize {
                rows: dimension.1,
                cols: dimension.0,
                pixel_width: 0,
                pixel_height: 0,
                dpi: 0,
            },
            Arc::new(TermConfig { scroll_back: 1000 }),
            "sudare",
            "0.1.0",
            Box::new(Vec::new()),
        );

        Self {
            terminal,
            pty_process,
            scroll_offset: 0,
        }
    }

    pub fn scroll_up(&mut self) {
        if self.scroll_offset
            > -((self.terminal.screen().scrollback_rows() - self.terminal.screen().physical_rows)
                as isize)
        {
            self.scroll_offset -= 1;
        }
    }

    pub fn scroll_down(&mut self) {
        if self.scroll_offset < 0 {
            self.scroll_offset += 1;
        }
    }

    pub fn reset_scroll(&mut self) {
        self.scroll_offset = 0;
    }

    pub fn resize_soft(&mut self, w: usize, h: usize) {
        let c = self.terminal.get_size();
        if c.cols != w || c.rows != h {
            self.terminal.resize(TerminalSize {
                rows: h,
                cols: w,
                pixel_width: 0,
                pixel_height: 0,
                dpi: 0,
            })
        }
    }

    pub fn poll(&mut self) -> Option<Vec<Change>> {
        let buffer = self.pty_process.poll();
        if !buffer.is_empty() {
            self.terminal.advance_bytes(&buffer);
        }

        let c = self.terminal.get_size();
        let visible_range = Range {
            start: self.scroll_offset as ScrollbackOrVisibleRowIndex,
            end: (self.scroll_offset + c.rows as isize) as ScrollbackOrVisibleRowIndex,
        };

        let screen = self.terminal.screen();
        //screen.physical_rows
        let (_, changes) = screen
            .lines_in_phys_range(screen.scrollback_or_visible_range(&visible_range))
            .iter()
            .fold(
                (CellAttributes::default(), Vec::<Change>::new()),
                |(a, mut xs), line| {
                    line.visible_cells()
                        .last()
                        .map(|c| {
                            //let ys = &mut xs;
                            xs.extend(line.changes(&a));
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
        Some(changes)
    }
}

enum PtyMessage {
    Bytes(Vec<u8>),
}

struct PtyProcess {
    pty: PtyPair,
    child: Box<dyn Child + Send + Sync>,
    child_handle: Option<JoinHandle<()>>,
    receiver: Receiver<PtyMessage>,
    exit_status: Option<ExitStatus>,
}

impl PtyProcess {
    pub fn new(
        pty_system: &dyn PtySystem,
        dimension: (usize, usize),
        argv: &str,
    ) -> Result<Self, Error> {
        let pty = pty_system.openpty(PtySize {
            rows: dimension.1 as u16,
            cols: dimension.0 as u16,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        let mut cmd = CommandBuilder::new("sh");
        cmd.args(["-c", argv]);
        let maybe_child = pty.slave.spawn_command(cmd);
        drop(&pty.slave);
        let child = maybe_child?;

        let (tx, receiver) = mpsc::channel();
        let mut reader = pty.master.try_clone_reader()?;

        let child_handle = thread::Builder::new()
            .name(argv.to_string())
            .spawn(move || {
                let mut buffer = [0u8; 1024];
                loop {
                    let n = reader.read(&mut buffer[..]).unwrap();
                    if n == 0 {
                        break;
                    } else {
                        tx.send(PtyMessage::Bytes(buffer[..n].to_vec())).unwrap();
                    }
                }
                log::info!("thread finished");
            })?;

        Ok(Self {
            pty,
            child,
            child_handle: Some(child_handle),
            receiver,
            exit_status: None,
        })
    }

    pub fn kill(&mut self) -> std::io::Result<()> {
        match self.child.try_wait() {
            Ok(Some(_)) => Ok(()),
            Ok(None) => self.child.kill(),
            Err(e) => Err(e),
        }
        // if let Some(handle) = self.child_handle.take() {
        //     let r = match handle.join() {
        //         Ok(_) => Ok(()),
        //         Err(e) => Err(std::io::Error::new(ErrorKind::Other, format!("{:?}", e))),
        //     };
        //     self.child_handle = None;
        //     r
        // } else {
        //     Ok(())
        // }
    }

    pub fn poll(&mut self) -> Vec<u8> {
        let mut buffer = Vec::<u8>::new();

        loop {
            match self.receiver.try_recv() {
                Ok(PtyMessage::Bytes(mut bytes)) => {
                    buffer.append(&mut bytes);
                    if buffer.len() > 1024 {
                        break;
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }

        match self.child.try_wait() {
            Ok(Some(r)) => {
                if self.exit_status.is_none() {
                    buffer.append(
                        &mut format!("[process exited with {}]", r.exit_code())
                            .as_bytes()
                            .to_vec(),
                    );
                    self.exit_status = Some(r);
                }
            }
            Ok(None) => {}
            Err(e) => {
                log::error!("try_wait error: {}", e);
            }
        }

        buffer
    }
}

impl Drop for PtyProcess {
    fn drop(&mut self) {
        log::debug!("pty_process dropped");

        let writer = self.pty.master.take_writer().unwrap();
        drop(writer);

        self.kill().unwrap();

        self.child.wait().unwrap();

        drop(&self.pty.master);

        drop(&self.child_handle);
        // if let Some(handle) = self.child_handle.take() {
        //     handle.join().unwrap();
        // }
        log::debug!("done");
    }
}

fn main() -> Result<(), Error> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        panic!("You must specify path to Procfile");
    }
    let procfile = parse_procfile(Path::new(&args[1].as_str()))?;
    // simplelog::WriteLogger::init(
    //     simplelog::LevelFilter::Debug,
    //     simplelog::Config::default(),
    //     File::create("sudare.log").unwrap(),
    // )
    // .unwrap();

    let pty_system = NativePtySystem::default();

    let caps =
        Capabilities::new_with_hints(ProbeHints::new_from_env().mouse_reporting(Some(false)))?;

    let mut buf = BufferedTerminal::new(new_terminal(caps)?)?;
    buf.terminal().set_raw_mode()?;
    buf.terminal().enter_alternate_screen()?;

    let mut ui_state = UiState::new(procfile, buf.dimensions());

    loop {
        match buf.terminal().poll_input(Some(Duration::ZERO)) {
            Ok(Some(InputEvent::Resized { rows, cols })) => {
                // FIXME: this is working around a bug where we don't realize
                // that we should redraw everything on resize in BufferedTerminal.
                buf.add_change(Change::ClearScreen(Default::default()));
                buf.resize(cols, rows);
            }
            Ok(Some(input)) => match input {
                InputEvent::Key(KeyEvent {
                    key: KeyCode::Escape,
                    ..
                }) => break,
                InputEvent::Key(KeyEvent {
                    key: KeyCode::Char('n'),
                    ..
                })
                | InputEvent::Key(KeyEvent {
                    key: KeyCode::DownArrow,
                    ..
                }) => ui_state.next_window(),
                InputEvent::Key(KeyEvent {
                    key: KeyCode::Char('p'),
                    ..
                })
                | InputEvent::Key(KeyEvent {
                    key: KeyCode::UpArrow,
                    ..
                }) => ui_state.previous_window(),
                InputEvent::Key(KeyEvent {
                    key: KeyCode::Char(c),
                    ..
                }) if c.is_digit(10) => {
                    ui_state.select_process(&pty_system, c.to_digit(10).unwrap() as usize)
                }
                InputEvent::Key(KeyEvent {
                    key: KeyCode::Char('k'),
                    ..
                }) => {
                    ui_state.scroll_up();
                }
                InputEvent::Key(KeyEvent {
                    key: KeyCode::Char('j'),
                    ..
                }) => {
                    ui_state.scroll_down();
                }
                _ => {}
            },
            Ok(None) => {}
            Err(e) => {
                print!("{:?}\r\n", e);
                break;
            }
        }

        ui_state.render_to_screen(&mut buf);
        buf.flush().unwrap();

        sleep(Duration::from_millis(10));
    }

    Ok(())
}
