use crate::metainfo::Metainfo;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub file: usize,
    pub offset: u64,
    pub length: u64,
}

#[derive(Debug)]
pub struct Storage {
    paths: Vec<PathBuf>,
    lengths: Vec<u64>,
    piece_length: u64,
    total: u64,
}

impl Storage {
    pub fn from_metainfo(meta: &Metainfo, output_dir: &Path) -> Storage {
        let mut paths = Vec::new();
        let mut lengths = Vec::new();
        for file in &meta.files {
            let mut path = output_dir.to_path_buf();
            path.push(&meta.name);
            for component in &file.path {
                path.push(component);
            }
            paths.push(path);
            lengths.push(file.length);
        }
        Storage {
            paths,
            lengths,
            piece_length: meta.piece_length,
            total: meta.total_length,
        }
    }

    pub fn paths(&self) -> &[PathBuf] {
        &self.paths
    }

    pub fn total(&self) -> u64 {
        self.total
    }

    pub fn piece_size(&self, index: u32) -> u64 {
        let start = index as u64 * self.piece_length;
        std::cmp::min(self.piece_length, self.total.saturating_sub(start))
    }

    pub fn segments(&self, offset: u64, length: u64) -> Vec<Segment> {
        let mut out = Vec::new();
        let mut remaining = length;
        let mut cursor = offset;
        let mut file_start = 0u64;
        for (index, file_length) in self.lengths.iter().enumerate() {
            let file_end = file_start + file_length;
            if remaining == 0 {
                break;
            }
            if cursor < file_end {
                let inside = cursor - file_start;
                let take = std::cmp::min(remaining, file_end - cursor);
                out.push(Segment {
                    file: index,
                    offset: inside,
                    length: take,
                });
                cursor += take;
                remaining -= take;
            }
            file_start = file_end;
        }
        out
    }

    pub fn allocate(&self) -> io::Result<()> {
        for (path, length) in self.paths.iter().zip(&self.lengths) {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            let file = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(false)
                .open(path)?;
            file.set_len(*length)?;
        }
        Ok(())
    }

    pub fn write_block(&self, piece: u32, begin: u32, data: &[u8]) -> io::Result<()> {
        let offset = piece as u64 * self.piece_length + begin as u64;
        if offset + data.len() as u64 > self.total {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "past end"));
        }
        let mut written = 0usize;
        for segment in self.segments(offset, data.len() as u64) {
            let mut file = OpenOptions::new()
                .write(true)
                .open(&self.paths[segment.file])?;
            file.seek(SeekFrom::Start(segment.offset))?;
            file.write_all(&data[written..written + segment.length as usize])?;
            written += segment.length as usize;
        }
        Ok(())
    }

    pub fn read_block(&self, piece: u32, begin: u32, length: u64) -> io::Result<Vec<u8>> {
        let offset = piece as u64 * self.piece_length + begin as u64;
        if offset + length > self.total {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "past end"));
        }
        let mut out = vec![0u8; length as usize];
        let mut read = 0usize;
        for segment in self.segments(offset, length) {
            let mut file = OpenOptions::new().read(true).open(&self.paths[segment.file])?;
            file.seek(SeekFrom::Start(segment.offset))?;
            file.read_exact(&mut out[read..read + segment.length as usize])?;
            read += segment.length as usize;
        }
        Ok(out)
    }

    pub fn read_piece(&self, piece: u32) -> io::Result<Vec<u8>> {
        self.read_block(piece, 0, self.piece_size(piece))
    }
}
