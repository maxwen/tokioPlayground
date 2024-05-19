use std::cmp::PartialEq;
use std::io::{Read, Seek, Write};
use std::num::NonZeroUsize;
use std::sync::Arc;

use futures_util::TryStreamExt;
use parking_lot::RwLock;
use rand::Rng;
use rodio::{Decoder, Sink, Source};
use serde::{Deserialize, Serialize};
use stream_download::source::SourceStream;
use symphonia::core::meta::MetadataRevision;
use tokio::io::AsyncReadExt;
use tokio::sync::broadcast;
use tokio::sync::broadcast::{Receiver, Sender};
use tokio::sync::broadcast::error::RecvError;

use crate::bounded::BoundedStorageProvider;
use crate::memory::MemoryStorageProvider;
use crate::stream::StreamDownload;

mod memory;
mod source;
mod stream;
mod bounded;
mod spy_decoder;
mod storage;


// async fn task_loop_one(tx: Arc<broadcast::Sender<i32>>) {
//     let mut count: i32 = 0;
//     loop {
//         println!("task_loop_one");
//         tx.send(count).unwrap();
//         count += 1;
//         sleep(Duration::from_millis(200)).await;
//     }
// }
//
// async fn task_loop_two(mut rx: broadcast::Receiver<i32>) {
//     loop {
//         match rx.recv().await {
//             Ok(value) => {
//                 println!("task_loop_two receiver {}", value);
//             }
//             Err(error) => { break; }
//         }
//     }
// }
// async fn task_loop_mutex_one(mut mutex: Arc<Mutex<i32>>) {
//     loop {
//         println!("task_loop_mutex_one before");
//         let guard = mutex.lock().await;
//         sleep(Duration::from_millis(200)).await;
//         println!("task_loop_mutex_one after");
//         drop(guard);
//     }
// }
//
// async fn task_loop_mutex_two(mut mutex: Arc<Mutex<i32>>) {
//     loop {
//         println!("task_loop_mutex_two before");
//         let guard = mutex.lock().await;
//         sleep(Duration::from_millis(2000)).await;
//         println!("task_loop_mutex_two after");
//         drop(guard);
//     }
// }
//
// async fn task_slow() {
//     println!("task_slow start");
//     sleep(Duration::from_millis(2000)).await;
//     println!("task_slow stop");
// }

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
}

impl RadioStationList {
    fn add(&mut self, station: RadioStation) {
        self.list.push(station);
    }

    fn get(self, name: &str) -> Option<RadioStation> {
        if let Some(station) = self.list.iter().find(|station| station.title == name) {
            return Some(station.clone());
        }
        None
    }

    fn get_random(self) -> Option<RadioStation> {
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

async fn play_station(station: RadioStation, sink: Arc<Sink>, command_channel_rx: Receiver<Command>, meta_channel_tx: Sender<MetaData>) {
    let storage_provider = BoundedStorageProvider::new(
        MemoryStorageProvider::new(),
        NonZeroUsize::new(MEMORY_BUFFER_SIZE).unwrap());
    match StreamDownload::new(station.url, storage_provider, command_channel_rx, meta_channel_tx).await {
        Ok(stream_download) => {
            let play_sink = sink.clone();
            play_sink.play();

            tokio::task::spawn(async move {
                println!("play_task ready");
                match Decoder::new(stream_download) {
                    Ok(decoder) => {
                        play_sink.append(decoder);
                    }
                    Err(_) => {}
                }

                let handle = tokio::task::spawn_blocking(move || {
                    play_sink.sleep_until_end();
                });
                handle.await.unwrap();
            });
        }
        Err(e) => {
            println!("create stream failed {}", e)
        }
    }
}

#[tokio::main]
async fn main() {
    let (command_channel_tx, mut command_channel_rx) = broadcast::channel::<Command>(1);
    let (meta_channel_tx, mut meta_channel_rx) = broadcast::channel::<MetaData>(1);

    let (_stream, handle) = rodio::OutputStream::try_default().unwrap();
    let sink = Arc::new(rodio::Sink::try_new(&handle).unwrap());

    let mut station_list = RadioStationList { list: vec![] };
    station_list.add(RadioStation::new("FM4", "http://orf-live.ors-shoutcast.at/fm4-q2a"));
    station_list.add(RadioStation::new("MetalRock.FM", "http://cheetah.streemlion.com:2160/stream"));
    station_list.add(RadioStation::new("Terry Callaghan's Classic Alternative Channel", "http://s16.myradiostream.com:7304"));
    station_list.add(RadioStation::new("OE3", "http://orf-live.ors-shoutcast.at/oe3-q2a"));
    station_list.add(RadioStation::new("local", "http://192.168.1.70:8001/stream"));

    tokio::task::spawn(async move {
        loop {
            match meta_channel_rx.recv().await {
                Ok(meta) => {println!("now playing {}", meta.title)}
                Err(_) => {}
            }
        }
    });

    let command_sink = sink.clone();
    loop {
        let mut buf = String::new();
        std::io::stdin().read_line(&mut buf).unwrap();
        let c = buf.chars().nth(0).unwrap();
        match c {
            's' => {
                // stop download
                let command = Command::new_quit();
                let _ = command_channel_tx.send(command);
                // stop play task
                command_sink.clear();
                play_station(station_list.clone().get_random().unwrap(), sink.clone(), command_channel_tx.subscribe(), meta_channel_tx.clone()).await;
            }
            'p' => {
                // TODO
                command_sink.pause();
            }
            'r' => {
                // TODO
                command_sink.play();
            }
            'q' => {
                let command = Command::new_quit();
                let _ = command_channel_tx.send(command);
                break;
            }
            _ => {}
        }
    }
}
