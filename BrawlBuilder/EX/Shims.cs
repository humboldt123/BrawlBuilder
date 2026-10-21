using System.Collections.Generic;
using System.Linq;

namespace BrawlBuilder.EX
{
    /// <summary>
    /// Code EX adds to main.dol (text right after the Gecko handler):
    ///  * "SD card on the disc": replacements for Brawl's FA (SD/FAT) file API that read the mod's SD folder from one pack file on the DVD
    ///    (see <see cref="SdPack"/>). Every SD reader of the Project+ family (File Patch Code, sawnd/brstm streamers, readSDFile/checkFileSD
    ///    users, Syringe, BrawlEx, THP replacement, stage/music loaders) ends up in FAFopen/FAFread/FAFseek/FAFclose/FAFstat, so the mod runs
    ///    unmodified. Paths resolve through a hash table that is loaded below the FST on first use - a few bytes per file instead of the
    ///    ~32 an FST entry costs, which matters for mods that use nearly all of MEM1 (P+Ex: ~9000 files).
    ///  * a CRT .bss clear that leaves the code list and the baked data alone (the launcher instead skipped the clear entirely), and that sets
    ///    the arena top before OSInit reads it, leaving room for the hash table.
    /// </summary>
    public static class Shims
    {
        public const uint DvdOpen = 0x801F6278, DvdReadPrio = 0x801F67F0, OsLockMutex = 0x801DEBB4, OsUnlockMutex = 0x801DEC90;
        public const uint Memset = 0x8000443C;
        public static readonly (string Name, uint Addr, uint Old)[] FaEntries =
        {
            ("open", 0x803EBEB8, 0x9421FFF0), ("read", 0x803EBEE4, 0x4BFFF7FC), ("seek", 0x803EBEE8, 0x9421FFF0),
            ("close", 0x803EBE8C, 0x9421FFF0), ("stat", 0x803EBF6C, 0x9421FFF0),
            // the prfile2 functions under them (same arguments; FAF* only normalize the result) - P+Ex's THP streamer calls pf2_fread directly
            ("open", 0x803EB644, 0x9421FFD0), ("read", 0x803EB6E0, 0x9421FFF0), ("seek", 0x803EB708, 0x9421FFF0),
            ("close", 0x803EB620, 0x9421FFF0), ("stat", 0x803EB7C4, 0x9421FFE0),
        };
        // file slot: +0 used, +4 position, +8 -> fake FA file object whose +0x264 is the size (RSBE reads the size that way), +0xC size,
        // +0x10 offset in its pack, +0x14 -> the pack's DVDFileInfo
        // RAM block (zeroed by the early hook): bounce buffer, mutex, init flag, one DVDFileInfo per pack, file slots
        const int NSlot = 16, Slot = 0x20, Used = 0, Pos = 4, Obj = 8, Size = 0xC, Base = 0x10, Fip = 0x14, Bounce = 0x400;
        const int BB = 0, MTX = Bounce, INIT = MTX + 0x20, FIP = INIT + 0x20;
        public static uint DataSize(int packs) => (uint)(FIP + packs * 0x40 + NSlot * Slot);

        /// <summary>where the packs are, where their hash table goes, and the RAM block</summary>
        public class PackInfo { public List<string> Paths; public uint Count, EntriesOff, IndexLoad; }

        static void Prolog(Asm a, int n) { a.W(Ppc.Stwu(1, -0x40, 1)); a.W(Ppc.Mflr(0)); a.W(Ppc.Stw(0, 0x44, 1)); a.W(Ppc.Stmw(32 - n, 0x08, 1)); }
        static void Epilog(Asm a, int n) { a.W(Ppc.Lmw(32 - n, 0x08, 1)); a.W(Ppc.Lwz(0, 0x44, 1)); a.W(Ppc.Mtlr(0)); a.W(Ppc.Addi(1, 1, 0x40)); a.W(Ppc.Blr); }
        static void Strip(Asm a, int dst, int src, string tag)          // dst = src past "xx:" if a ':' occurs in the first 6 chars
        {
            a.W(Ppc.Mr(dst, src)); a.W(Ppc.Li(0, 6)); a.W(Ppc.Mtctr(0)); a.W(Ppc.Mr(12, src));
            a.Label(tag + "_l"); a.W(Ppc.Lbz(0, 0, 12)); a.W(Ppc.Cmpwi(0, 0)); a.Bc("eq", tag + "_d");
            a.W(Ppc.Cmpwi(0, 0x3A)); a.Bc("ne", tag + "_n"); a.W(Ppc.Addi(dst, 12, 1)); a.Br(tag + "_d");
            a.Label(tag + "_n"); a.W(Ppc.Addi(12, 12, 1)); a.Bdnz(tag + "_l");
            a.Label(tag + "_d");
        }

        static void LaL(Asm a, int reg, string label, uint add)
        {
            a.W((pc, l) => Ppc.Lis(reg, (int)(((l[label] + add + 0x8000) >> 16) & 0xFFFF))); a.W((pc, l) => Ppc.Addi(reg, reg, (int)((l[label] + add) & 0xFFFF)));
        }

        /// <summary>reg = RAM block + off (the block's address is decided at boot, below the real FST, and kept at label "ram")</summary>
        static void LaD(Asm a, int reg, uint off)
        {
            a.W((pc, l) => Ppc.Lis(reg, (int)(((l["ram"] + 0x8000) >> 16) & 0xFFFF))); a.W((pc, l) => Ppc.Lwz(reg, (int)(short)(l["ram"] & 0xFFFF), reg));
            if (off != 0) a.W(Ppc.Addi(reg, reg, (int)off));
        }

        static void SdDisc(Asm a, PackInfo pk)
        {
            uint BB = Shims.BB, MTX = Shims.MTX, INIT = Shims.INIT, FIP = Shims.FIP, T = FIP + 0x40u * (uint)pk.Paths.Count;
            uint IDX = DataSize(pk.Paths.Count), HASHES = IDX + 0x20;

            // ex_init (mutex held): open the packs once and load the hash table from ex0.pak; r3 = 1 ok / 0 failed
            a.Label("ex_init"); Prolog(a, 1);
            LaD(a, 12, INIT); a.W(Ppc.Lwz(0, 0, 12)); a.W(Ppc.Cmpwi(0, 0)); a.Bc("ne", "in_ok");
            for (int i = 0; i < pk.Paths.Count; i++) { LaL(a, 3, "paths", 0x10u * (uint)i); LaD(a, 4, FIP + 0x40u * (uint)i); a.Bl(DvdOpen); a.W(Ppc.Cmpwi(3, 0)); a.Bc("eq", "in_fail"); }
            LaD(a, 3, FIP); LaD(a, 4, IDX); a.La(5, pk.IndexLoad); a.W(Ppc.Li(6, 0)); a.W(Ppc.Li(7, 2)); a.Bl(DvdReadPrio);
            a.W(Ppc.Cmpwi(3, 0)); a.Bc("lt", "in_fail");
            LaD(a, 12, INIT); a.W(Ppc.Li(0, 1)); a.W(Ppc.Stw(0, 0, 12));
            a.Label("in_ok"); a.W(Ppc.Li(3, 1)); a.Br("in_ret");
            a.Label("in_fail"); a.W(Ppc.Li(3, 0));
            a.Label("in_ret"); Epilog(a, 1);

            // ex_find(r3 = path, mutex held): r3 = 1 and r4 = offset, r5 = size, r6 = pack DVDFileInfo if the file exists, else r3 = 0
            // key: path without leading '/', ASCII lower case; h1 = FNV-1a, h2 = djb2-xor (checked against the entry read from the disc)
            a.Label("ex_find"); Prolog(a, 8);
            a.W(Ppc.Mr(31, 3));
            a.Label("fd_sl"); a.W(Ppc.Lbz(0, 0, 31)); a.W(Ppc.Cmpwi(0, 0x2F)); a.Bc("ne", "fd_h"); a.W(Ppc.Addi(31, 31, 1)); a.Br("fd_sl");
            a.Label("fd_h"); a.La(30, 0x811C9DC5); a.W(Ppc.Li(29, 5381)); a.La(28, 0x01000193); a.W(Ppc.Li(27, 33)); a.W(Ppc.Li(9, 1));
            a.Label("fd_hl"); a.W(Ppc.Lbz(10, 0, 31)); a.W(Ppc.Cmpwi(10, 0)); a.Bc("eq", "fd_bs");
            // normalize like DVDOpen: at a segment start skip extra '/' and "./" ("/P+Ex/././pf/..." in Legacy's codes)
            a.W(Ppc.Cmpwi(9, 0)); a.Bc("eq", "fd_ch");
            a.W(Ppc.Cmpwi(10, 0x2F)); a.Bc("eq", "fd_sk1");
            a.W(Ppc.Cmpwi(10, 0x2E)); a.Bc("ne", "fd_ch"); a.W(Ppc.Lbz(0, 1, 31)); a.W(Ppc.Cmpwi(0, 0x2F)); a.Bc("ne", "fd_ch");
            a.W(Ppc.Addi(31, 31, 1));
            a.Label("fd_sk1"); a.W(Ppc.Addi(31, 31, 1)); a.Br("fd_hl");
            a.Label("fd_ch"); a.W(Ppc.Li(9, 0)); a.W(Ppc.Cmpwi(10, 0x2F)); a.Bc("ne", "fd_cs"); a.W(Ppc.Li(9, 1));
            a.Label("fd_cs");
            a.W(Ppc.Addi(12, 10, -0x41)); a.W(Ppc.Cmplwi(12, 25)); a.Bc("gt", "fd_lc"); a.W(Ppc.Addi(10, 10, 0x20));
            a.Label("fd_lc");
            a.W(Ppc.Xor(30, 30, 10)); a.W(Ppc.Mullw(30, 30, 28)); a.W(Ppc.Mullw(29, 29, 27)); a.W(Ppc.Xor(29, 29, 10));
            a.W(Ppc.Addi(31, 31, 1)); a.Br("fd_hl");
            // lower bound of h1 in the sorted table: r26 lo, r25 hi, r24 table
            a.Label("fd_bs"); a.W(Ppc.Li(26, 0)); a.La(25, pk.Count); LaD(a, 24, HASHES);
            a.Label("fd_bl"); a.W(Ppc.Cmplw(26, 25)); a.Bc("ge", "fd_scan");
            a.W(Ppc.Add(12, 26, 25)); a.W(Ppc.Rlwinm(12, 12, 31, 1, 31)); a.W(Ppc.Rlwinm(11, 12, 2, 0, 29)); a.W(Ppc.Lwzx(0, 24, 11));
            a.W(Ppc.Cmplw(0, 30)); a.Bc("lt", "fd_up"); a.W(Ppc.Mr(25, 12)); a.Br("fd_bl");
            a.Label("fd_up"); a.W(Ppc.Addi(26, 12, 1)); a.Br("fd_bl");
            // every entry with this h1: read its 16-byte record (offset, size, h2) and compare h2
            a.Label("fd_scan"); a.La(9, pk.Count); a.W(Ppc.Cmplw(26, 9)); a.Bc("ge", "fd_no");
            a.W(Ppc.Rlwinm(11, 26, 2, 0, 29)); a.W(Ppc.Lwzx(0, 24, 11)); a.W(Ppc.Cmplw(0, 30)); a.Bc("ne", "fd_no");
            a.W(Ppc.Rlwinm(25, 26, 4, 0, 27)); a.La(9, pk.EntriesOff); a.W(Ppc.Add(25, 25, 9));      // r25 = record offset in the pack
            LaD(a, 3, FIP); LaD(a, 4, BB); a.W(Ppc.Li(5, 0x20)); a.W(Ppc.Rlwinm(6, 25, 0, 0, 26)); a.W(Ppc.Li(7, 2)); a.Bl(DvdReadPrio);
            a.W(Ppc.Cmpwi(3, 0)); a.Bc("lt", "fd_no");
            LaD(a, 12, BB); a.W(Ppc.Rlwinm(0, 25, 0, 27, 31)); a.W(Ppc.Add(12, 12, 0));
            a.W(Ppc.Lwz(0, 8, 12)); a.W(Ppc.Cmplw(0, 29)); a.Bc("eq", "fd_hit");
            a.W(Ppc.Addi(26, 26, 1)); a.Br("fd_scan");
            a.Label("fd_hit"); a.W(Ppc.Lwz(4, 0, 12)); a.W(Ppc.Lwz(5, 4, 12)); a.W(Ppc.Lwz(6, 12, 12)); a.W(Ppc.Rlwinm(6, 6, 6, 0, 25)); LaD(a, 7, FIP); a.W(Ppc.Add(6, 6, 7)); a.W(Ppc.Li(3, 1)); a.Br("fd_ret");
            a.Label("fd_no"); a.W(Ppc.Li(3, 0));
            a.Label("fd_ret"); Epilog(a, 8);

            // FILE* fopen(path, mode)
            a.Label("fa_open"); Prolog(a, 5);
            Strip(a, 31, 3, "op"); LaD(a, 3, MTX); a.Bl(OsLockMutex);
            a.Bl("ex_init"); a.W(Ppc.Cmpwi(3, 0)); a.Bc("eq", "op_fail");
            a.W(Ppc.Mr(3, 31)); a.Bl("ex_find"); a.W(Ppc.Cmpwi(3, 0)); a.Bc("eq", "op_fail"); a.W(Ppc.Mr(29, 4)); a.W(Ppc.Mr(28, 5)); a.W(Ppc.Mr(27, 6));
            LaD(a, 30, T); a.W(Ppc.Li(0, NSlot)); a.W(Ppc.Mtctr(0));
            a.Label("op_find"); a.W(Ppc.Lwz(0, Used, 30)); a.W(Ppc.Cmpwi(0, 0)); a.Bc("eq", "op_got"); a.W(Ppc.Addi(30, 30, Slot));
            a.Bdnz("op_find"); a.Br("op_fail");
            a.Label("op_got"); a.W(Ppc.Li(0, 1)); a.W(Ppc.Stw(0, Used, 30)); a.W(Ppc.Li(0, 0)); a.W(Ppc.Stw(0, Pos, 30));
            a.W(Ppc.Stw(29, Base, 30)); a.W(Ppc.Stw(28, Size, 30)); a.W(Ppc.Stw(27, Fip, 30)); a.W(Ppc.Addi(0, 30, Size - 0x264)); a.W(Ppc.Stw(0, Obj, 30));
            a.W(Ppc.Mr(31, 30)); a.Br("op_ret");
            a.Label("op_fail"); a.W(Ppc.Li(31, 0));
            a.Label("op_ret"); LaD(a, 3, MTX); a.Bl(OsUnlockMutex); a.W(Ppc.Mr(3, 31)); Epilog(a, 5);
            // int fclose(FILE*)
            a.Label("fa_close");
            a.W(Ppc.Cmpwi(3, 0)); a.Bc("eq", "cl_ret"); a.W(Ppc.Li(0, 0)); a.W(Ppc.Stw(0, Used, 3));
            a.Label("cl_ret"); a.W(Ppc.Li(3, 0)); a.W(Ppc.Blr);
            // int fseek(FILE*, long off, int whence)
            a.Label("fa_seek");
            a.W(Ppc.Cmpwi(5, 1)); a.Bc("eq", "sk_cur"); a.W(Ppc.Cmpwi(5, 2)); a.Bc("eq", "sk_end");
            a.W(Ppc.Stw(4, Pos, 3)); a.W(Ppc.Li(3, 0)); a.W(Ppc.Blr);
            a.Label("sk_cur"); a.W(Ppc.Lwz(0, Pos, 3)); a.W(Ppc.Add(0, 0, 4)); a.W(Ppc.Stw(0, Pos, 3)); a.W(Ppc.Li(3, 0)); a.W(Ppc.Blr);
            a.Label("sk_end"); a.W(Ppc.Lwz(0, Size, 3)); a.W(Ppc.Add(0, 0, 4)); a.W(Ppc.Stw(0, Pos, 3)); a.W(Ppc.Li(3, 0)); a.W(Ppc.Blr);
            // size_t fread(buf, size, n, FILE*): r31 f, r30 buf, r29 size, r28 remaining, r27 done, r26 pos, r25 aligned off, r24 skip, r23 chunk, r22 base, r21 pack
            a.Label("fa_read"); Prolog(a, 11);
            a.W(Ppc.Mr(31, 6)); a.W(Ppc.Mr(30, 3)); a.W(Ppc.Mr(29, 4)); a.W(Ppc.Mullw(28, 4, 5)); a.W(Ppc.Li(27, 0));
            a.W(Ppc.Cmpwi(29, 0)); a.Bc("eq", "rd_zero");
            LaD(a, 3, MTX); a.Bl(OsLockMutex);
            a.W(Ppc.Lwz(22, Base, 31)); a.W(Ppc.Lwz(21, Fip, 31));
            a.W(Ppc.Lwz(26, Pos, 31)); a.W(Ppc.Lwz(0, Size, 31)); a.W(Ppc.Cmpwi(26, 0)); a.Bc("lt", "rd_done"); a.W(Ppc.Cmpw(26, 0)); a.Bc("ge", "rd_done");
            a.W(Ppc.Subf(0, 26, 0)); a.W(Ppc.Cmplw(28, 0)); a.Bc("le", "rd_loop"); a.W(Ppc.Mr(28, 0));
            a.Label("rd_loop"); a.W(Ppc.Cmpwi(28, 0)); a.Bc("le", "rd_done");
            // fast path: destination 32-aligned, position 4-aligned (pack offsets are 32-aligned) -> read (remaining & ~31) straight into the caller's buffer
            a.W(Ppc.Add(4, 30, 27)); a.W(Ppc.Rlwinm(0, 4, 0, 27, 31)); a.W(Ppc.Cmpwi(0, 0)); a.Bc("ne", "rd_slow");
            a.W(Ppc.Rlwinm(0, 26, 0, 30, 31)); a.W(Ppc.Cmpwi(0, 0)); a.Bc("ne", "rd_slow");
            a.W(Ppc.Rlwinm(23, 28, 0, 0, 26)); a.W(Ppc.Cmpwi(23, 0)); a.Bc("eq", "rd_slow");
            a.W(Ppc.Mr(3, 21)); a.W(Ppc.Mr(5, 23)); a.W(Ppc.Add(6, 26, 22)); a.W(Ppc.Li(7, 2)); a.Bl(DvdReadPrio);
            a.W(Ppc.Cmpwi(3, 0)); a.Bc("lt", "rd_done");
            a.W(Ppc.Add(26, 26, 23)); a.W(Ppc.Add(27, 27, 23)); a.W(Ppc.Subf(28, 23, 28)); a.Br("rd_loop");
            a.Label("rd_slow");
            a.W(Ppc.Add(0, 26, 22)); a.W(Ppc.Rlwinm(25, 0, 0, 0, 26)); a.W(Ppc.Subf(24, 25, 0));
            a.W(Ppc.Li(23, Bounce)); a.W(Ppc.Subf(23, 24, 23)); a.W(Ppc.Cmplw(23, 28)); a.Bc("le", "rd_c"); a.W(Ppc.Mr(23, 28));
            a.Label("rd_c");
            a.W(Ppc.Add(5, 24, 23)); a.W(Ppc.Addi(5, 5, 31)); a.W(Ppc.Rlwinm(5, 5, 0, 0, 26));
            a.W(Ppc.Mr(3, 21)); LaD(a, 4, BB); a.W(Ppc.Mr(6, 25)); a.W(Ppc.Li(7, 2)); a.Bl(DvdReadPrio);
            a.W(Ppc.Cmpwi(3, 0)); a.Bc("lt", "rd_done");
            LaD(a, 4, BB); a.W(Ppc.Add(4, 4, 24)); a.W(Ppc.Add(5, 30, 27)); a.W(Ppc.Mtctr(23));
            a.Label("rd_cp"); a.W(Ppc.Lbz(0, 0, 4)); a.W(Ppc.Stb(0, 0, 5)); a.W(Ppc.Addi(4, 4, 1)); a.W(Ppc.Addi(5, 5, 1)); a.Bdnz("rd_cp");
            a.W(Ppc.Add(26, 26, 23)); a.W(Ppc.Add(27, 27, 23)); a.W(Ppc.Subf(28, 23, 28)); a.Br("rd_loop");
            a.Label("rd_done"); a.W(Ppc.Stw(26, Pos, 31)); LaD(a, 3, MTX); a.Bl(OsUnlockMutex); a.W(Ppc.Divwu(3, 27, 29)); a.Br("rd_ret");
            a.Label("rd_zero"); a.W(Ppc.Li(3, 0));
            a.Label("rd_ret"); Epilog(a, 11);
            // int fstat(path, struct stat*): st[0] = size
            a.Label("fa_stat"); Prolog(a, 3);
            a.W(Ppc.Mr(30, 4)); a.W(Ppc.Mr(29, 3)); LaD(a, 3, MTX); a.Bl(OsLockMutex);
            a.W(Ppc.Li(31, -1)); a.Bl("ex_init"); a.W(Ppc.Cmpwi(3, 0)); a.Bc("eq", "st_ret");
            Strip(a, 3, 29, "st"); a.Bl("ex_find"); a.W(Ppc.Cmpwi(3, 0)); a.Bc("eq", "st_ret");
            a.W(Ppc.Stw(5, 0, 30)); a.W(Ppc.Li(31, 0));
            a.Label("st_ret"); LaD(a, 3, MTX); a.Bl(OsUnlockMutex); a.W(Ppc.Mr(3, 31)); Epilog(a, 3);
        }

        static void BssClear(Asm a, uint arenaHi, uint below, uint dataSize, bool mem2)
        {
            // runs from __init_data (once per .bss section), before OSInit. On the first call the EX RAM block + hash table are placed:
            //  mem2: <below> bytes under the MEM2 arena top (0x80003128, lowered to the block); the MEM1 arena top is set to arenaHi if given
            //  else: under the FST the apploader placed (0x80000038) and its BI2 block (0x800000F4); the MEM1 arena top OSInit uses
            //        (0x80003110, else BI2 0x80000034) becomes min(arenaHi, block) (arenaHi 0: the block itself)
            // bss_clear(r3 = start, r4 = 0, r5 = size): memset every gap of [start, start+size) not covered by the protect table
            a.Label("bss");
            a.W(Ppc.Stwu(1, -0x20, 1)); a.W(Ppc.Mflr(0)); a.W(Ppc.Stw(0, 0x24, 1)); a.W(Ppc.Stmw(27, 0x0C, 1));
            a.W(Ppc.Mr(31, 3)); a.W(Ppc.Add(30, 3, 5));
            a.W((pc, l) => Ppc.Lis(10, (int)(((l["ram"] + 0x8000) >> 16) & 0xFFFF))); a.W((pc, l) => Ppc.Lwz(0, (int)(short)(l["ram"] & 0xFFFF), 10));
            a.W(Ppc.Cmpwi(0, 0)); a.Bc("ne", "placed");
            a.W(Ppc.Lis(11, 0x8000));
            if (mem2)
            {
                a.W(Ppc.Lwz(12, 0x3128, 11)); a.W(Ppc.Cmpwi(12, 0)); a.Bc("ne", "m2_ok"); a.La(12, 0x935E0000);
                a.Label("m2_ok"); a.La(10, below); a.W(Ppc.Subf(12, 10, 12)); a.W(Ppc.Rlwinm(12, 12, 0, 0, 26)); a.W(Ppc.Stw(12, 0x3128, 11));
                if (arenaHi != 0) { a.La(9, arenaHi); a.W(Ppc.Stw(9, 0x34, 11)); a.W(Ppc.Stw(9, 0x3110, 11)); }
            }
            else
            {
                // below the FST (0x80000038) and the BI2 block the apploader leaves under it (0x800000F4)
                a.W(Ppc.Lwz(12, 0x38, 11)); a.W(Ppc.Lwz(10, 0xF4, 11)); a.W(Ppc.Cmplw(10, 12)); a.Bc("ge", "bi2_ok");
                a.W(Ppc.Rlwinm(0, 10, 4, 28, 31)); a.W(Ppc.Cmplwi(0, 8)); a.Bc("ne", "bi2_ok"); a.W(Ppc.Mr(12, 10));     // only a MEM1 address
                a.Label("bi2_ok"); a.La(10, below); a.W(Ppc.Subf(12, 10, 12)); a.W(Ppc.Rlwinm(12, 12, 0, 0, 26));
                a.W(Ppc.Mr(9, 12));
                if (arenaHi != 0) { a.La(10, arenaHi); a.W(Ppc.Cmplw(10, 12)); a.Bc("ge", "ah_min"); a.W(Ppc.Mr(9, 10)); a.Label("ah_min"); }
                a.W(Ppc.Stw(9, 0x34, 11)); a.W(Ppc.Stw(9, 0x3110, 11));
            }
            a.W((pc, l) => Ppc.Lis(10, (int)(((l["ram"] + 0x8000) >> 16) & 0xFFFF))); a.W((pc, l) => Ppc.Stw(12, (int)(short)(l["ram"] & 0xFFFF), 10));
            a.W(Ppc.Mr(3, 12)); a.W(Ppc.Li(4, 0)); a.La(5, dataSize); a.Bl(Memset);
            a.Label("placed");
            a.W((pc, l) => Ppc.Lis(29, (int)(((l["table"] + 0x8000) >> 16) & 0xFFFF))); a.W((pc, l) => Ppc.Addi(29, 29, (int)(l["table"] & 0xFFFF)));
            a.Label("loop");
            a.W(Ppc.Lwz(28, 0, 29)); a.W(Ppc.Cmpwi(28, 0)); a.Bc("eq", "tail");
            a.W(Ppc.Lwz(27, 4, 29)); a.W(Ppc.Addi(29, 29, 8));
            a.W(Ppc.Cmplw(28, 30)); a.Bc("le", "lo_ok"); a.W(Ppc.Mr(28, 30));
            a.Label("lo_ok");
            a.W(Ppc.Cmplw(28, 31)); a.Bc("le", "skip");
            a.W(Ppc.Mr(3, 31)); a.W(Ppc.Li(4, 0)); a.W(Ppc.Subf(5, 31, 28)); a.Bl(Memset);
            a.Label("skip");
            a.W(Ppc.Cmplw(27, 31)); a.Bc("le", "loop"); a.W(Ppc.Mr(31, 27));
            a.Br("loop");
            a.Label("tail");
            a.W(Ppc.Cmplw(30, 31)); a.Bc("le", "done");
            a.W(Ppc.Mr(3, 31)); a.W(Ppc.Li(4, 0)); a.W(Ppc.Subf(5, 31, 30)); a.Bl(Memset);
            a.Label("done");
            a.W(Ppc.Lmw(27, 0x0C, 1)); a.W(Ppc.Lwz(0, 0x24, 1)); a.W(Ppc.Mtlr(0)); a.W(Ppc.Addi(1, 1, 0x20)); a.W(Ppc.Blr);
        }

        /// <summary>assemble at <paramref name="baseAddr"/>. RAM block + hash table: <paramref name="fstMargin"/> below the FST (or at
        /// top of MEM2 (<paramref name="mem2"/>)); <paramref name="arenaHi"/>: fixed MEM1 arena top (0: leave / the block)</summary>
        public static (byte[] Code, Dictionary<string, uint> Labels) Build(uint baseAddr, IEnumerable<(uint Lo, uint Hi)> protect, PackInfo pack, uint arenaHi, uint fstMargin, bool mem2)
        {
            var a = new Asm(baseAddr);
            uint ds = DataSize(pack.Paths.Count);
            BssClear(a, arenaHi, ((mem2 ? 0 : fstMargin) + ds + pack.IndexLoad + 0x1F) & ~0x1Fu, ds, mem2);
            SdDisc(a, pack);
            a.Label("table");
            foreach (var r in protect.OrderBy(r => r.Lo)) { a.W(r.Lo); a.W(r.Hi); }
            a.W(0); a.W(0);
            a.Label("ram"); a.W(0);
            a.Label("paths");
            foreach (var p in pack.Paths)
            {
                var b = new byte[0x10]; var t = System.Text.Encoding.ASCII.GetBytes(p); Bin.Require(t.Length < 0x10, "pack path too long"); t.CopyTo(b, 0);
                for (int k = 0; k < 0x10; k += 4) a.W(Bin.Get32(b, k));
            }
            return (a.Bytes(), a.Labels);
        }
    }
}
