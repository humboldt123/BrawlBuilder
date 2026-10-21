# BrawlBuilderEX test status

Tested in Dolphin (USA Rev 2 base, no SD card, no launcher), final .wbfs images.

| Mod | Result |
| --- | --- |
| Brawl Minus 5.1.5 | boots to character select |
| Project+ 3.2.1 | boots to character select |
| Legacy MX 0.2b | boots to character select |
| P+Ex / PMEX Remix 0.95DX | boots to the title screen (8.0 GB: needs shortened movies, Subspace removed and trophy models emptied) |
| Brawl Void | boots to the title screen (no boot-to-CSS code in Void) |

Played by hand (P+Ex Remix, Dolphin with keyboard as GameCube pad 1): opening movie, title, main menu, Versus and
Training character select with the full BrawlEx roster, stage select, and a Training match (Masked Man vs CPU Masked Man
on Kongo Jungle 64) with working controls.

Open: one Training match, Terry vs a CPU Ice Climbers, crashed while the fighters were loading (`ftManager` "failed create
instance" for Popo). It has not been reproduced yet, and it is not yet checked against the real launcher with an SD card.

## Fixes found along the way (these apply to all mods)

- the SD folder is stored as `ex0.pak`, `ex1.pak`... (each under 2 GiB) with a hash index instead of ~9000 FST entries (the FST lives in MEM1, which PMEX fills)
- the hash index and EX RAM are placed at boot below the real FST and BI2 (read from 0x80000038 / 0x800000F4), so any disc builder's layout works
- path lookups normalize `./` and `//` the way `DVDOpen` does (Legacy uses `/P+Ex/././pf/...`)
- EX helper code is placed where the mod's codes don't write (PMEX's VBI hook switch writes 0x80002778)
- the prfile2 functions under FA (`pf2_fopen/fread/fseek/fclose/fstat`) are redirected too: P+Ex's movie streamer calls `pf2_fread` directly
- Brawl's SD-mounted flag (0x805664EC) is always set
- LZ77 type 0x11 files (PMEX common2.pac) are recompressed as type 0x11
- EX helper placement avoids low-memory addresses the mod's ASM uses, not only its Gecko writes (Legacy TE's Tags Controls flag at 0x80002800 sent boot to the Controls menu)
- only vanilla files with the exact same path as a mod file are emptied (PMEX ships 64-byte `system/common.pac` stubs while the game still loads `common_en.pac` from the disc)

## Diagnostic CLI options

`--no-bake` (everything at runtime), `--mem2-index` (index in MEM2, MEM1 untouched), `--keep-shadowed`, `--no-free`;
environment variable `EXCODES=<file>` appends extra Gecko lines (e.g. OSReport probes) to the code list.

`--fit` allows the lossy steps on the command line (shortened movies, Subspace, trophies); the GUI asks instead.
