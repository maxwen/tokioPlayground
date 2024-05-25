use std::io::{Read, Seek};
use std::time::Duration;

use rodio::{Decoder, Source};
use rodio::decoder::DecoderError;

pub struct SpyDecoder<R> where R: Read + Seek
{
    inner: Decoder<R>,
    stats_period: usize,
    stats_index: usize,
    stats_collect: bool,
}

#[allow(dead_code)]
impl<R> SpyDecoder<R>
    where
        R: Read + Seek + Send + Sync + 'static,
{
    pub fn new(data: R) -> Result<SpyDecoder<R>, DecoderError> {
        let inner = Decoder::new(data)?;
        Ok(Self {
            inner,
            stats_period: 1024 * 32,
            stats_index: 0,
            stats_collect: false,
        })
    }
}

impl<R> Iterator for SpyDecoder<R>
    where R: Read + Seek {
    type Item = i16;
    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        let sample = self.inner.next();
        if self.stats_collect {
            if self.stats_index >= self.stats_period {
                if sample.is_some() {
                    println!("{}", self.inner.sample_rate());
                }
                self.stats_index = 0;
            } else {
                self.stats_index += 1;
            }
        }
        sample
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<R> Source for SpyDecoder<R>
    where R: Read + Seek {
    fn current_frame_len(&self) -> Option<usize> {
        self.inner.current_frame_len()
    }

    fn channels(&self) -> u16 {
        self.inner.channels()
    }

    fn sample_rate(&self) -> u32 {
        self.inner.sample_rate()
    }

    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }
}