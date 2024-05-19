use std::io;
use std::io::{Read, Seek, Write};

/// Creates a [`StorageReader`] and [`StorageWriter`]
/// The reader and writer must track their position in the stream independently.
pub trait StorageProvider: Clone + Send {
    /// Source used to read from the underlying storage.
    type Reader: StorageReader;
    /// Handle that can write to the underlying storage.
    type Writer: StorageWriter;

    /// Turn the provider into a reader and writer.
    fn into_reader_writer(
        self,
        content_length: Option<u64>,
    ) -> io::Result<(Self::Reader, Self::Writer)>;
}

/// Trait used to read from a storage layer
pub trait StorageReader: Read + Seek + Send {}

impl<T> StorageReader for T where T: Read + Seek + Send {}

/// Handle for writing to the underlying storage layer.
pub trait StorageWriter: Write + Seek + Send + 'static {}

impl<T> StorageWriter for T where T: Write + Seek + Send + 'static {}
