using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Text;

namespace BrawlBuilder.EX
{
    /// <summary>
    /// BrawlBuilderEX: turn an extracted vanilla Brawl USA Rev 2 tree (wit layout: sys/, files/) plus a launcher-based mod into a
    /// self-contained disc tree - no SD card, no launcher, no cheat file.  Nothing in here is specific to one mod:
    ///   1. SD card on the disc  : the mod folder goes onto the disc unchanged as one pack file (SdPack) and Brawl's FA (SD) file API is
    ///                              redirected to it (Shims), so every SD reader of the mod keeps working
    ///   2. launcher pre-pass    : the code handler (from the mod's own launcher) and the code list are embedded in main.dol at the address
    ///                              the launcher used (gc.txt codeliststart - lists built with GCTRealMate -b point into themselves),
    ///                              hooked like the launcher (VBI), and the unconditional writes and C2 hooks are installed statically
    ///   3. memory layout        : the pack's hash table sits below the FST, so the arena top is lowered (before OSInit); mods that hard-code
    ///                              heaps at the vanilla top (BrawlEx's BrawlX heap, Syringe) get the heap and every pointer to it moved by
    ///                              the same amount; pointers into the vanilla FST name table get their own copy of the string
    /// </summary>
    public class ExBuild
    {
        public const string VanillaMainDolSha1 = "a09cb9af20ae8356396374c27e31883d893f9ebe";
        const uint HandlerAddr = 0x80001800, HandlerEntry = 0x800018A8, VbiHook = 0x80200984, BssClearCall = 0x800042B8, GameIdAddr = 0x805A14B8, SdMounted = 0x805664EC;
        const uint BssLo = 0x80494880, BssHi = 0x805A5154, LowLo = 0x800022C0, LowHi = 0x80003000;

        public Action<string> Log = s => { };
        public List<string> Report = new List<string>();

        public const long DiscMax = 8511160320L * 31 / 32 - 96L * 1024 * 1024;   // file data a dual-layer Wii disc holds (31 of every 32 KiB; the rest is hashes), minus headroom
        public bool KeepShadowed;                                         // diagnosis: keep vanilla files the mod replaces
        public bool FreeDiscSpace = true;                                 // empty shadowed vanilla files / fit the disc (off: diagnosis, folder boot)
        public bool Mem2Index;                                            // diagnostic: put the SD index in MEM2
        // the EX RAM block + hash table are placed at boot FstMargin below the FST; the arena/heap move is sized for an FST up to
        // FstSlack bigger than this builder computes (Dolphin lays out folder-boot FSTs ~8 KiB bigger than wit images)
        const uint Mem2Top = 0x935E0000, FstMargin = 0x100, FstSlack = 0x6000;                 // (+ the 8 KiB BI2 block under the FST)
        public bool Bake = true;                                          // install unconditional writes / C2 hooks into main.dol (off: all at runtime, for diagnosis)
        public bool RemoveSubspace;                                       // delete the Subspace Emissary files
        public bool Fit = true;                                           // shrink optional vanilla content until the image fits a dual-layer disc
        public string SubspaceList;                                       // Resources/SubspaceEmissaryFiles.txt

        public void Run(string tree, ExMod m, string gameId)
        {
            string files = Path.Combine(tree, "files"), sys = Path.Combine(tree, "sys");
            var vanDol = File.ReadAllBytes(Path.Combine(sys, "main.dol"));
            Bin.Require(Bin.Sha1(vanDol) == VanillaMainDolSha1, "sys/main.dol is not the vanilla Brawl USA Rev 2 DOL (sha1 " + Bin.Sha1(vanDol) + ")");
            var vanFst = File.ReadAllBytes(Path.Combine(sys, "fst.bin"));
            Bin.Require(ArenaHi((uint)vanFst.Length) == GeckoList.VanArenaHi, "sys/fst.bin is not the vanilla FST");

            // 1. the mod's SD folder onto the disc
            var sd = m.SdFiles(out int skipped);
            foreach (var (src, disc) in sd)
            {
                var dst = Path.Combine(files, disc.Replace('/', Path.DirectorySeparatorChar));
                Directory.CreateDirectory(Path.GetDirectoryName(dst)); if (File.Exists(dst)) File.Delete(dst); File.Copy(src, dst);
            }
            Log(string.Format("SD folder /{0}: {1} files copied to the disc{2}", m.Root, sd.Count, skipped > 0 ? " (" + skipped + " non-ASCII names skipped)" : ""));
            if (FreeDiscSpace) FreeSpace(tree, m, sd);

            // 2. code list
            var gct = GeckoList.Parse(File.ReadAllBytes(m.GctPath));
            List<GeckoCode> codes = null;
            if (m.CodesetPath != null) codes = GeckoList.FromCodeset(File.ReadAllText(m.CodesetPath), gct);
            if (codes == null) codes = GeckoList.SplitAtTerminators(gct);
            Log(string.Format("code list: {0} lines, {1} codes{2}", gct.Count, codes.Count, m.CodesetPath != null && codes.Count > 0 && codes[0].Name != "code 0" ? " (names from codeset)" : ""));

            // 3. memory layout
            var extraGcts = sd.Where(x => x.Disc.EndsWith(".gct", StringComparison.OrdinalIgnoreCase) && !SamePath(x.Src, m.GctPath)).ToList();
            bool usesHeap = UsesHeap(codes, extraGcts, sd);
            // the SD folder becomes one pack file (SdPack): the FST stays about vanilla-sized and only the pack's hash table (4 bytes a file)
            // needs room below it, made by lowering the arena top before OSInit; hard-coded heaps at the vanilla top move by whole 64 KiB steps
            string rootDir = Path.Combine(files, m.Root);
            var (nFiles, packNames) = SdPack.Layout(files, m.Root);
            uint fstAfter = FstSize(files, rootDir) + (uint)packNames.Sum(x => 12 + x.Length + 1);
            uint dataSize = Shims.DataSize(packNames.Count);
            uint fstStart = ArenaHi(fstAfter), dataAddr = (fstStart - FstSlack - FstMargin - SdPack.IndexLoad(nFiles) - dataSize) & ~0x1Fu, idxAddr = dataAddr + dataSize;
            int n = !usesHeap || dataAddr >= GeckoList.VanArenaHi ? 0 : (int)((GeckoList.VanArenaHi - dataAddr + 0xFFFF) / 0x10000);
            int delta = -0x10000 * n; uint targetHi = usesHeap ? (uint)(GeckoList.VanArenaHi + delta) : dataAddr;
            if (Mem2Index)
            {
                // diagnostic: index + EX RAM at the top of the MEM2 arena, MEM1 and the hard-coded heaps exactly vanilla
                Bin.Require(fstStart >= GeckoList.VanArenaHi, "--mem2-index needs an FST no bigger than vanilla");
                dataAddr = (Mem2Top - SdPack.IndexLoad(nFiles) - dataSize) & ~0x1Fu; idxAddr = dataAddr + dataSize; delta = 0; targetHi = GeckoList.VanArenaHi; usesHeap = true;
            }
            Log(string.Format("SD pack: {0} files in " + packNames.Count + " pack file(s), hash table 0x{1:X} bytes (+ EX RAM) placed at boot below the FST (~0x{5:X8}); arena top 0x{3:X8}{4}", nFiles, SdPack.IndexLoad(nFiles), idxAddr, targetHi,
                usesHeap ? string.Format(", BrawlX/Syringe heap moved by -0x{0:X}", -delta) : " (no hard-coded heap in this mod)", dataAddr));
            if (delta != 0)
                foreach (var (src, disc) in sd.Where(x => System.Text.RegularExpressions.Regex.IsMatch(x.Disc, @"(?i)/system/common2(_en)?\.pac$")))
                {
                    var raw = File.ReadAllBytes(src); if (!Lz10.IsCompressed(raw)) continue;
                    var dec = Lz10.Decompress(raw); int hits = 0;
                    for (int i = 0; i + 4 <= dec.Length; i += 1) if (Bin.Get32(dec, i) == GeckoList.HeapOld) { Bin.Put32(dec, i, (uint)(GeckoList.HeapOld + delta)); hits++; }
                    if (hits > 0) { Put(Path.Combine(files, disc.Replace('/', Path.DirectorySeparatorChar)), Lz10.Compress(dec, raw[0] == 0x11)); Report.Add(disc + ": " + hits + " heap-base relocation addends moved"); }
                }

            // code fix-ups (heap pointers, FST-name pointers)
            uint cl = m.CodeListStart, clSec = cl & ~0x1Fu;
            uint glen = 16 + 8u * (uint)codes.Sum(c => c.Lines.Count);
            uint strBase = (cl + glen + 0x1F) & ~0x1Fu;
            var strBlob = new List<byte>(); var strings = new Dictionary<string, uint>();
            Func<uint, byte[]> fstName = a => { long o = a - GeckoList.VanArenaHi; if (o < 0 || o >= vanFst.Length) return null; int e = Array.IndexOf(vanFst, (byte)0, (int)o); return vanFst.Skip((int)o).Take(e - (int)o + 1).ToArray(); };
            Func<byte[], uint> stringAddr = s => { var key = Convert.ToBase64String(s); if (!strings.ContainsKey(key)) { strings[key] = strBase + (uint)strBlob.Count; strBlob.AddRange(s); } return strings[key]; };
            var van = new Dol(vanDol);
            foreach (var c in codes)
            {
                var notes = GeckoList.FixHeapAndFst(c, delta, fstName, stringAddr, van.Read);
                if (notes.Count > 0) Report.Add(c.Name + ": " + string.Join("; ", notes));
            }
            foreach (var (src, disc) in extraGcts)
            {
                var raw = File.ReadAllBytes(src); if (raw.Length < 16 || Bin.Get32(raw, 0) != 0x00D0C0DE) continue;
                var parts = GeckoList.SplitAtTerminators(GeckoList.Parse(raw)); var notes = new List<string>();
                foreach (var c in parts) notes.AddRange(GeckoList.FixHeapAndFst(c, delta, fstName, stringAddr, van.Read));
                if (notes.Count > 0) { Put(Path.Combine(files, disc.Replace('/', Path.DirectorySeparatorChar)), GeckoList.Build(parts)); Report.Add(disc + ": " + string.Join("; ", notes)); }
            }
            // every edit of the SD copy is done: pack it
            var pk = SdPack.Write(files, m.Root);
            Bin.Require(ArenaHi(FstSize(files)) == fstStart, "FST size after packing differs from the estimate");
            var g = GeckoList.Build(codes);
            uint dataEnd = (strBase + (uint)strBlob.Count + 0x1F) & ~0x1Fu;
            Bin.Require(dataEnd <= m.CodeListEnd, "the code list does not fit below codelistend (0x" + m.CodeListEnd.ToString("X8") + ")");

            // 4. main.dol
            var dol = new Dol(vanDol);
            dol.Write(VbiHook, Ppc.B(VbiHook, HandlerEntry), Ppc.Blr);
            foreach (var p in m.Pokes)
            {
                if (p.WriteAddr == BssClearCall) continue;                     // replaced by our protecting .bss clear
                if (!p.IfEqual.HasValue || (dol.Section(p.Addr) >= 0 && dol.Read(p.Addr) == p.IfEqual.Value)) dol.Write(p.WriteAddr, p.Value);
            }
            if (!string.IsNullOrEmpty(gameId) && gameId != "RSBE01")
            {
                Bin.Require(gameId.Length == 6 && gameId.StartsWith("RSBE"), "game id must be RSBExx");
                dol.WriteBytes(GameIdAddr, Encoding.ASCII.GetBytes(gameId.Substring(4)));
            }
            // pre-pass: unconditional writes and C2 hooks
            var extra = new SortedDictionary<uint, byte>(); int nin = 0, nbss = 0, nrt = 0, nhook = 0; int gline = 0;
            foreach (var c in codes)
            {
                int baseLine = gline; gline += c.Lines.Count;
                var writes = new List<(uint, byte[])>(); var hooks = new List<(int, uint, int)>();
                if (Bake) GeckoList.StaticOps(c.Lines, writes, hooks);
                foreach (var (hdr, hk, nl) in hooks)
                {
                    int si = dol.Section(hk); if (si < 0 || si >= 7) continue;
                    uint pay = cl + 8 + 8u * (uint)(baseLine + hdr + 1), last = pay + 8u * (uint)nl - 4; int o = (int)(last - cl);
                    if (Bin.Get32(g, o) != 0) { Report.Add("C2 " + hk.ToString("X8") + " left to the runtime handler"); continue; }
                    dol.Write(hk, Ppc.B(hk, pay)); Bin.Put32(g, o, Ppc.B(last, hk + 4)); nhook++;
                }
                foreach (var (va, b) in writes)
                {
                    if (dol.Covers(va, b.Length)) { dol.WriteBytes(va, b); nin++; }
                    else if (va >= BssLo && va + b.Length <= BssHi && dol.Free(va, b.Length)) { for (int k = 0; k < b.Length; k++) extra[va + (uint)k] = b[k]; nbss++; }
                    else nrt++;
                }
            }
            var blobs = new List<(uint Lo, byte[] B)>();
            foreach (var va in extra.Keys)
            {
                uint lo = va & ~0x1Fu;
                if (blobs.Count > 0 && lo <= blobs[blobs.Count - 1].Lo + (uint)blobs[blobs.Count - 1].B.Length + 0x4000)
                {
                    var bl = blobs[blobs.Count - 1]; uint hi = Math.Max(bl.Lo + (uint)bl.B.Length, lo + 0x20);
                    if (hi > bl.Lo + bl.B.Length) { var nb = new byte[hi - bl.Lo]; Array.Copy(bl.B, nb, bl.B.Length); blobs[blobs.Count - 1] = (bl.Lo, nb); }
                }
                else blobs.Add((lo, new byte[0x20]));
                var cur = blobs[blobs.Count - 1]; cur.B[va - cur.Lo] = extra[va];
            }
            foreach (var bl in blobs) Bin.Require(!(bl.Lo < dataEnd && clSec < bl.Lo + bl.B.Length), "a baked write lands in the code list area");
            // Brawl's own "SD card mounted" flag: our shims are the SD card, and the File Patch Codes only run while it is set
            {
                uint lo = SdMounted & ~0x1Fu; var fb = new byte[0x20]; fb[SdMounted - lo + 3] = 1;
                int ix = blobs.FindIndex(b => b.Lo <= SdMounted && SdMounted < b.Lo + b.B.Length);
                if (ix >= 0) blobs[ix].B[SdMounted - blobs[ix].Lo + 3] = 1; else blobs.Add((lo, fb));
            }
            Log(string.Format("launcher pre-pass: {0} writes into main.dol, {1} into .bss, {2} runtime-only, {3} C2 hooks installed", nin, nbss, nrt, nhook));
            // handler, EX helpers (text), code list + strings + helper data (data), baked .bss blocks
            var protect = new List<(uint, uint)> { (clSec, dataEnd) }; protect.AddRange(blobs.Select(b => (b.Lo, b.Lo + (uint)b.B.Length)));
            // the EX helpers go in the low-memory space after the handler where no code of the mod writes (P+'s VBI hook switch uses
            // 0x80002778, others keep data there), else after the code list
            uint shimLen = (uint)Shims.Build(0x80002000, protect.Concat(new[] { (0u, 0u) }), pk, usesHeap ? targetHi : 0, FstMargin, Mem2Index ? dataAddr : 0).Code.Length;
            var used = GeckoList.WritesIn(gct, LowLo, LowHi);
            foreach (var (src, _) in extraGcts) { var raw = File.ReadAllBytes(src); if (raw.Length >= 16 && Bin.Get32(raw, 0) == 0x00D0C0DE) used.AddRange(GeckoList.WritesIn(GeckoList.Parse(raw), LowLo, LowHi)); }
            uint shimBase = 0;
            for (uint at = LowLo; at + shimLen <= LowHi; )
            {
                var hit = used.Where(u => u.Lo < at + shimLen && u.Hi > at).ToList();
                if (hit.Count == 0) { shimBase = at; break; }
                at = (hit.Max(u => u.Hi) + 0x1F) & ~0x1Fu;
            }
            if (shimBase == 0)
            {
                shimBase = dataEnd; protect.Add((shimBase, (shimBase + shimLen + 0x1F) & ~0x1Fu));
                Bin.Require(shimBase + shimLen <= m.CodeListEnd, "no room for the EX helpers (low memory is used by the mod's codes and the code list area is full)");
            }
            var (shim, labels) = Shims.Build(shimBase, protect, pk, usesHeap ? targetHi : 0, FstMargin, Mem2Index ? dataAddr : 0);
            Bin.Require(shim.Length <= shimLen, "EX helpers grew");
            var h = (byte[])m.Handler.Clone();
            Bin.Require(Bin.Get32(h, 0x104) == 0x3DE08000 && Bin.Get32(h, 0x108) == 0x61EF22A8, "unexpected handler template");
            Bin.Put32(h, 0x104, 0x3DE00000 | (cl >> 16)); Bin.Put32(h, 0x108, 0x61EF0000 | (cl & 0xFFFF));
            dol.Add(HandlerAddr, h, true);
            dol.Add(shimBase, shim, true);
            dol.Write(BssClearCall, Ppc.B(BssClearCall, labels["bss"], true), 0x48000185);
            foreach (var (name, addr, old) in Shims.FaEntries) dol.Write(addr, Ppc.B(addr, labels["fa_" + name]), old);
            var listSec = new byte[dataEnd - clSec];
            Array.Copy(g, 0, listSec, cl - clSec, g.Length); strBlob.ToArray().CopyTo(listSec, strBase - clSec);
            dol.Add(clSec, listSec, false);
            foreach (var bl in blobs) dol.Add(bl.Lo, bl.B, false);
            Put(Path.Combine(sys, "main.dol"), dol.ToArray());
            // the game compares its own id with the disc's
            if (!string.IsNullOrEmpty(gameId))
                foreach (var hp in new[] { Path.Combine(sys, "boot.bin"), Path.Combine(tree, "disc", "header.bin") })
                    if (File.Exists(hp)) { var hb = File.ReadAllBytes(hp); Encoding.ASCII.GetBytes(gameId).CopyTo(hb, 0); Put(hp, hb); }
            Log("main.dol: code handler @0x80001800, code list @0x" + cl.ToString("X8") + ", EX helpers @0x" + shimBase.ToString("X8") + ", SD file API -> disc");
        }

        /// <summary>
        /// Vanilla files the mod replaces are never read (the File Patch Code checks the mod's copy first): empty them, and the dummy padding
        /// files, keeping their FST entries. If the image is still bigger than a dual-layer disc, Subspace has to go.
        /// </summary>
        void FreeSpace(string tree, ExMod m, List<(string Src, string Disc)> sd)
        {
            string files = Path.Combine(tree, "files"); long freed = 0;
            var vanilla = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);
            foreach (var f in Directory.EnumerateFiles(files, "*", SearchOption.AllDirectories))
            {
                var rel = Path.GetRelativePath(files, f).Replace(Path.DirectorySeparatorChar, '/');
                if (!rel.StartsWith(m.Root + "/", StringComparison.OrdinalIgnoreCase)) vanilla[rel] = f;
            }
            string pfPrefix = Directory.Exists(Path.Combine(m.Dir, "pf")) ? m.Root + "/pf/" : m.Root + "/";
            foreach (var (_, disc) in sd)
            {
                if (!disc.StartsWith(pfPrefix, StringComparison.OrdinalIgnoreCase)) continue;
                var rel = disc.Substring(pfPrefix.Length);
                string hit = null;
                // exact paths only: a mod's X.pac does not always replace the localized X_en.pac (P+Ex ships 64-byte common.pac stubs
                // and the game still loads system/common_en.pac from the disc)
                if (vanilla.ContainsKey(rel)) hit = vanilla[rel];
                if (hit != null && KeepShadowed) continue;
                if (hit != null && new FileInfo(hit).Length > 0) { freed += new FileInfo(hit).Length; Put(hit, new byte[0]); }
            }
            foreach (var d in new[] { "dummy1.dat", "dummy2.dat", "border.dat" })
            { var f = Path.Combine(files, d); if (File.Exists(f)) { freed += new FileInfo(f).Length; Put(f, new byte[0]); } }
            long total = Directory.EnumerateFiles(tree, "*", SearchOption.AllDirectories).Sum(f => new FileInfo(f).Length);
            // still too big: least destructive first - vanilla movies to 10-frame placeholders, then Subspace (if allowed), then trophy models
            if (total > DiscMax && Fit)
            {
                int n = 0; var md = Path.Combine(files, "movie");
                if (Directory.Exists(md))
                    foreach (var f in Directory.GetFiles(md, "*.thp").OrderByDescending(f => new FileInfo(f).Length))
                    {
                        long len = new FileInfo(f).Length; if (len < 2000000 || total <= DiscMax) break;
                        var t = Thp.Truncate(File.ReadAllBytes(f), 10); Put(f, t); total -= len - t.Length; n++;
                    }
                if (n > 0) Report.Add("fit: " + n + " vanilla movies shortened to 10 frames");
            }
            if (total > DiscMax && (RemoveSubspace || Fit) && SubspaceList != null && File.Exists(SubspaceList))
            {
                int n = 0;
                foreach (var l in File.ReadLines(SubspaceList))
                {
                    var f = Path.Combine(files, l.Trim().Replace((char)92, Path.DirectorySeparatorChar));
                    if (l.Trim().Length > 0 && File.Exists(f)) { total -= new FileInfo(f).Length; File.Delete(f); n++; }
                }
                Report.Add("Subspace Emissary removed (" + n + " files) to fit the disc");
            }
            if (total > DiscMax && Fit)
            {
                int n = 0; var fd = Path.Combine(files, "toy", "fig");
                if (Directory.Exists(fd)) foreach (var f in Directory.GetFiles(fd)) { total -= new FileInfo(f).Length; Put(f, new byte[0]); n++; }
                if (n > 0) Report.Add("fit: " + n + " trophy models emptied (the Vault's trophy viewer will not work)");
            }
            Log(string.Format("disc space: {0:F2} GB of unused vanilla data emptied, image {1:F2} GB", freed / 1e9, total / 1e9));
            Bin.Require(total <= DiscMax, string.Format("the image would be {0:F2} GB, more than a dual-layer Wii disc holds ({1:F2} GB). ", total / 1e9, DiscMax / 1e9));
        }

        /// <summary>replace a file (delete first: never writes through a hard link into a shared copy)</summary>
        static void Put(string path, byte[] data) { if (File.Exists(path)) File.Delete(path); File.WriteAllBytes(path, data); }

        static bool SamePath(string a, string b) => string.Equals(Path.GetFullPath(a), Path.GetFullPath(b), StringComparison.OrdinalIgnoreCase);

        public static uint ArenaHi(uint fst) => (GeckoList.MemTop - fst) & ~0x1Fu;

        /// <summary>FST bytes for the tree under files/, optionally without one folder</summary>
        public static uint FstSize(string files, string without = null)
        {
            long n = 1, names = 0;
            foreach (var e in Directory.EnumerateFileSystemEntries(files, "*", SearchOption.AllDirectories))
            {
                if (without != null && (SamePath(e, without) || Path.GetFullPath(e).StartsWith(Path.GetFullPath(without) + Path.DirectorySeparatorChar, StringComparison.OrdinalIgnoreCase))) continue;
                n++; names += Encoding.UTF8.GetByteCount(Path.GetFileName(e)) + 1;
            }
            return (uint)((n * 12 + names + 3) & ~3L);
        }

        static bool UsesHeap(List<GeckoCode> codes, List<(string Src, string Disc)> extraGcts, List<(string Src, string Disc)> sd)
        {
            Func<GeckoCode, bool> probe = c =>
            {
                var copy = new GeckoCode { Name = c.Name, Lines = new List<Line>(c.Lines) };
                return GeckoList.FixHeapAndFst(copy, -0x10000, a => null, s => 0x80000000, a => 0).Any(x => !x.Contains("FST") && !x.Contains("without"));
            };
            if (codes.Any(probe)) return true;
            foreach (var (src, _) in extraGcts)
            {
                var raw = File.ReadAllBytes(src);
                if (raw.Length >= 16 && Bin.Get32(raw, 0) == 0x00D0C0DE && GeckoList.SplitAtTerminators(GeckoList.Parse(raw)).Any(probe)) return true;
            }
            foreach (var (src, disc) in sd.Where(x => System.Text.RegularExpressions.Regex.IsMatch(x.Disc, @"(?i)/system/common2(_en)?\.pac$")))
            {
                var raw = File.ReadAllBytes(src); if (!Lz10.IsCompressed(raw)) continue;
                var dec = Lz10.Decompress(raw);
                for (int i = 0; i + 4 <= dec.Length; i++) if (Bin.Get32(dec, i) == GeckoList.HeapOld) return true;
            }
            return false;
        }
    }
}
