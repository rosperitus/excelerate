//! The OLE compound file, the container an xls lives in.
//!
//! A compound file is a FAT filesystem inside one file: fixed-size sectors, a
//! chain per stream, and a directory of named entries. Streams shorter than
//! 4096 bytes live in a second, finer filesystem — the mini stream — kept as
//! one ordinary stream of the root entry.
//!
//! Only reading is implemented, and only what a workbook needs: a stream by
//! name. Everything here parses untrusted input, so a chain that loops, a
//! sector past the end of the file and a size larger than the file itself are
//! all expected rather than exceptional.

/// The eight bytes every compound file starts with.
const SIGNATURE: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];

/// End of a sector chain.
const END_OF_CHAIN: u32 = 0xFFFF_FFFE;
/// A sector holding part of the allocation table itself.
const FAT_SECTOR: u32 = 0xFFFF_FFFD;
/// A free sector.
const FREE_SECTOR: u32 = 0xFFFF_FFFF;

/// One directory entry, 128 bytes.
const DIRECTORY_ENTRY_SIZE: usize = 128;

/// Largest stream this reader will assemble, the same cap the zip readers use.
const MAX_STREAM_SIZE: u64 = 128 * 1024 * 1024;

/// A parsed container: enough to pull streams out of it.
pub struct Ole<'a> {
    bytes: &'a [u8],
    sector_size: usize,
    mini_sector_size: usize,
    /// Streams shorter than this live in the mini stream.
    mini_cutoff: u32,
    fat: Vec<u32>,
    mini_fat: Vec<u32>,
    /// The directory, one entry per name.
    entries: Vec<Entry>,
}

/// One named entry of the directory.
struct Entry {
    name: String,
    start: u32,
    size: u64,
}

/// Reads a little-endian number out of a slice, `None` past its end.
fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

/// The same for four bytes.
fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

/// The same for eight.
fn u64_at(bytes: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(bytes.get(at..at + 8)?.try_into().ok()?))
}

impl<'a> Ole<'a> {
    /// Parses the header, the allocation tables and the directory.
    ///
    /// # Errors
    /// A message naming what did not hold; the caller wraps it in the error of
    /// its format.
    pub fn new(bytes: &'a [u8]) -> Result<Self, String> {
        if bytes.get(..8) != Some(&SIGNATURE) {
            return Err("not an OLE compound file".to_owned());
        }
        let shift = u16_at(bytes, 0x1E).unwrap_or(0);
        let mini_shift = u16_at(bytes, 0x20).unwrap_or(0);
        // A shift outside this range would mean a sector larger than the file
        // can hold, or one smaller than a directory entry.
        if !(7..=20).contains(&shift) || !(2..=12).contains(&mini_shift) {
            return Err(format!("sector size 2^{shift} is out of range"));
        }
        let sector_size = 1usize << shift;
        let mini_sector_size = 1usize << mini_shift;

        let mut ole = Self {
            bytes,
            sector_size,
            mini_sector_size,
            mini_cutoff: u32_at(bytes, 0x38).unwrap_or(4096),
            fat: Vec::new(),
            mini_fat: Vec::new(),
            entries: Vec::new(),
        };
        ole.read_fat()?;
        ole.read_mini_fat();
        ole.read_directory()?;
        Ok(ole)
    }

    /// The bytes of one sector.
    fn sector(&self, index: u32) -> Option<&'a [u8]> {
        // Sector 0 starts right after the 512-byte header, whatever the sector
        // size is.
        let start = (usize::try_from(index).ok()?)
            .checked_add(1)?
            .checked_mul(self.sector_size)?;
        self.bytes.get(start..start.checked_add(self.sector_size)?)
    }

    /// Every sector of a chain, in order, stopping at its end, at a loop or at
    /// a sector that is not there.
    fn chain(table: &[u32], mut sector: u32) -> Vec<u32> {
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        while sector != END_OF_CHAIN && sector != FREE_SECTOR && sector != FAT_SECTOR {
            if !seen.insert(sector) || out.len() > table.len() {
                break;
            }
            out.push(sector);
            match table.get(sector as usize) {
                Some(&next) => sector = next,
                None => break,
            }
        }
        out
    }

    /// Reads the file allocation table, whose own sectors are listed in the
    /// header and, past the first 109 of them, in a chain of their own.
    fn read_fat(&mut self) -> Result<(), String> {
        let count = u32_at(self.bytes, 0x2C).unwrap_or(0) as usize;
        let mut sectors: Vec<u32> = (0..109)
            .filter_map(|i| u32_at(self.bytes, 0x4C + i * 4))
            .take_while(|&s| s != FREE_SECTOR && s != END_OF_CHAIN)
            .collect();

        // The rest of the list lives in the DIFAT chain, each of whose sectors
        // ends with a pointer to the next.
        let mut difat = u32_at(self.bytes, 0x44).unwrap_or(END_OF_CHAIN);
        let mut seen = std::collections::HashSet::new();
        while difat != END_OF_CHAIN && difat != FREE_SECTOR && sectors.len() < count {
            if !seen.insert(difat) {
                return Err("the DIFAT chain loops".to_owned());
            }
            let Some(data) = self.sector(difat) else {
                break;
            };
            let last = self.sector_size / 4 - 1;
            sectors.extend(
                (0..last)
                    .filter_map(|i| u32_at(data, i * 4))
                    .filter(|&s| s != FREE_SECTOR),
            );
            difat = u32_at(data, last * 4).unwrap_or(END_OF_CHAIN);
        }

        for sector in sectors {
            let Some(data) = self.sector(sector) else {
                continue;
            };
            self.fat
                .extend((0..self.sector_size / 4).filter_map(|i| u32_at(data, i * 4)));
        }
        if self.fat.is_empty() {
            return Err("the file has no allocation table".to_owned());
        }
        Ok(())
    }

    /// Reads the allocation table of the mini stream.
    fn read_mini_fat(&mut self) {
        let start = u32_at(self.bytes, 0x3C).unwrap_or(END_OF_CHAIN);
        for sector in Self::chain(&self.fat.clone(), start) {
            let Some(data) = self.sector(sector) else {
                continue;
            };
            self.mini_fat
                .extend((0..self.sector_size / 4).filter_map(|i| u32_at(data, i * 4)));
        }
    }

    /// Reads the directory: the name, first sector and size of each entry.
    fn read_directory(&mut self) -> Result<(), String> {
        let start = u32_at(self.bytes, 0x30).unwrap_or(END_OF_CHAIN);
        let fat = self.fat.clone();
        for sector in Self::chain(&fat, start) {
            let Some(data) = self.sector(sector) else {
                continue;
            };
            for entry in data.as_chunks::<DIRECTORY_ENTRY_SIZE>().0 {
                // An entry names its type at 0x42: 0 is an unused slot.
                if entry.get(0x42) == Some(&0) {
                    continue;
                }
                let length = usize::from(u16_at(entry, 0x40).unwrap_or(0)).min(64);
                let name: String = char::decode_utf16(
                    entry
                        .get(..length.saturating_sub(2))
                        .unwrap_or_default()
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|pair| u16::from_le_bytes([pair[0], pair[1]])),
                )
                .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
                .collect();
                self.entries.push(Entry {
                    name,
                    start: u32_at(entry, 0x74).unwrap_or(END_OF_CHAIN),
                    size: u64_at(entry, 0x78).unwrap_or(0),
                });
            }
        }
        if self.entries.is_empty() {
            return Err("the file has no directory".to_owned());
        }
        Ok(())
    }

    /// Assembles a chain of ordinary sectors into a stream of a given size.
    fn read_chain(&self, start: u32, size: u64) -> Vec<u8> {
        let size = usize::try_from(size.min(MAX_STREAM_SIZE)).unwrap_or(0);
        let mut out = Vec::with_capacity(size.min(self.bytes.len()));
        for sector in Self::chain(&self.fat, start) {
            match self.sector(sector) {
                Some(data) => out.extend_from_slice(data),
                None => break,
            }
            if out.len() >= size {
                break;
            }
        }
        out.truncate(size);
        out
    }

    /// The mini stream: the root entry's own stream, holding every short one.
    fn mini_stream(&self) -> Vec<u8> {
        // The root is the first entry of the directory.
        self.entries
            .first()
            .map(|root| self.read_chain(root.start, root.size))
            .unwrap_or_default()
    }

    /// One stream by name, `None` if the file has no such entry.
    pub fn stream(&self, name: &str) -> Option<Vec<u8>> {
        let entry = self.entries.iter().find(|e| e.name == name)?;
        let size = usize::try_from(entry.size.min(MAX_STREAM_SIZE)).ok()?;
        if entry.size >= u64::from(self.mini_cutoff) {
            return Some(self.read_chain(entry.start, entry.size));
        }
        let mini = self.mini_stream();
        let mut out = Vec::with_capacity(size.min(mini.len()));
        for sector in Self::chain(&self.mini_fat, entry.start) {
            let start = (sector as usize).checked_mul(self.mini_sector_size)?;
            match mini.get(start..start + self.mini_sector_size) {
                Some(data) => out.extend_from_slice(data),
                None => break,
            }
            if out.len() >= size {
                break;
            }
        }
        out.truncate(size);
        Some(out)
    }

    /// The names of every entry, for a reader that has to guess which stream
    /// holds the workbook.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|e| e.name.as_str())
    }
}
