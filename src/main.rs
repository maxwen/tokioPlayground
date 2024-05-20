extern crate core;

use std::cmp::{max, min, PartialEq};
use std::error::Error;
use std::io::{Read, Seek, stdout, Write};
use std::num::NonZeroUsize;
use std::sync::Arc;

use crossterm::{event, ExecutableCommand};
use crossterm::event::Event::Key;
use crossterm::event::KeyCode;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use futures_util::TryStreamExt;
use rand::Rng;
use ratatui::{Frame, Terminal};
use ratatui::prelude::{Backend, Color, Constraint, CrosstermBackend, Direction, Layout, Rect, Span, Style, Stylize, Text};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use rodio::{Decoder, Sink, Source};
use serde::{Deserialize, Serialize};
use stream_download::source::SourceStream;
use symphonia::core::conv::IntoSample;
use tokio::io::AsyncReadExt;
use tokio::sync::broadcast;
use tokio::sync::broadcast::{Receiver, Sender};

use crate::bounded::BoundedStorageProvider;
use crate::memory::MemoryStorageProvider;
use crate::stream::StreamDownload;

mod memory;
mod source;
mod stream;
mod bounded;
mod spy_decoder;
mod storage;

const MEMORY_BUFFER_SIZE: usize = 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
struct RadioStation {
    url: String,
    title: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct RadioStationList {
    list: Vec<RadioStation>,
}

impl RadioStation {
    fn new(title: &str, url: &str) -> Self {
        RadioStation {
            url: String::from(url),
            title: String::from(title),
        }
    }

    fn title(&self) -> &str {
        self.title.as_str()
    }

    fn url(&self) -> &str {
        self.url.as_str()
    }
}

impl PartialEq for RadioStation {
    fn eq(&self, other: &Self) -> bool {
        self.title == other.title
    }
}

impl RadioStationList {
    fn add(&mut self, station: RadioStation) {
        self.list.push(station);
    }

    fn get(&self, name: &str) -> Option<RadioStation> {
        if let Some(station) = self.list.iter().find(|station| station.title == name) {
            return Some(station.clone());
        }
        None
    }

    fn get_at_index(&self, index: usize) -> Option<RadioStation> {
        if index >= 0 && index < self.list.len() {
            return Some(self.list[index].clone());
        }
        None
    }

    fn get_random(&self) -> Option<RadioStation> {
        if self.list.len() != 0 {
            return Some(self.list[rand::thread_rng().gen_range(0..self.list.len())].clone());
        }
        None
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct MetaData {
    title: String,
}

impl MetaData {
    fn new(title: &str) -> Self {
        MetaData { title: title.to_string() }
    }

    fn title(&self) -> &str {
        self.title.as_str()
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Action {
    Start,
    Pause,
    Resume,
    Quit,
}

#[derive(Clone, Debug)]
struct Command {
    action: Action,
    station: Option<RadioStation>,
}

impl Command {
    pub fn new_start(station: RadioStation) -> Command {
        Command {
            action: Action::Start,
            station: Some(station.clone()),
        }
    }
    pub fn new_pause() -> Command {
        Command {
            action: Action::Pause,
            station: None,
        }
    }

    pub fn new_resume() -> Command {
        Command {
            action: Action::Resume,
            station: None,
        }
    }

    pub fn new_quit() -> Command {
        Command {
            action: Action::Quit,
            station: None,
        }
    }
}

async fn play_station(station: RadioStation, sink: Arc<Sink>, command_channel_rx: Receiver<Command>, meta_channel_tx: Sender<MetaData>) -> Result<(), Box<dyn Error>> {
    let storage_provider = BoundedStorageProvider::new(
        MemoryStorageProvider::new(),
        NonZeroUsize::new(MEMORY_BUFFER_SIZE).unwrap());
    let stream_download = StreamDownload::new(station.url(), storage_provider, command_channel_rx, meta_channel_tx).await?;

    let play_sink = sink.clone();
    play_sink.play();

    let handle = tokio::task::spawn_blocking(move || {
        match Decoder::new(stream_download) {
            Ok(decoder) => {
                play_sink.append(decoder);
            }
            Err(_) => {}
        }
    });
    handle.await?;
    Ok(())
}

#[derive(Clone, Debug)]
pub struct App {
    pub station_list: RadioStationList,
    current_station: Option<RadioStation>,
    current_meta: Option<MetaData>,
    current_status: Option<String>,
}

impl App {
    pub fn init() -> Self {
        let mut station_list = RadioStationList { list: vec![] };
        station_list.add(RadioStation::new("FM4", "http://orf-live.ors-shoutcast.at/fm4-q2a"));
        station_list.add(RadioStation::new("MetalRock.FM", "http://cheetah.streemlion.com:2160/stream"));
        station_list.add(RadioStation::new("Terry Callaghan's Classic Alternative Channel", "http://s16.myradiostream.com:7304"));
        station_list.add(RadioStation::new("OE3", "http://orf-live.ors-shoutcast.at/oe3-q2a"));
        station_list.add(RadioStation::new("local", "http://192.168.1.70:8001/stream"));

        Self {
            station_list,
            current_station: None,
            current_meta: None,
            current_status: None,
        }
    }

    pub fn set_current_station(&mut self, radio_station: RadioStation) {
        self.current_station = Some(radio_station.clone());
    }

    pub fn set_current_station_index(&mut self, index: usize) {
        self.set_current_station(self.station_list.get_at_index(index).unwrap());
    }

    pub fn set_random_station(&mut self) {
        self.current_station = self.station_list.get_random();
    }

    pub fn set_next_station(&mut self) {
        let current_index = self.current_station_index();
        if current_index.is_none() {
            self.set_current_station(self.station_list.list.first().unwrap().clone())
        } else {
            let next_index = min(current_index.unwrap() + 1, self.station_list.list.len() - 1);
            self.set_current_station(self.station_list.get_at_index(next_index).unwrap());
        }
    }

    pub fn set_prev_station(&mut self) {
        let current_index = self.current_station_index();
        if current_index.is_none() {
            self.set_current_station(self.station_list.list.first().unwrap().clone())
        } else {
            let prev_index = current_index.unwrap().saturating_sub(1);
            self.set_current_station(self.station_list.get_at_index(prev_index).unwrap());
        }
    }

    pub fn current_station(&self) -> Option<RadioStation> {
        self.current_station.clone()
    }

    pub fn current_station_index(&self) -> Option<usize> {
        if self.current_station.is_some() {
            let current_title = self.current_station().unwrap().title;
            return self.station_list.list.iter().position(|n| n.title == current_title);
        }
        None
    }

    pub fn set_current_meta(&mut self, meta: MetaData) {
        self.current_meta = Some(meta.clone())
    }

    pub fn current_meta(&self) -> Option<MetaData> {
        self.current_meta.clone()
    }
    pub fn set_current_status(&mut self, status: Option<String>) {
        self.current_status = status;
    }

    pub fn current_status(&self) -> Option<String> {
        self.current_status.clone()
    }
}

fn draw_radio_stations(
    f: &mut Frame,
    app: &App,
    layout_chunk: Rect,
    title: &str,
)
{
    let mut state = ListState::default().with_selected(app.current_station_index());

    let lst_items: Vec<ListItem> = app.station_list.list
        .iter()
        .map(|station| ListItem::new(Span::raw(station.title())))
        .collect();

    let list = List::new(lst_items)
        .highlight_style(Style::default().bg(Color::LightBlue).fg(Color::Black))
        .block(
            Block::default()
                .title(title)
                .borders(Borders::ALL));
    f.render_stateful_widget(list, layout_chunk, &mut state);
}

fn draw_now_playing(
    f: &mut Frame,
    app: &App,
    layout_chunk: Rect,
    title: &str,
)
{
    if app.current_meta().is_some() {
        let cm = app.current_meta().unwrap();
        let lines = Text::from(cm.title());
        let now_playing = Paragraph::new(lines).block(
            Block::new().borders(Borders::ALL).title(title));
        f.render_widget(now_playing, layout_chunk);
    } else {
        let lines = Text::from("");
        let now_playing = Paragraph::new(lines).block(
            Block::new().borders(Borders::ALL).title(title));
        f.render_widget(now_playing, layout_chunk);
    }
}

fn draw_status(
    f: &mut Frame,
    app: &App,
    layout_chunk: Rect,
)
{
    if app.current_status().is_some() {
        let msg = app.current_status().unwrap();
        let lines = Text::from(msg);
        let status = Paragraph::new(lines);
        f.render_widget(status, layout_chunk);
    } else {
        let lines = Text::from("");
        let status = Paragraph::new(lines);
        f.render_widget(status, layout_chunk);
    }
}

#[tokio::main]
async fn main() {
    let (command_channel_tx, mut command_channel_rx) = broadcast::channel::<Command>(1);
    let (meta_channel_tx, mut meta_channel_rx) = broadcast::channel::<MetaData>(1);

    let (_stream, handle) = rodio::OutputStream::try_default().unwrap();
    let sink = Arc::new(rodio::Sink::try_new(&handle).unwrap());

    let mut app = App::init();
    app.set_current_station_index(0);

    let command_sink = sink.clone();

    enable_raw_mode().unwrap();
    stdout().execute(EnterAlternateScreen).unwrap();
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout())).unwrap();

    loop {
        if let Ok(meta_data) = meta_channel_rx.try_recv() {
            app.set_current_meta(meta_data);
        }
        terminal.draw(|frame| {
            let main_layout = Layout::new(
                Direction::Vertical,
                [
                    Constraint::Percentage(80),
                    Constraint::Percentage(30),
                    Constraint::Percentage(10),
                ],
            )
                .split(frame.size());

            draw_radio_stations(frame, &app, main_layout[0], "Station List");
            draw_now_playing(frame, &app, main_layout[1], "Now Playing");
            draw_status(frame, &app, main_layout[2]);
        }).unwrap();

        if event::poll(std::time::Duration::from_millis(50)).unwrap() {
            if let Key(key) = event::read().unwrap() {
                if key.kind == event::KeyEventKind::Press {
                    match key.code {
                        KeyCode::Up => {
                            app.set_prev_station();
                        }
                        KeyCode::Down => {
                            app.set_next_station();
                        }
                        KeyCode::Char('s') => {
                            // stop download
                            let command = Command::new_quit();
                            let _ = command_channel_tx.send(command);
                            // stop play task
                            command_sink.stop();
                            // start new
                            // app.set_random_station();
                            match play_station(app.current_station().unwrap(), sink.clone(), command_channel_tx.subscribe(), meta_channel_tx.clone()).await {
                                Ok(_) => {
                                    app.set_current_status(None);
                                }
                                Err(e) => {
                                    app.set_current_status(Some(format!("create stream failed {}", e)));
                                }
                            }
                        }
                        KeyCode::Char('p') => {
                            let command = Command::new_quit();
                            let _ = command_channel_tx.send(command);
                            // stop play task
                            command_sink.stop();
                        }
                        KeyCode::Char('r') => {
                            let command = Command::new_quit();
                            let _ = command_channel_tx.send(command);
                            // stop play task
                            command_sink.stop();
                            // restart current
                            match play_station(app.current_station().unwrap(), sink.clone(), command_channel_tx.subscribe(), meta_channel_tx.clone()).await {
                                Ok(_) => {
                                    app.set_current_status(None);
                                }
                                Err(e) => {
                                    app.set_current_status(Some(format!("create stream failed {}", e)));
                                }
                            }
                        }
                        KeyCode::Char('q') => {
                            command_sink.stop();
                            let command = Command::new_quit();
                            let _ = command_channel_tx.send(command);
                            break;
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    disable_raw_mode().unwrap();
    stdout().execute(LeaveAlternateScreen).unwrap();
}
