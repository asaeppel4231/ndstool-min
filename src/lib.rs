//! Read and write Nintendo DS ROM images while preserving unmodeled data.
//!
//! ```no_run
//! use ndsrom::NdsRom;
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let mut rom = NdsRom::open("game.nds")?;
//! std::fs::write("arm9.bin", rom.arm9())?;
//! rom.replace_file(0, b"replacement contents")?;
//! rom.save("edited.nds")?;
//! # Ok(())
//! # }
//! ```
//!
//! Resizing a file appends new bytes and redirects its FAT entry. This does not
//! rebuild FNT paths or update DSi/TWL authentication metadata.

use std::error::Error as StdError;
use std::fmt;
use std::fs;
use std::io::{self, Read, Write};
use std::path::Path;

pub const HEADER_SIZE: usize = 0x200;
const FAT_ENTRY_SIZE: usize = 8;
const LOGO_OFFSET: usize = 0xC0;
const LOGO_SIZE: usize = 156;
const LOGO_CRC_OFFSET: usize = 0x15C;
const HEADER_CRC_OFFSET: usize = 0x15E;

/// A ROM region described by an absolute file offset and byte size.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RomSection {
    pub offset: u32,
    pub size: u32,
}

/// A file's start and end offsets from one FAT entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FatEntry {
    pub start: u32,
    pub end: u32,
}

impl FatEntry {
    /// Returns the file size in bytes.
    pub fn size(self) -> u32 {
        self.end.saturating_sub(self.start)
    }
}

/// The fixed 0x200-byte NTR header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NdsHeader {
    bytes: [u8; HEADER_SIZE],
}

impl NdsHeader {
    fn parse(image: &[u8]) -> Result<Self, Error> {
        let bytes = image.get(..HEADER_SIZE).ok_or(Error::ImageTooShort {
            actual: image.len(),
        })?;
        let bytes = bytes.try_into().map_err(|_| Error::ImageTooShort {
            actual: image.len(),
        })?;
        Ok(Self { bytes })
    }

    /// Returns the header's raw bytes.
    pub fn as_bytes(&self) -> &[u8; HEADER_SIZE] {
        &self.bytes
    }

    /// Returns the 12-byte title field.
    pub fn title(&self) -> &[u8] {
        &self.bytes[0x00..0x0C]
    }

    /// Returns the four-byte game code.
    pub fn game_code(&self) -> &[u8] {
        &self.bytes[0x0C..0x10]
    }

    /// Returns the two-byte maker code.
    pub fn maker_code(&self) -> &[u8] {
        &self.bytes[0x10..0x12]
    }

    /// Returns the unit code (bit 1 indicates a DSi/TWL header extension).
    pub fn unit_code(&self) -> u8 {
        self.bytes[0x12]
    }

    /// Returns the cartridge device-capacity field.
    pub fn device_capacity(&self) -> u8 {
        self.bytes[0x14]
    }

    /// Returns the ROM header size field.
    pub fn rom_header_size(&self) -> u32 {
        read_u32(&self.bytes, 0x84)
    }

    /// Returns the section containing the ARM9 binary.
    pub fn arm9(&self) -> RomSection {
        section(&self.bytes, 0x20, 0x2C)
    }

    /// Returns the section containing the ARM7 binary.
    pub fn arm7(&self) -> RomSection {
        section(&self.bytes, 0x30, 0x3C)
    }

    /// Returns the ARM9i section in a DSi/TWL header.
    pub fn arm9i(&self) -> RomSection {
        section(&self.bytes, 0x1C0, 0x1CC)
    }

    /// Returns the ARM7i section in a DSi/TWL header.
    pub fn arm7i(&self) -> RomSection {
        section(&self.bytes, 0x1D0, 0x1DC)
    }

    /// Returns the File Name Table section.
    pub fn fnt(&self) -> RomSection {
        section(&self.bytes, 0x40, 0x44)
    }

    /// Returns the File Allocation Table section.
    pub fn fat(&self) -> RomSection {
        section(&self.bytes, 0x48, 0x4C)
    }

    /// Returns the ARM9 overlay table section.
    pub fn arm9_overlay_table(&self) -> RomSection {
        section(&self.bytes, 0x50, 0x54)
    }

    /// Returns the ARM7 overlay table section.
    pub fn arm7_overlay_table(&self) -> RomSection {
        section(&self.bytes, 0x58, 0x5C)
    }

    /// Returns the banner offset. The banner length is version-dependent for NTR ROMs.
    pub fn banner_offset(&self) -> u32 {
        read_u32(&self.bytes, 0x68)
    }

    /// Returns the application-end offset.
    pub fn application_end(&self) -> u32 {
        read_u32(&self.bytes, 0x80)
    }
}

/// An NDS ROM image. Unknown bytes and sections are retained verbatim.
///
/// The library validates the base header, its declared sections, and every FAT
/// entry. It does not validate DSi/TWL signatures, HMACs, or digest tables.
#[derive(Clone, Debug)]
pub struct NdsRom {
    image: Vec<u8>,
    fat_entries: Vec<FatEntry>,
}

impl NdsRom {
    /// Reads an NDS image from any `Read` implementation.
    pub fn read_from<R: Read>(mut reader: R) -> Result<Self, Error> {
        let mut image = Vec::new();
        reader.read_to_end(&mut image)?;
        Self::from_bytes(image)
    }

    /// Reads an NDS image from a file.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, Error> {
        Self::from_bytes(fs::read(path)?)
    }

    /// Parses an owned NDS image.
    pub fn from_bytes(image: Vec<u8>) -> Result<Self, Error> {
        let header = NdsHeader::parse(&image)?;
        let sections = [
            ("ARM9", header.arm9()),
            ("ARM7", header.arm7()),
            ("FNT", header.fnt()),
            ("FAT", header.fat()),
            ("ARM9 overlay table", header.arm9_overlay_table()),
            ("ARM7 overlay table", header.arm7_overlay_table()),
        ];
        for (name, section) in sections {
            validate_section(name, section, image.len())?;
        }
        if header.unit_code() & 2 != 0 {
            validate_section("ARM9i", header.arm9i(), image.len())?;
            validate_section("ARM7i", header.arm7i(), image.len())?;
        }

        let fat_section = header.fat();
        if fat_section.size as usize % FAT_ENTRY_SIZE != 0 {
            return Err(Error::InvalidFatSize {
                size: fat_section.size,
            });
        }
        let mut fat_entries = Vec::with_capacity(fat_section.size as usize / FAT_ENTRY_SIZE);
        if fat_section.size != 0 {
            let fat_start = fat_section.offset as usize;
            let fat_end = fat_start + fat_section.size as usize;
            for (file_id, entry) in image[fat_start..fat_end]
                .chunks_exact(FAT_ENTRY_SIZE)
                .enumerate()
            {
                let start = read_u32(entry, 0);
                let end = read_u32(entry, 4);
                if end < start || end as usize > image.len() {
                    return Err(Error::InvalidFatEntry {
                        file_id,
                        start,
                        end,
                        image_size: image.len(),
                    });
                }
                fat_entries.push(FatEntry { start, end });
            }
        }

        Ok(Self { image, fat_entries })
    }

    /// Returns the parsed fixed-size header.
    pub fn header(&self) -> NdsHeader {
        NdsHeader::parse(&self.image).expect("NdsRom always contains a complete header")
    }

    /// Returns the complete ROM image as bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.image
    }

    /// Writes the complete ROM image to any `Write` implementation.
    pub fn write_to<W: Write>(&self, mut writer: W) -> Result<(), Error> {
        writer.write_all(&self.image)?;
        Ok(())
    }

    /// Writes the complete ROM image to a file.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), Error> {
        fs::write(path, &self.image)?;
        Ok(())
    }

    /// Returns the bytes of a declared section.
    pub fn section(&self, section: RomSection) -> Result<&[u8], Error> {
        section_bytes(&self.image, "section", section)
    }

    /// Returns the ARM9 binary.
    pub fn arm9(&self) -> &[u8] {
        self.section(self.header().arm9())
            .expect("validated ARM9 section")
    }

    /// Returns the ARM7 binary.
    pub fn arm7(&self) -> &[u8] {
        self.section(self.header().arm7())
            .expect("validated ARM7 section")
    }

    /// Returns the ARM9i binary when the image declares a DSi/TWL header.
    pub fn arm9i(&self) -> Result<Option<&[u8]>, Error> {
        if self.header().unit_code() & 2 == 0 {
            return Ok(None);
        }
        self.section(self.header().arm9i()).map(Some)
    }

    /// Returns the ARM7i binary when the image declares a DSi/TWL header.
    pub fn arm7i(&self) -> Result<Option<&[u8]>, Error> {
        if self.header().unit_code() & 2 == 0 {
            return Ok(None);
        }
        self.section(self.header().arm7i()).map(Some)
    }

    /// Returns the raw 16 KiB secure area when the image contains it.
    ///
    /// The bytes are returned as stored; this method does not decrypt or
    /// encrypt them.
    pub fn secure_area(&self) -> Option<&[u8]> {
        self.image.get(0x4000..0x8000)
    }

    /// Returns the raw FNT bytes.
    pub fn fnt(&self) -> &[u8] {
        self.section(self.header().fnt())
            .expect("validated FNT section")
    }

    /// Returns the raw FAT bytes.
    pub fn fat(&self) -> &[u8] {
        self.section(self.header().fat())
            .expect("validated FAT section")
    }

    /// Returns all entries in file-ID order.
    pub fn fat_entries(&self) -> &[FatEntry] {
        &self.fat_entries
    }

    /// Returns a file's contents by FAT file ID.
    pub fn file(&self, file_id: usize) -> Result<&[u8], Error> {
        let entry = self
            .fat_entries
            .get(file_id)
            .ok_or(Error::FileIdOutOfRange {
                file_id,
                file_count: self.fat_entries.len(),
            })?;
        Ok(&self.image[entry.start as usize..entry.end as usize])
    }

    /// Replaces a file by ID. Different-sized data is appended to the image and
    /// its FAT entry is redirected; the old bytes are preserved as unused data.
    ///
    /// Header and logo CRCs are recalculated. DSi/TWL digests, HMACs, and
    /// signatures are not regenerated.
    pub fn replace_file(&mut self, file_id: usize, data: &[u8]) -> Result<(), Error> {
        let old_entry = *self
            .fat_entries
            .get(file_id)
            .ok_or(Error::FileIdOutOfRange {
                file_id,
                file_count: self.fat_entries.len(),
            })?;

        if data.len() > u32::MAX as usize {
            return Err(Error::ImageTooLarge);
        }

        let relocated = if data.len() == old_entry.size() as usize {
            None
        } else {
            let append_at = align_up(self.image.len(), 4)?;
            let end = append_at
                .checked_add(data.len())
                .ok_or(Error::ImageTooLarge)?;
            let start_u32 = u32::try_from(append_at).map_err(|_| Error::ImageTooLarge)?;
            let end_u32 = u32::try_from(end).map_err(|_| Error::ImageTooLarge)?;
            Some((append_at, start_u32, end_u32))
        };

        let (start, end) = relocated
            .map(|(_, start, end)| (start, end))
            .unwrap_or((old_entry.start, old_entry.end));
        let application_end = self.header().application_end().max(end);
        let device_capacity = device_capacity_for(application_end)?;

        if let Some((append_at, start, end)) = relocated {
            self.image.resize(append_at, 0xFF);
            self.image.extend_from_slice(data);
            let fat_entry_offset = self.header().fat().offset as usize + file_id * FAT_ENTRY_SIZE;
            self.image[fat_entry_offset..fat_entry_offset + 4]
                .copy_from_slice(&start.to_le_bytes());
            self.image[fat_entry_offset + 4..fat_entry_offset + 8]
                .copy_from_slice(&end.to_le_bytes());
        } else {
            let start = old_entry.start as usize;
            let end = old_entry.end as usize;
            self.image[start..end].copy_from_slice(data);
        }

        self.fat_entries[file_id] = FatEntry { start, end };
        self.image[0x80..0x84].copy_from_slice(&application_end.to_le_bytes());
        self.image[0x14] = device_capacity;
        self.recalculate_header_crcs();
        Ok(())
    }

    /// Returns whether the stored Nintendo logo CRC matches the logo bytes.
    pub fn logo_crc_is_valid(&self) -> bool {
        read_u16(&self.image, LOGO_CRC_OFFSET)
            == crc16(&self.image[LOGO_OFFSET..LOGO_OFFSET + LOGO_SIZE])
    }

    /// Returns whether the stored header CRC matches bytes `0x000..0x15E`.
    pub fn header_crc_is_valid(&self) -> bool {
        read_u16(&self.image, HEADER_CRC_OFFSET) == crc16(&self.image[..HEADER_CRC_OFFSET])
    }

    /// Recalculates the Nintendo logo and base-header CRC fields.
    pub fn recalculate_header_crcs(&mut self) {
        let logo_crc = crc16(&self.image[LOGO_OFFSET..LOGO_OFFSET + LOGO_SIZE]);
        self.image[LOGO_CRC_OFFSET..HEADER_CRC_OFFSET].copy_from_slice(&logo_crc.to_le_bytes());
        let header_crc = crc16(&self.image[..HEADER_CRC_OFFSET]);
        self.image[HEADER_CRC_OFFSET..HEADER_CRC_OFFSET + 2]
            .copy_from_slice(&header_crc.to_le_bytes());
    }
}

/// Errors returned while parsing or editing an NDS image.
#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    ImageTooShort {
        actual: usize,
    },
    InvalidSection {
        name: &'static str,
        offset: u32,
        size: u32,
        image_size: usize,
    },
    InvalidFatSize {
        size: u32,
    },
    InvalidFatEntry {
        file_id: usize,
        start: u32,
        end: u32,
        image_size: usize,
    },
    FileIdOutOfRange {
        file_id: usize,
        file_count: usize,
    },
    ImageTooLarge,
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "{error}"),
            Self::ImageTooShort { actual } => write!(
                formatter,
                "image is {actual} bytes; an NDS header requires at least {HEADER_SIZE}"
            ),
            Self::InvalidSection {
                name,
                offset,
                size,
                image_size,
            } => write!(
                formatter,
                "{name} section at 0x{offset:X} with size 0x{size:X} exceeds image size 0x{image_size:X}"
            ),
            Self::InvalidFatSize { size } => {
                write!(formatter, "FAT size {size} is not a multiple of 8")
            }
            Self::InvalidFatEntry {
                file_id,
                start,
                end,
                image_size,
            } => write!(
                formatter,
                "FAT entry {file_id} has invalid range 0x{start:X}..0x{end:X} for image size 0x{image_size:X}"
            ),
            Self::FileIdOutOfRange {
                file_id,
                file_count,
            } => write!(
                formatter,
                "file ID {file_id} is out of range; ROM has {file_count} FAT entries"
            ),
            Self::ImageTooLarge => write!(formatter, "image exceeds the NDS 32-bit offset range"),
        }
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

fn section(bytes: &[u8], offset_field: usize, size_field: usize) -> RomSection {
    RomSection {
        offset: read_u32(bytes, offset_field),
        size: read_u32(bytes, size_field),
    }
}

fn validate_section(
    name: &'static str,
    section: RomSection,
    image_size: usize,
) -> Result<(), Error> {
    if section.size == 0 {
        return Ok(());
    }
    let end = u64::from(section.offset) + u64::from(section.size);
    if end > image_size as u64 {
        return Err(Error::InvalidSection {
            name,
            offset: section.offset,
            size: section.size,
            image_size,
        });
    }
    Ok(())
}

fn section_bytes<'a>(
    image: &'a [u8],
    name: &'static str,
    section: RomSection,
) -> Result<&'a [u8], Error> {
    validate_section(name, section, image.len())?;
    if section.size == 0 {
        return Ok(&image[..0]);
    }
    let start = section.offset as usize;
    let end = start + section.size as usize;
    Ok(&image[start..end])
}

fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

fn crc16(bytes: &[u8]) -> u16 {
    let mut crc = 0xFFFFu16;
    for byte in bytes {
        crc ^= u16::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xA001
            } else {
                crc >> 1
            };
        }
    }
    crc
}

fn align_up(value: usize, alignment: usize) -> Result<usize, Error> {
    value
        .checked_add(alignment - 1)
        .map(|value| value & !(alignment - 1))
        .ok_or(Error::ImageTooLarge)
}

fn device_capacity_for(image_end: u32) -> Result<u8, Error> {
    let required = u64::from(image_end).max(128 * 1024);
    let capacity_power = (64 - (required - 1).leading_zeros()) as i32;
    u8::try_from(capacity_power - 17).map_err(|_| Error::ImageTooLarge)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put_u32(image: &mut [u8], offset: usize, value: u32) {
        image[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn test_rom() -> NdsRom {
        let mut image = vec![0u8; 0x2400];
        put_u32(&mut image, 0x20, 0x2000);
        put_u32(&mut image, 0x2C, 4);
        put_u32(&mut image, 0x30, 0x2100);
        put_u32(&mut image, 0x3C, 4);
        put_u32(&mut image, 0x40, 0x2200);
        put_u32(&mut image, 0x44, 8);
        put_u32(&mut image, 0x48, 0x2210);
        put_u32(&mut image, 0x4C, 8);
        let image_size = image.len() as u32;
        put_u32(&mut image, 0x80, image_size);
        image[0x2000..0x2004].copy_from_slice(b"ARM9");
        image[0x2100..0x2104].copy_from_slice(b"ARM7");
        put_u32(&mut image, 0x2210, 0x2300);
        put_u32(&mut image, 0x2214, 0x2304);
        image[0x2300..0x2304].copy_from_slice(b"data");
        let mut rom = NdsRom::from_bytes(image).unwrap();
        rom.recalculate_header_crcs();
        rom
    }

    #[test]
    fn reads_sections_and_files() {
        let rom = test_rom();
        assert_eq!(rom.arm9(), b"ARM9");
        assert_eq!(rom.arm7(), b"ARM7");
        assert_eq!(rom.file(0).unwrap(), b"data");
        assert!(rom.header_crc_is_valid());
        assert!(rom.logo_crc_is_valid());
    }

    #[test]
    fn replaces_file_and_round_trips() {
        let mut rom = test_rom();
        rom.replace_file(0, b"expanded file").unwrap();
        assert_eq!(rom.file(0).unwrap(), b"expanded file");
        assert!(rom.header_crc_is_valid());
        assert!(rom.logo_crc_is_valid());

        let mut bytes = Vec::new();
        rom.write_to(&mut bytes).unwrap();
        let reparsed = NdsRom::from_bytes(bytes).unwrap();
        assert_eq!(reparsed.file(0).unwrap(), b"expanded file");
    }

    #[test]
    fn replaces_equal_sized_file_in_place() {
        let mut rom = test_rom();
        let original_image_size = rom.as_bytes().len();
        let original_entry = rom.fat_entries()[0];

        rom.replace_file(0, b"edit").unwrap();

        assert_eq!(rom.file(0).unwrap(), b"edit");
        assert_eq!(rom.as_bytes().len(), original_image_size);
        assert_eq!(rom.fat_entries()[0], original_entry);
    }

    #[test]
    fn rejects_invalid_fat_ranges() {
        let mut image = test_rom().as_bytes().to_vec();
        put_u32(&mut image, 0x2210, 0x2304);
        put_u32(&mut image, 0x2214, 0x2300);
        assert!(matches!(
            NdsRom::from_bytes(image),
            Err(Error::InvalidFatEntry { file_id: 0, .. })
        ));
    }

    #[test]
    fn reads_optional_dsi_binaries() {
        let mut image = test_rom().as_bytes().to_vec();
        image[0x12] = 2;
        put_u32(&mut image, 0x1C0, 0x2304);
        put_u32(&mut image, 0x1CC, 4);
        put_u32(&mut image, 0x1D0, 0x2308);
        put_u32(&mut image, 0x1DC, 4);
        image[0x2304..0x2308].copy_from_slice(b"ARM9");
        image[0x2308..0x230C].copy_from_slice(b"ARM7");

        let rom = NdsRom::from_bytes(image).unwrap();
        assert_eq!(rom.arm9i().unwrap(), Some(&b"ARM9"[..]));
        assert_eq!(rom.arm7i().unwrap(), Some(&b"ARM7"[..]));
    }
}
