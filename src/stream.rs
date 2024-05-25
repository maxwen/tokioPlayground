use std::{fmt, io};
use std::error::Error;
use std::io::{Read, Seek, SeekFrom, Write};

use bytes::Buf;
use futures_util::StreamExt;
use reqwest::{Client, Url};
use stream_download::http::ClientResponse;
use tokio::sync::broadcast::{Receiver, Sender};
use tracing::{debug, Level, span};

use crate::{Action, MetaData};
use crate::source::{PositionReached, SourceHandle};
use crate::storage::{StorageProvider, StorageWriter};

pub struct StreamDownload<P: StorageProvider> {
    reader: P::Reader,
    handle: SourceHandle,
}

#[derive(Debug, Clone)]
pub struct StreamError {
    msg: String,
}

impl StreamError {
    pub fn new(msg: &str) -> Self {
        Self {
            msg: msg.to_string()
        }
    }
}

impl Error for StreamError {}

impl fmt::Display for StreamError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.msg)
    }
}

impl<P: StorageProvider> StreamDownload<P> {
    // with and without anyhow
    // anyhow::Result<Self>
    // Result<Self, Box<dyn Error>>
    pub async fn new(working_url: &str, storage_provider: P, mut command_channel_rx: Receiver<Action>,
                     meta_channel_tx: Sender<MetaData>) -> anyhow::Result<Self> {
        let span = span!(Level::DEBUG, "StreamDownload");
        let _enter = span.enter();

        let (reader, mut writer) = storage_provider.into_reader_writer(None)?;

        let client = Client::new();
        let mut response = client.get(Url::parse(working_url).unwrap()).header("icy-metadata", "1").send().await?;

        let headers = response.headers().clone();
        let mut stream = response.stream();

        if let Some(header_value) = headers.get("content-type") {
            if header_value.to_str().unwrap_or_default() != "audio/mpeg" {
                return Err(StreamError::new("wrong content type").into());
            }
        } else {
            return Err(StreamError::new("wrong content type").into());
        }

        for header in headers.iter().filter(|header| header.0.as_str().starts_with("icy")).into_iter() {
            debug!("{} {}", header.0, header.1.to_str().unwrap_or_default().parse::<String>().unwrap_or_default())
        }

        let meta_interval: usize = if let Some(header_value) = headers.get("icy-metaint") {
            header_value.to_str().unwrap_or_default().parse().unwrap_or_default()
        } else {
            0
        };

        let bitrate: usize = if let Some(header_value) = headers.get("icy-br") {
            header_value.to_str().unwrap_or_default().parse().unwrap_or_default()
        } else {
            128
        };

        meta_channel_tx.send(MetaData::with_bitrate(bitrate as u16)).unwrap();

        // buffer 5 seconds of audio
        // bitrate (in kilobits) / bits per byte * bytes per kilobyte * 5 seconds
        let prefetch_bytes = bitrate / 8 * 1024 * 5;
        debug!("prefetch_bytes = {}", prefetch_bytes);

        let mut source = Source::new(writer, prefetch_bytes as u64);
        let handle = source.source_handle();

        tokio::spawn(async move {
            let mut counter = meta_interval;
            let mut awaiting_metadata_size = false;
            let mut metadata_size: u8 = 0;
            let mut awaiting_metadata = false;
            let mut metadata: Vec<u8> = Vec::with_capacity(1024);

            'outer: loop {
                if let Ok(chunk) = stream.next().await.transpose() {
                    if !command_channel_rx.is_empty() {
                        if let Ok(action) = command_channel_rx.recv().await {
                            match action {
                                _ => { break 'outer; }
                            }
                        }
                    }
                    if meta_interval != 0 {
                        for byte in chunk.unwrap() {
                            if awaiting_metadata_size {
                                awaiting_metadata_size = false;
                                metadata_size = byte * 16;
                                if metadata_size == 0 {
                                    counter = meta_interval;
                                } else {
                                    awaiting_metadata = true;
                                }
                            } else if awaiting_metadata {
                                metadata.push(byte);
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
                                                meta_channel_tx.send(MetaData::with_title(trimmed_song_title)).unwrap();
                                            }
                                        }
                                    }
                                    metadata.clear();
                                    counter = meta_interval;
                                }
                            } else {
                                let _ = source.handle_bytes(&[byte]).await;
                                counter = counter.saturating_sub(1);
                                if counter == 0 {
                                    awaiting_metadata_size = true;
                                }
                            }
                        }
                    } else {
                        let _ = source.handle_bytes(chunk.unwrap().chunk()).await;
                    }
                }
            }
        });

        Ok(Self {
            reader,
            handle,
        })
    }
}

impl<P: StorageProvider> Read for StreamDownload<P> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.handle.wait_for_requested_position();
        self.reader.read(buf)
    }
}

impl<P: StorageProvider> Seek for StreamDownload<P> {
    fn seek(&mut self, relative_position: SeekFrom) -> io::Result<u64> {
        self.handle.wait_for_requested_position();

        self.reader
            .seek(relative_position)
    }
}

pub(crate) struct Source<W: StorageWriter> {
    writer: W,
    position_reached: PositionReached,
    prefetch_bytes: u64,
}

impl<H: StorageWriter> Source<H> {
    pub(crate) fn new(writer: H, prefetch_bytes: u64) -> Self {
        Self {
            writer,
            position_reached: PositionReached::default(),
            prefetch_bytes,
        }
    }

    pub(crate) fn source_handle(&self) -> SourceHandle {
        SourceHandle {
            position_reached: self.position_reached.clone(),
        }
    }

    async fn handle_bytes(&mut self, buf: &[u8]) -> io::Result<usize> {
        let res = self.writer.write(buf);
        let new_position = self.writer.stream_position().unwrap();

        if new_position >= self.prefetch_bytes {
            self.position_reached.notify_position_reached();
        }
        res
    }
}


