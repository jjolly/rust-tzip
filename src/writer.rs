//! TorrentZip writer implementation.

use std::io::Write;

use crc32fast::Hasher as CrcHasher;
use flate2::write::DeflateEncoder;
use flate2::Compression;

use crate::error::{Error, Result};
use crate::spec::{
    general_purpose_flag_for, make_torrentzip_comment, CentralDirHeader, EndOfCentralDir,
    LocalFileHeader,
};

/// Internal representation of a file entry being written.
///
/// Only the compressed bytes are retained; the uncompressed copy is dropped
/// as soon as it has been hashed and deflated, so an in-progress archive
/// costs one compressed copy per file instead of two full copies plus the
/// finished archive.
struct FileEntry {
    name: String,
    name_lower: String,
    crc32: u32,
    uncompressed_size: u32,
    compressed: Vec<u8>,
    general_flag: u16,
}

/// TorrentZip-compliant ZIP file writer.
///
/// This writer produces deterministic, byte-identical ZIP archives
/// following the TorrentZip specification.
pub struct TorrentZipWriter<W: Write> {
    writer: W,
    entries: Vec<FileEntry>,
    torrentzip_crc32: Option<u32>,
    finished: bool,
}

impl TorrentZipWriter<Vec<u8>> {
    /// Create a new TorrentZip writer that writes to a Vec<u8> buffer.
    pub fn new_vec() -> Self {
        Self::new(Vec::new())
    }

    /// Get the finished ZIP data.
    ///
    /// Panics if not finished.
    pub fn into_inner(self) -> Vec<u8> {
        assert!(self.finished, "TorrentZipWriter not finished");
        self.writer
    }
}

impl<W: Write> TorrentZipWriter<W> {
    /// Create a new TorrentZip writer.
    pub fn new(writer: W) -> Self {
        Self {
            writer,
            entries: Vec::new(),
            torrentzip_crc32: None,
            finished: false,
        }
    }

    /// Add a file to the archive.
    ///
    /// Note: Path separators are automatically normalized to forward slashes
    /// per the ZIP specification and TorrentZip requirements.
    pub fn add_file(&mut self, name: &str, data: &[u8]) -> Result<()> {
        if self.finished {
            return Err(Error::AlreadyFinished);
        }

        // Normalize path separators to forward slashes (ZIP spec requirement)
        // This ensures TorrentZip compatibility regardless of input source
        let normalized_name = name.replace('\\', "/");

        // Compute CRC32 of uncompressed data
        let mut crc_hasher = CrcHasher::new();
        crc_hasher.update(data);
        let crc32 = crc_hasher.finalize();

        // Compress with DEFLATE level 9
        let compressed = if data.is_empty() {
            vec![]
        } else {
            let mut encoder = DeflateEncoder::new(Vec::new(), Compression::best());
            encoder.write_all(data)?;
            encoder.finish()?
        };

        let entry = FileEntry {
            name: normalized_name.clone(),
            name_lower: normalized_name.to_lowercase(),
            crc32,
            uncompressed_size: data.len() as u32,
            compressed,
            // ASCII names keep the legacy trrntzip flag so archives stay
            // byte-identical; non-ASCII names set bit 11 (UTF-8 / EFS) so
            // standard ZIP tools decode them instead of assuming CP437.
            general_flag: general_purpose_flag_for(&normalized_name),
        };

        self.entries.push(entry);
        Ok(())
    }

    /// Finish writing the ZIP archive.
    pub fn finish(&mut self) -> Result<()> {
        if self.finished {
            return Err(Error::AlreadyFinished);
        }

        // Sort entries by lowercase filename
        self.entries.sort_by(|a, b| a.name_lower.cmp(&b.name_lower));

        // Stream the archive straight to the writer instead of assembling it
        // in a second full-archive buffer: local file headers and compressed
        // data go out in sorted order while their offsets are recorded, and
        // the central directory CRC32 is computed incrementally as those
        // bytes stream out. The per-entry central-directory lookup is gone
        // too (offsets were recorded in this same order).
        let mut out = std::io::BufWriter::with_capacity(1 << 20, &mut self.writer);
        let mut local_offsets: Vec<u32> = Vec::with_capacity(self.entries.len());
        let mut pos: u32 = 0;

        // Write local file headers + data
        for entry in &self.entries {
            local_offsets.push(pos);

            let header = LocalFileHeader::new_torrentzip(entry.name.len() as u16)
                .with_general_flag(entry.general_flag)
                .with_sizes(
                    entry.crc32,
                    entry.compressed.len() as u32,
                    entry.uncompressed_size,
                );

            out.write_all(&header.to_bytes())?;
            out.write_all(entry.name.as_bytes())?;
            out.write_all(&entry.compressed)?;

            pos += LocalFileHeader::SIZE as u32
                + entry.name.len() as u32
                + entry.compressed.len() as u32;
        }

        // Record central directory offset
        let cd_offset = pos;

        // Write central directory headers, hashing them as they stream out
        let mut cd_hasher = CrcHasher::new();
        let mut cd_size: u32 = 0;
        for (entry, &local_offset) in self.entries.iter().zip(&local_offsets) {
            let header = CentralDirHeader::new_torrentzip(entry.name.len() as u16, local_offset)
                .with_general_flag(entry.general_flag)
                .with_sizes(
                    entry.crc32,
                    entry.compressed.len() as u32,
                    entry.uncompressed_size,
                );

            let mut chunk = Vec::with_capacity(CentralDirHeader::SIZE + entry.name.len());
            chunk.extend_from_slice(&header.to_bytes());
            chunk.extend_from_slice(entry.name.as_bytes());
            cd_hasher.update(&chunk);
            cd_size += chunk.len() as u32;
            out.write_all(&chunk)?;
        }

        let tz_crc32 = cd_hasher.finalize();

        // Write EOCD
        let num_entries = self.entries.len() as u16;
        let eocd = EndOfCentralDir::new_torrentzip(num_entries, cd_size, cd_offset);
        out.write_all(&eocd.to_bytes())?;

        // Write TorrentZip comment
        let comment = make_torrentzip_comment(tz_crc32);
        out.write_all(&comment)?;
        out.flush()?;

        self.torrentzip_crc32 = Some(tz_crc32);
        self.finished = true;
        Ok(())
    }

    /// Get the TorrentZip CRC32 (available after finish()).
    pub fn torrentzip_crc32(&self) -> Option<u32> {
        self.torrentzip_crc32
    }

    /// Check if the writer has been finished.
    pub fn is_finished(&self) -> bool {
        self.finished
    }
}
