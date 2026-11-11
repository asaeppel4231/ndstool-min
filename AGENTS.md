# NDS format and project notes

## ROM layout

An NDS ROM is a little-endian image described by its header at file offset `0`.
The standard header is `0x200` bytes; fields contain absolute file offsets and
sizes for the ARM9 and ARM7 binaries, overlay tables, the File Name Table (FNT),
the File Allocation Table (FAT), and the banner. ARM9/ARM7 entry points and RAM
addresses are separate from their ROM offsets. Sections can have alignment
padding, so use the header's offsets and sizes rather than assuming a fixed
order or packing.

The FNT describes directories and maps file IDs to names. The FAT has one
`(start, end)` offset pair per file ID; file data is stored in the ROM at those
offsets. Overlay tables refer to overlay files through file IDs and are separate
from the ARM binaries. The logo and its CRC, header CRC, and secure-area CRC
are distinct header data/checks.

The secure area is a region near the start of the image (commonly
`0x4000–0x7FFF`); encrypted and decrypted ROMs share the same general layout.
This project’s secure-area operation encrypts/decrypts that region, not the
whole ROM. DSi/TWL ROMs add extended header fields and may include ARM9i/ARM7i,
digest, encryption, HMAC, and signature data. The presence of corresponding
fields in this code does not mean the tool generates or validates all DSi
integrity/authentication data.

## Code map and conventions

- `source/header.h` defines the packed header; `header.cpp` handles header CRCs,
  ROM type detection, and information output.
- `source/ndstool.cpp` parses options and dispatches operations.
- `source/ndscreate.cpp` writes ROM sections and FNT/FAT data.
- `source/ndsextract.cpp` reads ROM sections and walks FNT/FAT data.
- `source/encryption.cpp` handles secure-area encryption/decryption.
- `source/elf.cpp` and `source/ndstree.cpp` support ELF inputs and filesystem
  construction.

Keep binary offsets, sizes, endianness, and alignment explicit. Preserve the
existing low-level `FILE*` I/O and data structures unless a change requires
otherwise. Avoid reintroducing removed help, banner/logo rendering, loader,
hook, or SHA-1 subsystems.

## Build

```sh
g++ -std=gnu++17 -O2 -Wall -Wextra source/*.cpp -o ndstool
```

There is no checked-in test suite. `./ndstool -?` exits silently; test ROM
operations with suitable input images when available.
