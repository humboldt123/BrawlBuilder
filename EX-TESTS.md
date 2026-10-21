# BrawlBuilderEX test status

Tested in Dolphin (USA Rev 2 base, no SD card, no launcher).

| Mod | Result |
| --- | --- |
| Brawl Minus 5.1.5 | boots to character select |
| Project+ 3.2.1 | boots to character select (re-checked with the SD pack layout: past boot) |
| Legacy MX 0.2b | boots to character select (vanilla movies shortened to fit) |
| Brawl Void | boots to title screen |
| P+Ex / PMEX Remix 0.95DX | **work in progress**: hangs in the boot scene |

## PMEX notes

Fixed while working on PMEX (these apply to all mods):

- the SD folder is stored as `ex0.pak`, `ex1.pak`... with a hash index instead of ~9000 FST entries (the FST lives in MEM1, which PMEX fills)
- EX helper code is placed where the mod's codes don't write (PMEX's VBI hook switch writes 0x80002778)
- Brawl's SD-mounted flag (0x805664EC) is always set
- LZ77 type 0x11 files (PMEX common2.pac) are recompressed as type 0x11

Open issue: the strap-screen archive is still linked when the menu heap is rebuilt, so `gfArchiveManager::update` loops on a corrupted list. This is likely a timing difference, because disc reads are slower than the SD card.
