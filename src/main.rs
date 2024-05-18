extern crate core;

use std::io::Write;
use std::num::NonZeroUsize;
use std::process::exit;
use std::sync::Arc;
use std::time::Duration;

use futures_util::TryStreamExt;
use parking_lot::RwLock;
use rodio::Source;
use serde::{Deserialize, Serialize};
use stream_download::http::reqwest::Client;
use stream_download::source::SourceStream;
use tokio::time::sleep;

use crate::bounded::BoundedStorageProvider;
use crate::memory::MemoryStorageProvider;
use crate::spy_decoder::SpyDecoder;
use crate::storage::StorageProvider;

mod memory;
mod source;
mod storage;
mod bounded;
mod spy_decoder;


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

const STREAM_BUFFER_SIZE: usize = 1024 * 16;
const MEMORY_BUFFER_SIZE: usize = 1024 * 512;

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
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct MetaData {
    title: String,
}

impl MetaData {
    fn new() -> Self {
        MetaData { title: "".to_string() }
    }
}

#[tokio::main]
/*
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            main() ...
        })
 */
async fn main() {
    //
    // let mutex = Arc::new(Mutex::new(0));
    // tokio::spawn(task_loop_mutex_one(mutex.clone()));
    // tokio::spawn(task_loop_mutex_two(mutex.clone()));
    //
    // let (_tx, rx) = broadcast::channel(16);
    // let tx = Arc::new(_tx);
    // tokio::spawn(task_loop_two(rx));
    // tokio::spawn(task_loop_one(tx.clone()));
    // //
    // // let t = tokio::spawn(async move {
    // //     task_slow().await
    // // });
    //
    // let mut rx1 = tx.subscribe();
    // loop {
    //     sleep(Duration::from_millis(2000)).await;
    //     match rx1.recv().await {
    //         Ok(value) => {
    //             println!("main receiver {}", value);
    //         }
    //         Err(error) => { break; }
    //     }
    // }
    // // t.await.unwrap();


    let mut working_url = "".to_string();
    let mut station_list = RadioStationList { list: vec![] };
    station_list.add(RadioStation::new("FM4", "http://orf-live.ors-shoutcast.at/fm4-q2a"));
    station_list.add(RadioStation::new("MetalRock.FM", "http://cheetah.streemlion.com:2160/stream"));
    station_list.add(RadioStation::new("Terry Callaghan's Classic Alternative Channel", "http://s16.myradiostream.com:7304"));
    station_list.add(RadioStation::new("OE3", "http://orf-live.ors-shoutcast.at/oe3-q2a"));
    station_list.add(RadioStation::new("local", "http://192.168.1.70:8001/stream"));

    if let Some(station) = station_list.get("local") {
        working_url = station.url.to_string();
    } else {
        println!("no station match");
        exit(-1);
    }

    println!("working_url {}", working_url);

    let current_metadata = Arc::new(RwLock::new(MetaData::new()));
    let current_metadata_write = current_metadata.clone();

    let (_stream, handle) = rodio::OutputStream::try_default().unwrap();
    let sink = rodio::Sink::try_new(&handle).unwrap();

    // let stream = HttpStream::<Client>::create(working_url.parse().unwrap()).await.unwrap();
    //
    // println!("content type={:?}", stream.content_type());
    // let bitrate: u64 = stream.header("Icy-Br").unwrap().parse().unwrap();
    // println!("bitrate={bitrate}");
    //
    // // buffer 5 seconds of audio
    // // bitrate (in kilobits) / bits per byte * bytes per kilobyte * 5 seconds
    // let prefetch_bytes = bitrate / 8 * 1024 * 5;
    // println!("prefetch bytes={prefetch_bytes}");
    //
    // let reader =
    //     StreamDownload::from_stream(stream, BoundedStorageProvider::new(
    //         MemoryStorageProvider,
    //         // be liberal with the buffer size, you need to make sure it holds enough space to
    //         // prevent any out-of-bounds reads
    //         NonZeroUsize::new(128 * 1024).unwrap(),
    //     )/*TempStorageProvider::new()*/, Settings::default().prefetch_bytes(prefetch_bytes))
    //         .await.unwrap();
    //
    // sink.append(rodio::Decoder::new_mp3(reader).unwrap());
    //
    // let handle = tokio::task::spawn_blocking(move || {
    //     sink.sleep_until_end();
    // });
    // handle.await.unwrap();

    // blatant kang and simplified from stream-download for learning purposes how
    // to create a streaming source for rodio
    // stripped down to bounded memory only
    let storage_provider = BoundedStorageProvider::new(
        MemoryStorageProvider::new(),
        NonZeroUsize::new(MEMORY_BUFFER_SIZE).unwrap());
    let (reader, mut writer) = storage_provider.into_reader_writer(None).unwrap();

    let stream_task = tokio::spawn(async move {
        let client = Client::new();
        let mut data_buffer = Vec::new();
        let mut data_buffer_index = 0;

        let mut response = match client.get(working_url).header("icy-metadata", "1").send().await {
            Ok(t) => t,
            Err(_) => {
                return;
            }
        };
        if let Some(header_value) = response.headers().get("content-type") {
            if header_value.to_str().unwrap_or_default() != "audio/mpeg" {
                return;
            }
        } else {
            return;
        }
        for header in response.headers().iter() {
            if header.0.to_string().starts_with("icy") { println!("{} {}", header.0, header.1.to_str().unwrap_or_default().parse::<String>().unwrap_or_default()) }
        }

        let meta_interval: usize = if let Some(header_value) = response.headers().get("icy-metaint") {
            header_value.to_str().unwrap_or_default().parse().unwrap_or_default()
        } else {
            0
        };
        println!("meta_interval={meta_interval}");

        let bitrate: usize = if let Some(header_value) = response.headers().get("Icy-Br") {
            header_value.to_str().unwrap_or_default().parse().unwrap_or_default()
        } else {
            0
        };
        println!("bitrate={bitrate}");

        // buffer 5 seconds of audio
        // bitrate (in kilobits) / bits per byte * bytes per kilobyte * 5 seconds
        let prefetch_bytes = bitrate / 8 * 1024 * 5;
        println!("prefetch bytes={prefetch_bytes}");

        let mut counter = meta_interval;
        let mut awaiting_metadata_size = false;
        let mut metadata_size: u8 = 0;
        let mut awaiting_metadata = false;
        let mut metadata: Vec<u8> = Vec::with_capacity(STREAM_BUFFER_SIZE);

        let mut http_stream = response.bytes_stream();
        while let Some(chunk) = http_stream.try_next().await.unwrap() {
            for byte in &chunk {
                if meta_interval != 0 {
                    if awaiting_metadata_size {
                        awaiting_metadata_size = false;
                        metadata_size = *byte * 16;
                        if metadata_size == 0 {
                            counter = meta_interval;
                        } else {
                            awaiting_metadata = true;
                        }
                    } else if awaiting_metadata {
                        metadata.push(*byte);
                        metadata_size = metadata_size.saturating_sub(1);
                        if metadata_size == 0 {
                            awaiting_metadata = false;
                            let metadata_string = std::str::from_utf8(&metadata).unwrap_or("");
                            if !metadata_string.is_empty() {
                                const STREAM_TITLE_KEYWORD: &str = "StreamTitle='";
                                if let Some(index) = metadata_string.find(STREAM_TITLE_KEYWORD) {
                                    let left_index = index + 13;
                                    let stream_title_substring = &metadata_string[left_index..];
                                    if let Some(right_index) = stream_title_substring.find('\'') {
                                        let trimmed_song_title = &stream_title_substring[..right_index];
                                        current_metadata_write.write().title = trimmed_song_title.to_string();
                                    }
                                }
                            }
                            metadata.clear();
                            counter = meta_interval;
                        }
                    } else {
                        data_buffer.push(*byte);
                        data_buffer_index += 1;

                        if data_buffer_index >= STREAM_BUFFER_SIZE {
                            writer.write(data_buffer.as_slice()).unwrap();
                            writer.inner.handle.position_reached.notify_position_reached();
                            data_buffer_index = 0;
                            data_buffer.clear();
                        }
                        counter = counter.saturating_sub(1);
                        if counter == 0 {
                            awaiting_metadata_size = true;
                        }
                    }
                } else {
                    data_buffer.push(*byte);
                    data_buffer_index += 1;

                    if data_buffer_index >= STREAM_BUFFER_SIZE {
                        writer.write(data_buffer.as_slice()).unwrap();
                        writer.inner.handle.position_reached.notify_position_reached();
                        data_buffer_index = 0;
                        data_buffer.clear();
                    }
                }
            }
        }
    });

    let play_task = tokio::spawn(async move {
        reader.inner.handle.wait_for_requested_position();

        let decoder = SpyDecoder::new(reader).unwrap();
        sink.append(decoder);

        let handle = tokio::task::spawn_blocking(move || {
            sink.sleep_until_end();
        });
    });

    let status_task = tokio::spawn(async move {
        loop {
            println!("Now playing: {}", current_metadata.read().title);
            sleep(Duration::from_secs(5)).await;
        }
    });

    stream_task.await.unwrap();
}
