# BrawlBuilderEX test status

Tested in Dolphin (USA Rev 2 base, no SD card, no launcher), final .wbfs images.

| Mod | Result |
| --- | --- |
| P+Ex / PMEX Remix 0.95DX | boots to the title screen (8.0 GB image, needs every fit step: movies shortened, Subspace removed, trophy models emptied) |
| Brawl Void | boots through the title screen into the main menu |
| Brawl Minus 5.1.5 | boots, but lands in the Controls menu (`sqButton`) instead of character select: **investigating** |
| Project+ 3.2.1 | same as Minus: **investigating** |
| Legacy MX 0.2b | same as Minus: **investigating** |

Minus, Project+ and Legacy MX reached character select with an earlier build, before the SD pack layout. The Controls-menu
issue comes from `sqBoot` seeing a non-zero boot parameter. It is not caused by baking, the arena/heap move or
shadowed-file emptying (each was ruled out with a diagnostic option).

## Fixed while getting PMEX to boot (these apply to all mods)

- the SD folder is stored as `ex0.pak`, `ex1.pak`... (each under 2 GiB) with a hash index instead of ~9000 FST entries (the FST lives in MEM1, which PMEX fills)
- the hash index and EX RAM are placed at boot below the real FST and BI2 (read from 0x80000038 / 0x800000F4), so any disc builder's layout works
- path lookups normalize `./` and `//` the way `DVDOpen` does (Legacy uses `/P+Ex/././pf/...`)
- EX helper code is placed where the mod's codes don't write (PMEX's VBI hook switch writes 0x80002778)
- Brawl's SD-mounted flag (0x805664EC) is always set
- LZ77 type 0x11 files (PMEX common2.pac) are recompressed as type 0x11
- only vanilla files with the exact same path as a mod file are emptied (PMEX ships 64-byte `system/common.pac` stubs while the game still loads `common_en.pac` from the disc)

## Diagnostic CLI options

`--no-bake` (everything at runtime), `--mem2-index` (index in MEM2, MEM1 untouched), `--keep-shadowed`, `--no-free`.
