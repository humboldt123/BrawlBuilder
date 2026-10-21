using System;
using System.Collections.Generic;
using System.IO;
using System.Text;

namespace BrawlBuilder.EX
{
    public class FatEntry
    {
        public string Name;          // long name if present, else 8.3 name
        public bool IsDir;
        public uint Size;
        public uint FirstCluster;
        public string Path;          // '/'-separated, no leading slash
    }

    /// <summary>Minimal read-only FAT16/FAT32 reader for a raw filesystem image (no mounting). Supports long file names and an optional MBR.
    /// Never writes: the image is opened FileAccess.Read.</summary>
    public class FatImage : IDisposable
    {
        readonly FileStream fs;
        long volStart;
        int bps, spc; long fatStart, dataStart, clusterBytes;
        uint rootCluster; long rootDirStart; int rootDirEntries;
        uint[] fat; uint clusterCount; bool isFat32;

        public FatImage(string path)
        {
            fs = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.Read, 1 << 16);
            var s0 = ReadAt(0, 512);
            volStart = 0;
            if (!LooksLikeBpb(s0))
            {
                // MBR: first non-empty partition
                Bin.Require(s0[510] == 0x55 && s0[511] == 0xAA, "sd image: neither a FAT boot sector nor an MBR");
                bool found = false;
                for (int i = 0; i < 4 && !found; i++)
                {
                    int o = 446 + 16 * i; byte type = s0[o + 4];
                    uint lba = BitConverter.ToUInt32(s0, o + 8);
                    if (type != 0 && lba != 0) { volStart = (long)lba * 512; found = true; }
                }
                Bin.Require(found, "sd image: no partition found");
                s0 = ReadAt(volStart, 512);
                Bin.Require(LooksLikeBpb(s0), "sd image: partition is not FAT");
            }
            bps = BitConverter.ToUInt16(s0, 11); spc = s0[13];
            int reserved = BitConverter.ToUInt16(s0, 14), nfats = s0[16];
            int rootEnt = BitConverter.ToUInt16(s0, 17);
            uint tot = BitConverter.ToUInt16(s0, 19); if (tot == 0) tot = BitConverter.ToUInt32(s0, 32);
            uint fatSz = BitConverter.ToUInt16(s0, 22); if (fatSz == 0) fatSz = BitConverter.ToUInt32(s0, 36);
            int rootSectors = (rootEnt * 32 + bps - 1) / bps;
            long dataSectors = tot - reserved - (long)nfats * fatSz - rootSectors;
            clusterCount = (uint)(dataSectors / spc);
            Bin.Require(clusterCount >= 4085, "FAT12 is not supported");
            isFat32 = clusterCount >= 65525;
            clusterBytes = (long)bps * spc;
            fatStart = volStart + (long)reserved * bps;
            rootDirStart = fatStart + (long)nfats * fatSz * bps;
            rootDirEntries = rootEnt;
            dataStart = rootDirStart + (long)rootSectors * bps;
            rootCluster = isFat32 ? BitConverter.ToUInt32(s0, 44) : 0;
            // load FAT 1
            var raw = ReadAt(fatStart, (int)(fatSz * bps));
            int n = (int)(isFat32 ? raw.Length / 4 : raw.Length / 2);
            fat = new uint[n];
            if (isFat32) for (int i = 0; i < n; i++) fat[i] = BitConverter.ToUInt32(raw, i * 4) & 0x0FFFFFFF;
            else for (int i = 0; i < n; i++) fat[i] = BitConverter.ToUInt16(raw, i * 2);
        }

        static bool LooksLikeBpb(byte[] s)
        {
            if (s[510] != 0x55 || s[511] != 0xAA) return false;
            int bps = BitConverter.ToUInt16(s, 11), spc = s[13];
            if (bps != 512 && bps != 1024 && bps != 2048 && bps != 4096) return false;
            if (spc == 0 || (spc & (spc - 1)) != 0) return false;
            return s[16] != 0 && BitConverter.ToUInt16(s, 14) != 0;
        }

        byte[] ReadAt(long off, int len)
        {
            var b = new byte[len]; fs.Position = off;
            int got = 0; while (got < len) { int r = fs.Read(b, got, len - got); if (r <= 0) throw new EndOfStreamException("image truncated"); got += r; }
            return b;
        }

        long ClusterOffset(uint c) { return dataStart + (long)(c - 2) * clusterBytes; }
        bool IsEoc(uint v) { return isFat32 ? v >= 0x0FFFFFF8 : v >= 0xFFF8; }

        List<uint> Chain(uint first)
        {
            var l = new List<uint>(); uint c = first;
            while (c >= 2 && !IsEoc(c))
            {
                Bin.Require(c < fat.Length && c < clusterCount + 2, "corrupt FAT chain");
                l.Add(c); Bin.Require(l.Count <= clusterCount, "FAT chain loop");
                c = fat[c];
            }
            return l;
        }

        byte[] ReadDirBytes(uint cluster, bool isRoot)
        {
            if (isRoot && !isFat32) return ReadAt(rootDirStart, rootDirEntries * 32);
            if (isRoot) cluster = rootCluster;
            using (var ms = new MemoryStream())
            {
                foreach (var c in Chain(cluster)) { var b = ReadAt(ClusterOffset(c), (int)clusterBytes); ms.Write(b, 0, b.Length); }
                return ms.ToArray();
            }
        }

        public List<FatEntry> List(string dirPath)
        {
            uint cl; bool root;
            if (string.IsNullOrEmpty(dirPath) || dirPath == "/") { cl = 0; root = true; }
            else { var e = Find(dirPath); Bin.Require(e != null && e.IsDir, "not a directory in sd image: " + dirPath); cl = e.FirstCluster; root = false; }
            return Parse(ReadDirBytes(cl, root), (dirPath ?? "").Trim('/'));
        }

        List<FatEntry> Parse(byte[] d, string parent)
        {
            var res = new List<FatEntry>();
            var lfn = new char[20 * 13]; int lfnCount = 0; byte lfnSum = 0;
            for (int o = 0; o + 32 <= d.Length; o += 32)
            {
                byte b0 = d[o];
                if (b0 == 0) break;
                if (b0 == 0xE5) { lfnCount = 0; continue; }
                byte attr = d[o + 11];
                if (attr == 0x0F)
                {
                    int seq = b0 & 0x1F;
                    if ((b0 & 0x40) != 0) { lfnCount = seq; lfnSum = d[o + 13]; Array.Clear(lfn, 0, lfn.Length); }
                    if (seq >= 1 && seq <= 20)
                    {
                        int[] offs = { 1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30 };
                        for (int k = 0; k < 13; k++) lfn[(seq - 1) * 13 + k] = (char)BitConverter.ToUInt16(d, o + offs[k]);
                    }
                    continue;
                }
                if ((attr & 0x08) != 0) { lfnCount = 0; continue; }   // volume label
                string name;
                byte sum = 0; for (int k = 0; k < 11; k++) sum = (byte)(((sum & 1) << 7) + (sum >> 1) + d[o + k]);
                if (lfnCount > 0 && sum == lfnSum)
                {
                    var sb = new StringBuilder();
                    for (int k = 0; k < lfnCount * 13; k++) { char ch = lfn[k]; if (ch == 0 || ch == 0xFFFF) break; sb.Append(ch); }
                    name = sb.ToString();
                }
                else
                {
                    string b = Encoding.ASCII.GetString(d, o, 8).TrimEnd(' '), e = Encoding.ASCII.GetString(d, o + 8, 3).TrimEnd(' ');
                    if ((d[o + 12] & 0x08) != 0) b = b.ToLowerInvariant();
                    if ((d[o + 12] & 0x10) != 0) e = e.ToLowerInvariant();
                    if (d[o] == 0x05) b = (char)0xE5 + b.Substring(1);
                    name = e.Length > 0 ? b + "." + e : b;
                }
                lfnCount = 0;
                if (name == "." || name == "..") continue;
                uint fc = (uint)(BitConverter.ToUInt16(d, o + 20) << 16 | BitConverter.ToUInt16(d, o + 26));
                if (!isFat32) fc &= 0xFFFF;
                res.Add(new FatEntry { Name = name, IsDir = (attr & 0x10) != 0, Size = BitConverter.ToUInt32(d, o + 28), FirstCluster = fc, Path = parent.Length == 0 ? name : parent + "/" + name });
            }
            return res;
        }

        /// <summary>Case-insensitive path lookup ("Minusery/module/x.rel").</summary>
        public FatEntry Find(string path)
        {
            var parts = path.Trim('/').Split('/');
            uint cl = 0; bool root = true; FatEntry cur = null; string acc = "";
            for (int pi = 0; pi < parts.Length; pi++)
            {
                var p = parts[pi];
                FatEntry hit = null;
                foreach (var e in Parse(ReadDirBytes(cl, root), acc)) if (string.Equals(e.Name, p, StringComparison.OrdinalIgnoreCase)) { hit = e; break; }
                if (hit == null) return null;
                cur = hit; acc = hit.Path; cl = hit.FirstCluster; root = false;
                if (!hit.IsDir && pi != parts.Length - 1) return null;
            }
            return cur;
        }

        /// <summary>All files below dirPath (recursive), paths relative to the image root.</summary>
        public IEnumerable<FatEntry> Walk(string dirPath)
        {
            var start = Find(dirPath);
            Bin.Require(start != null && start.IsDir, "directory not found in sd image: " + dirPath);
            var stack = new Stack<FatEntry>(); stack.Push(start);
            while (stack.Count > 0)
            {
                var dir = stack.Pop();
                foreach (var e in Parse(ReadDirBytes(dir.FirstCluster, false), dir.Path))
                {
                    if (e.IsDir) stack.Push(e); else yield return e;
                }
            }
        }

        public void CopyTo(FatEntry e, Stream dst)
        {
            long remaining = e.Size;
            if (remaining == 0) return;
            var chain = Chain(e.FirstCluster);
            Bin.Require((long)chain.Count * clusterBytes >= remaining, "FAT chain shorter than file size: " + e.Path);
            int i = 0; var buf = new byte[1 << 20];
            while (i < chain.Count && remaining > 0)
            {
                int run = 1; while (i + run < chain.Count && chain[i + run] == chain[i + run - 1] + 1 && (long)(run + 1) * clusterBytes <= buf.Length) run++;
                long want = Math.Min(remaining, (long)run * clusterBytes);
                fs.Position = ClusterOffset(chain[i]);
                int got = 0; while (got < want) { int r = fs.Read(buf, got, (int)want - got); if (r <= 0) throw new EndOfStreamException(); got += r; }
                dst.Write(buf, 0, (int)want); remaining -= want; i += run;
            }
        }

        public byte[] ReadAll(FatEntry e)
        {
            using (var ms = new MemoryStream((int)e.Size)) { CopyTo(e, ms); return ms.ToArray(); }
        }

        public byte[] ReadAll(string path)
        {
            var e = Find(path); Bin.Require(e != null && !e.IsDir, "file not found in sd image: " + path);
            return ReadAll(e);
        }

        /// <summary>copy every file of the image (skipping 'System Volume Information') into <paramref name="dir"/></summary>
        public void ExtractAll(string dir)
        {
            foreach (var e in Walk(""))
            {
                if (e.Path.StartsWith("System Volume Information", StringComparison.OrdinalIgnoreCase)) continue;
                var dst = System.IO.Path.Combine(dir, e.Path.Replace('/', System.IO.Path.DirectorySeparatorChar));
                Directory.CreateDirectory(System.IO.Path.GetDirectoryName(dst));
                using (var f = File.Create(dst)) CopyTo(e, f);
            }
        }

        public void Dispose() { fs.Dispose(); }
    }
}
