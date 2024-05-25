extern crate core;

use std::{env, time};
use std::cmp::{min, PartialEq};
use std::error::Error;
use std::fmt::{Debug, Display};
use std::io::{Read, Seek, stdout, Write};
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crossterm::event;
use crossterm::event::Event::Key;
use crossterm::event::KeyCode;
use crossterm::ExecutableCommand;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use diesel::associations::HasTable;
use diesel::prelude::*;
use dotenvy::dotenv;
use futures_util::TryStreamExt;
use rand::Rng;
use ratatui::{Frame, Terminal};
use ratatui::prelude::{Backend, Color, Constraint, CrosstermBackend, Direction, Layout, Rect, Span, Style, Stylize, Text};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use rodio::{Decoder, Sink, Source};
use rodio::decoder::DecoderError;
use serde::{Deserialize, Serialize};
use stream_download::source::SourceStream;
use symphonia::core::conv::IntoSample;
use tokio::io::AsyncReadExt;
use tokio::sync::broadcast;
use tokio::sync::broadcast::{Receiver, Sender};
use tracing::{debug, info, Level, span};
use tracing_subscriber::fmt::writer::MakeWriterExt;

use crate::bounded::BoundedStorageProvider;
use crate::memory::MemoryStorageProvider;
use crate::model::{History, NewHistory};
use crate::stream::StreamDownload;

mod memory;
mod source;
mod stream;
mod bounded;
mod spy_decoder;
mod storage;
mod schema;
mod model;

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
        if index < self.list.len() {
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
    bitrate: u16,
    channels: u16,
    sample_rate: u32,
}

impl Default for MetaData {
    fn default() -> Self {
        MetaData { title: "".to_string(), bitrate: 0, channels: 0, sample_rate: 0 }
    }
}

impl MetaData {
    fn with_title(title: &str) -> Self {
        MetaData { title: title.to_string(), ..Self::default() }
    }

    fn with_decoder_config(channels: u16, sample_rate: u32) -> Self {
        MetaData { channels, sample_rate, ..Self::default() }
    }

    fn with_bitrate(bitrate: u16) -> Self {
        MetaData { bitrate, ..Self::default() }
    }

    fn title(&self) -> &str {
        self.title.as_str()
    }

    fn merge(&mut self, meta: &MetaData) -> MetaData {
        let mut merged_meta = self.clone();
        if self.title.is_empty() && !meta.title.is_empty() {
            merged_meta.title = meta.title().to_string();
        }
        if self.bitrate == 0 && meta.bitrate != 0 {
            merged_meta.bitrate = meta.bitrate;
        }
        if self.channels == 0 && meta.channels != 0 {
            merged_meta.channels = meta.channels;
        }
        if self.sample_rate == 0 && meta.sample_rate != 0 {
            merged_meta.sample_rate = meta.sample_rate;
        }
        merged_meta
    }

    fn decoder_config(&self) -> String {
        if self.channels != 0 {
            return format!("{}/{}/{}", self.sample_rate, self.channels, self.bitrate);
        }
        "".to_string()
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Action {
    Start,
    Pause,
    Resume,
    Quit,
}

impl Action {
    pub fn new_quit() -> Self {
        Self::Quit
    }
}

async fn play_station(station: RadioStation, sink: Arc<Sink>, command_channel_rx: Receiver<Action>, meta_channel_tx: Sender<MetaData>) -> anyhow::Result<()> {
    let span = span!(Level::DEBUG, "play_station");
    let _enter = span.enter();

    debug!("started");
    let storage_provider = BoundedStorageProvider::new(
        MemoryStorageProvider::new(),
        NonZeroUsize::new(MEMORY_BUFFER_SIZE).unwrap());
    let stream_download = StreamDownload::new(station.url(), storage_provider, command_channel_rx, meta_channel_tx.clone()).await?;

    debug!("wait for decoder");
    let handle = tokio::task::spawn_blocking(move || {
        let decoder = Decoder::new(stream_download)?;
        meta_channel_tx.send(MetaData::with_decoder_config(decoder.channels(), decoder.sample_rate())).unwrap();
        sink.append(decoder);
        debug!("decoder created");
        Ok::<(), DecoderError>(())
    });
    debug!("done");

    match handle.await {
        Ok(Ok(())) => { Ok(()) }
        Ok(Err(e)) => { Err(anyhow::anyhow!(e)) }
        Err(e) => { Err(anyhow::anyhow!(e)) }
    }
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

    pub fn set_current_meta(&mut self, meta: &MetaData) {
        let merged_meta = self.current_meta().unwrap().merge(meta);
        self.current_meta = Some(merged_meta);
    }

    pub fn current_meta(&self) -> Option<MetaData> {
        self.current_meta.clone()
    }

    pub fn clear_current_meta(&mut self) {
        self.current_meta = Some(MetaData::default());
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
        let lines = Text::from(format!("{}\n{}", cm.title(), cm.decoder_config()));
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

pub fn establish_connection() -> anyhow::Result<SqliteConnection> {
    dotenv().ok();

    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    Ok(SqliteConnection::establish(&database_url)?)
}

pub fn add_history(radio_station: RadioStation, meta_data: MetaData) -> anyhow::Result<()> {
    use crate::schema::history;

    let mut connection = establish_connection()?;
    let datetime = chrono::prelude::DateTime::<chrono::Local>::from(SystemTime::now());
    let timestamp_str = datetime.format("%a, %e %b %Y %H:%M").to_string();
    let new_history = NewHistory { station: radio_station.title(), title: meta_data.title(), timestamp: timestamp_str.as_str() };

    let find_history_items = find_history(meta_data.title(), timestamp_str.as_str())?;
    debug!("find history for {} {:?}", meta_data.title(), find_history_items);
    if find_history_items.is_empty() {
        debug!("add history entry = {:?}", new_history);

        let _ = diesel::insert_into(history::table)
            .values(&new_history)
            .execute(&mut connection);
    }

    Ok(())
}

pub fn get_history() -> anyhow::Result<Vec<History>> {
    use self::schema::history::dsl::*;

    let mut connection = establish_connection()?;
    Ok(history
        .select(History::as_select())
        .load::<History>(&mut connection)?)
}

pub fn find_history(meta_title: &str, timestamp_str: &str) -> anyhow::Result<Vec<History>> {
    use self::schema::history::dsl::*;

    let mut connection = establish_connection()?;
    Ok(history
        .filter(title.eq(meta_title))
        .select(History::as_select())
        .load::<History>(&mut connection)?)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // let file_appender = tracing_appender::rolling::hourly("logs", "prefix.log");
    // let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);
    let (non_blocking, _guard) = tracing_appender::non_blocking(std::io::stderr());
    tracing_subscriber::fmt()
        .with_max_level(Level::DEBUG)
        .with_writer(non_blocking)
        .init();

    // tracing_subscriber::fmt()
    //     .with_max_level(Level::DEBUG)
    //     .init();

    debug!("main");
    let (command_channel_tx, mut command_channel_rx) = broadcast::channel::<Action>(1);
    let (meta_channel_tx, mut meta_channel_rx) = broadcast::channel::<MetaData>(3);

    let (_stream, handle) = rodio::OutputStream::try_default()?;
    let sink = Arc::new(rodio::Sink::try_new(&handle).unwrap());

    let mut app = App::init();
    app.set_current_station_index(0);

    let command_sink = sink.clone();

    enable_raw_mode().unwrap();
    stdout().execute(EnterAlternateScreen).unwrap();
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout())).unwrap();

    for history in get_history().unwrap() {
        debug!("{:?}", history);
    }

    loop {
        if let Ok(meta_data) = meta_channel_rx.try_recv() {
            app.set_current_meta(&meta_data);
            if !meta_data.title.is_empty() {
                let _ = add_history(app.current_station().unwrap(), meta_data);
            }
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
                        KeyCode::Char('s') | KeyCode::Char('r') | KeyCode::Enter => {
                            // stop download
                            let action = Action::new_quit();
                            let _ = command_channel_tx.send(action);
                            // stop play task
                            command_sink.stop();
                            // start new
                            app.clear_current_meta();
                            match play_station(app.current_station().unwrap(), sink.clone(), command_channel_tx.subscribe(), meta_channel_tx.clone()).await {
                                Ok(_) => {
                                    app.set_current_status(None);
                                }
                                Err(e) => {
                                    app.set_current_status(Some(format!("Create stream failed {}", e)));
                                }
                            }
                        }
                        KeyCode::Char('p') => {
                            let action = Action::new_quit();
                            let _ = command_channel_tx.send(action);
                            // stop play task
                            command_sink.stop();
                            app.set_current_status(Some("Paused".to_string()));
                        }
                        KeyCode::Char('q') => {
                            command_sink.stop();
                            let action = Action::new_quit();
                            let _ = command_channel_tx.send(action);
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
    Ok(())
}
