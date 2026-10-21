using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Text;

namespace BrawlBuilder.EX
{
    /// <summary>
    /// The mod's SD folder as a few big files on the disc (ex0.pak, ex1.pak, ... each under 2 GiB: DVD read offsets are signed 32-bit),
    /// so the FST (which lives at the top of MEM1) stays about vanilla-sized. ex0.pak starts with the index:
    ///   0x00  "EXPK", count, records offset, pack count
    ///   0x20  h1[count]                               sorted; this part is loaded into memory (Shims ex_init)
    ///   R     {offset, size, h2, pack}[count]          R 32-aligned, same order as h1; one record is read per open
    /// then file data, every file 32-aligned, in path order (files of one folder stay together on the disc).
    /// Key: the path below the SD root's parent ("ex_remix/pf/menu/boot/boot.pac"), ASCII lower case, as the shims hash it.
    /// </summary>
    public static class SdPack
    {
        public const long MaxPack = 0x7FF00000;

        public static (uint H1, uint H2) Hash(string key)
        {
            uint h1 = 0x811C9DC5, h2 = 5381;
            foreach (char ch in key)
            {
                uint c = ch >= 'A' && ch <= 'Z' ? (uint)ch + 0x20 : ch;
                h1 = (h1 ^ c) * 0x01000193; h2 = (h2 * 33) ^ c;
            }
            return (h1, h2);
        }

        public static uint IndexLoad(int count) => (uint)((0x20 + 4 * count + 0x1F) & ~0x1F);
        public static string PackName(int i) => "ex" + i + ".pak";

        class Item { public string File, Key; public uint H1, H2; public long Len, Off; public int Pack; }

        static List<Item> Plan(string files, string root, out int packs, out uint records)
        {
            var dir = Path.Combine(files, root);
            var items = Directory.EnumerateFiles(dir, "*", SearchOption.AllDirectories).Select(f =>
            {
                var key = root + "/" + Path.GetRelativePath(dir, f).Replace(Path.DirectorySeparatorChar, '/'); var h = Hash(key);
                return new Item { File = f, Key = key, H1 = h.H1, H2 = h.H2, Len = new FileInfo(f).Length };
            }).OrderBy(x => x.Key, StringComparer.OrdinalIgnoreCase).ToList();
            records = IndexLoad(items.Count);
            long off = (records + 16L * items.Count + 0x1F) & ~0x1FL; int p = 0;
            foreach (var it in items)
            {
                Bin.Require(it.Len < MaxPack, it.Key + " is too big for a disc file");
                if (off + it.Len > MaxPack) { p++; off = 0; }
                it.Pack = p; it.Off = off; off = (off + it.Len + 0x1F) & ~0x1FL;
            }
            packs = p + 1;
            return items;
        }

        /// <summary>file count and pack file names the folder will need (for the FST size, before anything is written)</summary>
        public static (int Files, List<string> Names) Layout(string files, string root)
        {
            var items = Plan(files, root, out int packs, out _);
            return (items.Count, Enumerable.Range(0, packs).Select(PackName).ToList());
        }

        /// <summary>packs files/&lt;root&gt;/ into files/ex*.pak and deletes the folder</summary>
        public static Shims.PackInfo Write(string files, string root)
        {
            var items = Plan(files, root, out int packs, out uint records);
            var byHash = items.OrderBy(x => x.H1).ThenBy(x => x.H2).ToList();
            for (int i = 1; i < byHash.Count; i++)
                Bin.Require(byHash[i].H1 != byHash[i - 1].H1 || byHash[i].H2 != byHash[i - 1].H2, "hash collision: " + byHash[i].Key + " / " + byHash[i - 1].Key);
            uint n = (uint)items.Count;
            var head = new byte[(records + 16 * n + 0x1F) & ~0x1Fu];
            Encoding.ASCII.GetBytes("EXPK").CopyTo(head, 0); Bin.Put32(head, 4, n); Bin.Put32(head, 8, records); Bin.Put32(head, 12, (uint)packs);
            for (int i = 0; i < byHash.Count; i++)
            {
                var it = byHash[i]; int r = (int)(records + 16 * i);
                Bin.Put32(head, 0x20 + 4 * i, it.H1);
                Bin.Put32(head, r, (uint)it.Off); Bin.Put32(head, r + 4, (uint)it.Len); Bin.Put32(head, r + 8, it.H2); Bin.Put32(head, r + 12, (uint)it.Pack);
            }
            var pad = new byte[0x20];
            for (int p = 0; p < packs; p++)
                using (var o = new FileStream(Path.Combine(files, PackName(p)), FileMode.Create, FileAccess.Write, FileShare.None, 1 << 20))
                {
                    if (p == 0) o.Write(head, 0, head.Length);
                    foreach (var it in items.Where(x => x.Pack == p))
                    {
                        Bin.Require(o.Position == it.Off, "pack layout mismatch");
                        using (var s = File.OpenRead(it.File)) s.CopyTo(o, 1 << 20);
                        int r = (int)(o.Position & 0x1F); if (r != 0) o.Write(pad, 0, 0x20 - r);
                    }
                }
            Directory.Delete(Path.Combine(files, root), true);
            return new Shims.PackInfo { Paths = Enumerable.Range(0, packs).Select(i => "/" + PackName(i)).ToList(), Count = n, EntriesOff = records, IndexLoad = IndexLoad(items.Count) };
        }
    }
}
