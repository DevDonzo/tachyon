use memmap2::{Mmap, MmapOptions};
use rayon::prelude::*;
use std::path::{Path, PathBuf};
use tachyon_core::{ByteOffset, ByteRange, LineNumber, Result, TachyonError};

pub const DEFAULT_CHUNK_SIZE: usize = 16 * 1024 * 1024;

pub struct MappedFile {
    path: PathBuf,
    mmap: Mmap,
}

impl MappedFile {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let file = std::fs::File::open(&path)?;
        let mmap = {
            // SAFETY: We create a read-only mapping and expose it as immutable bytes.
            unsafe { MmapOptions::new().map(&file)? }
        };
        Ok(Self { path, mmap })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn len(&self) -> u64 {
        self.mmap.len() as u64
    }

    pub fn is_empty(&self) -> bool {
        self.mmap.is_empty()
    }

    pub fn bytes(&self) -> &[u8] {
        &self.mmap
    }

    pub fn slice(&self, range: ByteRange) -> Result<&[u8]> {
        if range.end.0 > self.len() {
            return Err(TachyonError::InvalidByteRange {
                start: range.start.0,
                end: range.end.0,
            });
        }
        Ok(&self.mmap[range.start.0 as usize..range.end.0 as usize])
    }

    pub fn build_newline_index(&self, chunk_size: usize) -> NewlineIndex {
        NewlineIndex::from_bytes_parallel(self.bytes(), chunk_size)
    }
}

#[derive(Debug, Clone)]
pub struct NewlineIndex {
    newline_offsets: Vec<u64>,
    file_len: u64,
}

impl NewlineIndex {
    pub fn from_bytes_parallel(bytes: &[u8], chunk_size: usize) -> Self {
        let chunk_size = chunk_size.max(1);
        let per_chunk: Vec<Vec<u64>> = bytes
            .par_chunks(chunk_size)
            .enumerate()
            .map(|(chunk_idx, chunk)| {
                let base = (chunk_idx * chunk_size) as u64;
                chunk
                    .iter()
                    .enumerate()
                    .filter_map(|(idx, byte)| (*byte == b'\n').then_some(base + idx as u64))
                    .collect()
            })
            .collect();

        let total_newlines = per_chunk.iter().map(Vec::len).sum();
        let mut newline_offsets = Vec::with_capacity(total_newlines);
        for mut chunk in per_chunk {
            newline_offsets.append(&mut chunk);
        }

        Self {
            newline_offsets,
            file_len: bytes.len() as u64,
        }
    }

    pub fn newline_count(&self) -> u64 {
        self.newline_offsets.len() as u64
    }

    pub fn file_len(&self) -> u64 {
        self.file_len
    }

    pub fn total_lines(&self) -> u64 {
        if self.file_len == 0 {
            1
        } else {
            self.newline_count() + 1
        }
    }

    pub fn line_to_byte(&self, line: LineNumber) -> Result<ByteOffset> {
        if line.0 >= self.total_lines() {
            return Err(TachyonError::LineOutOfBounds {
                requested: line.0,
                total: self.total_lines(),
            });
        }

        if line.0 == 0 {
            return Ok(ByteOffset(0));
        }

        Ok(ByteOffset(self.newline_offsets[(line.0 - 1) as usize] + 1))
    }

    pub fn byte_to_line(&self, byte_offset: ByteOffset) -> LineNumber {
        if self.file_len == 0 {
            return LineNumber(0);
        }

        if byte_offset.0 >= self.file_len {
            return LineNumber(self.total_lines() - 1);
        }

        let line_idx = self
            .newline_offsets
            .partition_point(|offset| *offset < byte_offset.0);
        LineNumber(line_idx as u64)
    }

    pub fn line_byte_range(&self, line: LineNumber) -> Result<ByteRange> {
        let start = self.line_to_byte(line)?;
        let end = if line.0 + 1 < self.total_lines() {
            let next_start = self.line_to_byte(LineNumber(line.0 + 1))?;
            ByteOffset(next_start.0.saturating_sub(1))
        } else {
            ByteOffset(self.file_len)
        };

        ByteRange::new(start, end)
    }
}

pub fn open_and_index(
    path: impl AsRef<Path>,
    chunk_size: usize,
) -> Result<(MappedFile, NewlineIndex)> {
    let mapped = MappedFile::open(path)?;
    let index = mapped.build_newline_index(chunk_size);
    Ok((mapped, index))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn empty_file_has_single_virtual_line() {
        let index = NewlineIndex::from_bytes_parallel(b"", 8);
        assert_eq!(index.file_len(), 0);
        assert_eq!(index.newline_count(), 0);
        assert_eq!(index.total_lines(), 1);
        assert_eq!(index.line_to_byte(LineNumber(0)).unwrap(), ByteOffset(0));
        assert_eq!(index.byte_to_line(ByteOffset(0)), LineNumber(0));
    }

    #[test]
    fn indexes_newlines_across_chunks() {
        let data = b"a\nbb\nccc\n";
        let index = NewlineIndex::from_bytes_parallel(data, 2);
        assert_eq!(index.newline_count(), 3);
        assert_eq!(index.total_lines(), 4);
        assert_eq!(index.line_to_byte(LineNumber(0)).unwrap(), ByteOffset(0));
        assert_eq!(index.line_to_byte(LineNumber(1)).unwrap(), ByteOffset(2));
        assert_eq!(index.line_to_byte(LineNumber(2)).unwrap(), ByteOffset(5));
        assert_eq!(index.line_to_byte(LineNumber(3)).unwrap(), ByteOffset(9));
    }

    #[test]
    fn byte_and_line_conversion_roundtrip() {
        let data = b"alpha\nbeta\ngamma";
        let index = NewlineIndex::from_bytes_parallel(data, 4);
        assert_eq!(index.byte_to_line(ByteOffset(0)), LineNumber(0));
        assert_eq!(index.byte_to_line(ByteOffset(5)), LineNumber(0));
        assert_eq!(index.byte_to_line(ByteOffset(6)), LineNumber(1));
        assert_eq!(index.byte_to_line(ByteOffset(15)), LineNumber(2));
        assert_eq!(index.byte_to_line(ByteOffset(16)), LineNumber(2));
    }

    #[test]
    fn line_ranges_exclude_newline_separator() {
        let data = b"alpha\nbeta\ngamma\n";
        let index = NewlineIndex::from_bytes_parallel(data, 3);
        assert_eq!(
            index.line_byte_range(LineNumber(0)).unwrap(),
            ByteRange::new(ByteOffset(0), ByteOffset(5)).unwrap()
        );
        assert_eq!(
            index.line_byte_range(LineNumber(1)).unwrap(),
            ByteRange::new(ByteOffset(6), ByteOffset(10)).unwrap()
        );
        assert_eq!(
            index.line_byte_range(LineNumber(3)).unwrap(),
            ByteRange::new(ByteOffset(data.len() as u64), ByteOffset(data.len() as u64)).unwrap()
        );
    }

    #[test]
    fn mapped_file_opens_and_slices() {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(b"line-1\nline-2\n").unwrap();
        let mapped = MappedFile::open(file.path()).unwrap();
        assert_eq!(mapped.path(), file.path());
        assert!(!mapped.is_empty());
        let slice = mapped
            .slice(ByteRange::new(ByteOffset(0), ByteOffset(6)).unwrap())
            .unwrap();
        assert_eq!(slice, b"line-1");
    }
}
