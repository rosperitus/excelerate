//! Writing the OLE compound file an xls lives in.
//!
//! The container is a FAT filesystem, and this writes the smallest one that
//! holds a single named stream: the data sectors, one directory sector naming
//! the root and the stream, and the allocation table listing both.
//!
//! Short streams belong in the mini stream, a second filesystem inside the root
//! entry's own stream. Rather than implement it for the one case where a
//! workbook is under 4 KB, such a stream is padded up to the cutoff — the
//! records past its end read as nothing, and the reader stops at the `EOF`
//! record long before.

/// Every compound file starts with these eight bytes.
const SIGNATURE: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
/// The only sector size this writer uses.
const SECTOR: usize = 512;
/// Streams shorter than this belong in the mini stream.
const MINI_CUTOFF: usize = 4096;
/// End of a sector chain.
const END_OF_CHAIN: u32 = 0xFFFF_FFFE;
/// A sector holding part of the allocation table.
const FAT_SECTOR: u32 = 0xFFFF_FFFD;
/// A free sector, and the empty directory pointer.
const FREE: u32 = 0xFFFF_FFFF;

/// Wraps one named stream in a compound file.
#[must_use]
pub fn container(name: &str, stream: &[u8]) -> Vec<u8> {
    let mut data = stream.to_vec();
    data.resize(data.len().max(MINI_CUTOFF), 0);
    let declared = data.len();
    // Sectors are whole, so the last one is padded out.
    let padding = (SECTOR - data.len() % SECTOR) % SECTOR;
    data.resize(data.len() + padding, 0);
    let data_sectors = data.len() / SECTOR;

    // The directory is one sector: four entries of 128 bytes, of which two are
    // used. The table has to describe itself, so its size is found by trying.
    let mut fat_sectors = 1;
    loop {
        let total = data_sectors + 1 + fat_sectors;
        let needed = total.div_ceil(SECTOR / 4);
        if needed <= fat_sectors {
            break;
        }
        fat_sectors = needed;
    }
    let directory_sector = data_sectors;
    let first_fat_sector = directory_sector + 1;

    // The allocation table: the stream's chain, then the directory, then the
    // sectors the table itself sits in.
    let mut fat = vec![FREE; fat_sectors * (SECTOR / 4)];
    for (i, entry) in fat.iter_mut().enumerate().take(data_sectors) {
        *entry = u32::try_from(i + 1).unwrap_or(END_OF_CHAIN);
    }
    if data_sectors > 0 {
        fat[data_sectors - 1] = END_OF_CHAIN;
    }
    fat[directory_sector] = END_OF_CHAIN;
    for i in 0..fat_sectors {
        fat[first_fat_sector + i] = FAT_SECTOR;
    }

    let mut out = Vec::with_capacity((1 + data_sectors + 1 + fat_sectors) * SECTOR);
    out.extend_from_slice(&header(fat_sectors, first_fat_sector, directory_sector));
    out.extend_from_slice(&data);
    out.extend_from_slice(&directory(name, declared));
    for entry in fat {
        out.extend_from_slice(&entry.to_le_bytes());
    }
    out
}

/// The 512-byte header, including the list of table sectors.
fn header(fat_sectors: usize, first_fat_sector: usize, directory_sector: usize) -> [u8; SECTOR] {
    let mut head = [0u8; SECTOR];
    head[..8].copy_from_slice(&SIGNATURE);
    // Minor and major version, and the little-endian byte order mark.
    head[0x18..0x1A].copy_from_slice(&0x003Eu16.to_le_bytes());
    head[0x1A..0x1C].copy_from_slice(&0x0003u16.to_le_bytes());
    head[0x1C..0x1E].copy_from_slice(&0xFFFEu16.to_le_bytes());
    // Sector size 2^9, mini sector size 2^6.
    head[0x1E..0x20].copy_from_slice(&9u16.to_le_bytes());
    head[0x20..0x22].copy_from_slice(&6u16.to_le_bytes());
    let fat_count = u32::try_from(fat_sectors).unwrap_or(1);
    head[0x2C..0x30].copy_from_slice(&fat_count.to_le_bytes());
    let directory = u32::try_from(directory_sector).unwrap_or(0);
    head[0x30..0x34].copy_from_slice(&directory.to_le_bytes());
    head[0x38..0x3C].copy_from_slice(&u32::try_from(MINI_CUTOFF).unwrap_or(4096).to_le_bytes());
    // No mini stream and no extra table sectors: everything fits in the header.
    head[0x3C..0x40].copy_from_slice(&END_OF_CHAIN.to_le_bytes());
    head[0x40..0x44].copy_from_slice(&0u32.to_le_bytes());
    head[0x44..0x48].copy_from_slice(&END_OF_CHAIN.to_le_bytes());
    head[0x48..0x4C].copy_from_slice(&0u32.to_le_bytes());
    for i in 0..109 {
        let at = 0x4C + i * 4;
        let value = if i < fat_sectors {
            u32::try_from(first_fat_sector + i).unwrap_or(FREE)
        } else {
            FREE
        };
        head[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }
    head
}

/// The directory sector: the root entry and the stream.
fn directory(name: &str, size: usize) -> [u8; SECTOR] {
    let mut out = [0u8; SECTOR];
    // The root names the storage and points at its only child.
    write_entry(&mut out[..128], "Root Entry", 5, END_OF_CHAIN, 0, 1);
    write_entry(&mut out[128..256], name, 2, 0, size, FREE);
    // The two unused slots stay zeroed, which is what an empty entry is.
    out
}

/// One 128-byte directory entry.
fn write_entry(entry: &mut [u8], name: &str, kind: u8, start: u32, size: usize, child: u32) {
    let units: Vec<u16> = name.encode_utf16().take(31).collect();
    for (i, unit) in units.iter().enumerate() {
        entry[i * 2..i * 2 + 2].copy_from_slice(&unit.to_le_bytes());
    }
    // The length counts the terminating zero.
    let length = u16::try_from(units.len() * 2 + 2).unwrap_or(0);
    entry[0x40..0x42].copy_from_slice(&length.to_le_bytes());
    entry[0x42] = kind;
    // Black, and no siblings.
    entry[0x43] = 1;
    entry[0x44..0x48].copy_from_slice(&FREE.to_le_bytes());
    entry[0x48..0x4C].copy_from_slice(&FREE.to_le_bytes());
    entry[0x4C..0x50].copy_from_slice(&child.to_le_bytes());
    entry[0x74..0x78].copy_from_slice(&start.to_le_bytes());
    entry[0x78..0x80].copy_from_slice(&(size as u64).to_le_bytes());
}
