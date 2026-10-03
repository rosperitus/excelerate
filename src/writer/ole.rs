//! Writing the OLE compound file an xls lives in.
//!
//! The container is a FAT filesystem, and this writes the smallest one that
//! holds a few named streams: the data sectors, the directory naming the root
//! and the streams, and the allocation table listing all of it.
//!
//! Short streams belong in the mini stream, a second filesystem inside the root
//! entry's own stream. Rather than implement it for the one case where a
//! workbook is under 4 KB, such a stream is padded up to the cutoff - the
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
/// A sector holding part of the list of allocation table sectors.
const DIFAT_SECTOR: u32 = 0xFFFF_FFFC;
/// How many table sectors the header itself can list.
const HEADER_FAT_SLOTS: usize = 109;
/// How many table sectors one list sector names; its last slot links on.
const DIFAT_SLOTS: usize = SECTOR / 4 - 1;
/// A free sector, and the empty directory pointer.
const FREE: u32 = 0xFFFF_FFFF;

/// Wraps several named streams in a compound file, in the order given: the
/// workbook, then the property sets beside it.
#[must_use]
pub fn streams(named: &[(&str, &[u8])]) -> Vec<u8> {
    // Each stream padded up to the mini cutoff and to whole sectors; `start`
    // is its first sector, `declared` the size its entry states.
    let mut data = Vec::new();
    let mut placed: Vec<(&str, usize, usize)> = Vec::with_capacity(named.len());
    for (name, stream) in named {
        let start = data.len() / SECTOR;
        let declared = stream.len().max(MINI_CUTOFF);
        data.extend_from_slice(stream);
        data.resize(data.len() + declared - stream.len(), 0);
        let padding = (SECTOR - data.len() % SECTOR) % SECTOR;
        data.resize(data.len() + padding, 0);
        placed.push((name, start, declared));
    }
    let data_sectors = data.len() / SECTOR;
    // Four 128-byte entries to a directory sector: the root, then the streams.
    let directory_sectors = (placed.len() + 1).div_ceil(SECTOR / 128);

    // The table has to describe itself, and past 109 sectors of it (about
    // 7 MB of streams) the header cannot list them all and list sectors
    // follow, which the table describes too; the sizes are found by trying.
    let mut fat_sectors = 1;
    let mut difat_sectors = 0;
    loop {
        let total = data_sectors + directory_sectors + fat_sectors + difat_sectors;
        let needed = total.div_ceil(SECTOR / 4);
        let lists = needed
            .saturating_sub(HEADER_FAT_SLOTS)
            .div_ceil(DIFAT_SLOTS);
        if needed <= fat_sectors && lists <= difat_sectors {
            break;
        }
        fat_sectors = fat_sectors.max(needed);
        difat_sectors = difat_sectors.max(lists);
    }
    let directory_sector = data_sectors;
    let first_fat_sector = directory_sector + directory_sectors;
    let first_difat_sector = first_fat_sector + fat_sectors;

    // The allocation table: each stream's chain, then the directory's, then
    // the sectors the table itself sits in.
    let mut fat = vec![FREE; fat_sectors * (SECTOR / 4)];
    let mut chain = |from: usize, count: usize| {
        for (i, entry) in fat.iter_mut().enumerate().skip(from).take(count) {
            *entry = if i + 1 == from + count {
                END_OF_CHAIN
            } else {
                u32::try_from(i + 1).unwrap_or(END_OF_CHAIN)
            };
        }
    };
    for &(_, start, declared) in &placed {
        chain(start, declared.div_ceil(SECTOR));
    }
    chain(directory_sector, directory_sectors);
    for i in 0..fat_sectors {
        fat[first_fat_sector + i] = FAT_SECTOR;
    }
    for i in 0..difat_sectors {
        fat[first_difat_sector + i] = DIFAT_SECTOR;
    }

    let total = 1 + data_sectors + directory_sectors + fat_sectors + difat_sectors;
    let mut out = Vec::with_capacity(total * SECTOR);
    out.extend_from_slice(&header(
        fat_sectors,
        first_fat_sector,
        directory_sector,
        (difat_sectors, first_difat_sector),
    ));
    out.extend_from_slice(&data);
    out.extend_from_slice(&directory(&placed, directory_sectors));
    for entry in fat {
        out.extend_from_slice(&entry.to_le_bytes());
    }
    // The table sectors the header had no room for, 127 to a list sector,
    // each list ending with the next one's number.
    let overflow: Vec<usize> = (HEADER_FAT_SLOTS..fat_sectors)
        .map(|i| first_fat_sector + i)
        .collect();
    for i in 0..difat_sectors {
        let start = i * DIFAT_SLOTS;
        let slots = overflow.get(start..).unwrap_or(&[]);
        for slot in 0..DIFAT_SLOTS {
            let value = slots
                .get(slot)
                .map_or(FREE, |&s| u32::try_from(s).unwrap_or(FREE));
            out.extend_from_slice(&value.to_le_bytes());
        }
        let next = if i + 1 < difat_sectors {
            u32::try_from(first_difat_sector + i + 1).unwrap_or(END_OF_CHAIN)
        } else {
            END_OF_CHAIN
        };
        out.extend_from_slice(&next.to_le_bytes());
    }
    out
}

/// The 512-byte header, including the list of table sectors.
fn header(
    fat_sectors: usize,
    first_fat_sector: usize,
    directory_sector: usize,
    (difat_sectors, first_difat_sector): (usize, usize),
) -> [u8; SECTOR] {
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
    // No mini stream; the list sectors, if the table outgrew the header.
    head[0x3C..0x40].copy_from_slice(&END_OF_CHAIN.to_le_bytes());
    head[0x40..0x44].copy_from_slice(&0u32.to_le_bytes());
    let first_difat = if difat_sectors == 0 {
        END_OF_CHAIN
    } else {
        u32::try_from(first_difat_sector).unwrap_or(END_OF_CHAIN)
    };
    head[0x44..0x48].copy_from_slice(&first_difat.to_le_bytes());
    let difat_count = u32::try_from(difat_sectors).unwrap_or(0);
    head[0x48..0x4C].copy_from_slice(&difat_count.to_le_bytes());
    for i in 0..HEADER_FAT_SLOTS {
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

/// The directory: the root entry, then one entry per stream.
///
/// The streams are the root's children, kept as a red-black tree ordered the
/// way the format compares names: shorter first, then by upper case. Readers
/// walk that tree to find a stream by name, so a plain chain of siblings is
/// not enough once there is more than one.
fn directory(placed: &[(&str, usize, usize)], sectors: usize) -> Vec<u8> {
    let mut out = vec![0u8; sectors * SECTOR];
    let mut order: Vec<usize> = (0..placed.len()).collect();
    order.sort_by_key(|&i| {
        let upper: Vec<u16> = placed[i].0.to_uppercase().encode_utf16().collect();
        (upper.len(), upper)
    });
    let mut links = vec![(FREE, FREE, true); placed.len()];
    let depth = usize::BITS - placed.len().leading_zeros();
    let perfect = (placed.len() + 1).is_power_of_two();
    let root = tree(&order, 1, depth, perfect, &mut links);
    write_entry(
        &mut out[..128],
        "Root Entry",
        5,
        END_OF_CHAIN,
        0,
        root,
        (FREE, FREE, true),
    );
    for (i, &(name, start, size)) in placed.iter().enumerate() {
        let at = (i + 1) * 128;
        let start = u32::try_from(start).unwrap_or(END_OF_CHAIN);
        write_entry(&mut out[at..at + 128], name, 2, start, size, FREE, links[i]);
    }
    out
}

/// Builds a balanced tree over `order` (entries sorted by name) and returns
/// the directory id of its root. Every node is black except those on the
/// last level of a tree that is not full: that keeps every path from the
/// root through the same number of black nodes, which is the rule.
fn tree(
    order: &[usize],
    level: u32,
    depth: u32,
    perfect: bool,
    links: &mut [(u32, u32, bool)],
) -> u32 {
    if order.is_empty() {
        return FREE;
    }
    let middle = order.len() / 2;
    let left = tree(&order[..middle], level + 1, depth, perfect, links);
    let right = tree(&order[middle + 1..], level + 1, depth, perfect, links);
    let black = perfect || level < depth;
    links[order[middle]] = (left, right, black);
    // Directory ids count the root as 0.
    u32::try_from(order[middle] + 1).unwrap_or(FREE)
}

/// One 128-byte directory entry; `siblings` is left, right and whether the
/// entry is black.
fn write_entry(
    entry: &mut [u8],
    name: &str,
    kind: u8,
    start: u32,
    size: usize,
    child: u32,
    (left, right, black): (u32, u32, bool),
) {
    let units: Vec<u16> = name.encode_utf16().take(31).collect();
    for (i, unit) in units.iter().enumerate() {
        entry[i * 2..i * 2 + 2].copy_from_slice(&unit.to_le_bytes());
    }
    // The length counts the terminating zero.
    let length = u16::try_from(units.len() * 2 + 2).unwrap_or(0);
    entry[0x40..0x42].copy_from_slice(&length.to_le_bytes());
    entry[0x42] = kind;
    entry[0x43] = u8::from(black);
    entry[0x44..0x48].copy_from_slice(&left.to_le_bytes());
    entry[0x48..0x4C].copy_from_slice(&right.to_le_bytes());
    entry[0x4C..0x50].copy_from_slice(&child.to_le_bytes());
    entry[0x74..0x78].copy_from_slice(&start.to_le_bytes());
    entry[0x78..0x80].copy_from_slice(&(size as u64).to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::streams;
    use crate::reader::ole::Ole;

    /// A stream too big for the header to list every table sector: past about
    /// 7 MB the list continues in sectors of its own, and without them the
    /// tail of the stream is unreachable.
    #[test]
    fn a_stream_past_the_header_list_reads_back_whole() {
        let stream: Vec<u8> = (0..20_000_000u32).map(|i| (i % 251) as u8).collect();
        let file = streams(&[("Workbook", &stream)]);
        let ole = Ole::new(&file).expect("the container opens");
        assert_eq!(ole.stream("Workbook").as_deref(), Some(stream.as_slice()));
    }

    /// The workbook and the two property sets, as Excel lays them out: each
    /// found again by name through the directory tree.
    #[test]
    fn several_streams_read_back_by_name() {
        let workbook = b"book".repeat(3000);
        let summary = b"summary".to_vec();
        let document = b"document".repeat(700);
        let file = streams(&[
            ("Workbook", &workbook),
            ("\u{5}SummaryInformation", &summary),
            ("\u{5}DocumentSummaryInformation", &document),
        ]);
        let ole = Ole::new(&file).expect("the container opens");
        for (name, data) in [
            ("Workbook", &workbook),
            ("\u{5}SummaryInformation", &summary),
            ("\u{5}DocumentSummaryInformation", &document),
        ] {
            let back = ole.stream(name).expect("the stream is there");
            assert_eq!(&back[..data.len()], data.as_slice(), "{name:?}");
        }
    }

    #[test]
    fn a_small_stream_reads_back_whole() {
        let stream = b"records".repeat(1000);
        let file = streams(&[("Workbook", &stream)]);
        let ole = Ole::new(&file).expect("the container opens");
        let back = ole.stream("Workbook").expect("the stream is there");
        assert_eq!(&back[..stream.len()], stream.as_slice());
    }
}
