using System;
using System.Collections.Generic;

namespace BrawlBuilder.EX
{
    /// <summary>Port of scripts/thp_truncate.py: keep the first K frames of a THP movie as a valid, self-consistent THP (SSE cutscene placeholders).</summary>
    public static class Thp
    {
        class Hdr { public uint N, FirstSz, DataSz, OffsOff, FirstOff, LastOff; }

        static Hdr Parse(byte[] d)
        {
            Bin.Require(d.Length >= 0x30 && d[0] == (byte)'T' && d[1] == (byte)'H' && d[2] == (byte)'P' && d[3] == 0, "not a THP file");
            return new Hdr { N = Bin.Get32(d, 0x14), FirstSz = Bin.Get32(d, 0x18), DataSz = Bin.Get32(d, 0x1C), OffsOff = Bin.Get32(d, 0x24), FirstOff = Bin.Get32(d, 0x28), LastOff = Bin.Get32(d, 0x2C) };
        }

        static List<KeyValuePair<long, long>> Frames(byte[] d, Hdr h)
        {
            var r = new List<KeyValuePair<long, long>>(); long off = h.FirstOff, sz = h.FirstSz;
            for (uint i = 0; i < h.N; i++)
            {
                r.Add(new KeyValuePair<long, long>(off, sz));
                long nxt = Bin.Get32(d, (int)off); off += sz; sz = nxt;
            }
            return r;
        }

        static bool Check(byte[] d, out Hdr h, out List<KeyValuePair<long, long>> fr)
        {
            h = Parse(d); fr = Frames(d, h);
            return fr[fr.Count - 1].Key == h.LastOff && h.DataSz == d.Length - h.FirstOff && h.OffsOff == 0;
        }

        public static byte[] Truncate(byte[] d, int k)
        {
            Hdr h; List<KeyValuePair<long, long>> fr;
            Bin.Require(Check(d, out h, out fr), "input THP failed consistency check");
            k = Math.Min(k, (int)h.N);
            long start = h.FirstOff, end = fr[k - 1].Key + fr[k - 1].Value;
            var outb = new byte[end]; Array.Copy(d, outb, end);
            long lastRel = fr[k - 1].Key - start;
            Bin.Put32(outb, (int)(start + lastRel), h.FirstSz);      // last.next = firstFrameSize
            Bin.Put32(outb, (int)start + 4, (uint)fr[k - 1].Value);   // first.prev = size of last frame
            Bin.Put32(outb, 0x14, (uint)k);
            Bin.Put32(outb, 0x1C, (uint)(end - start));
            Bin.Put32(outb, 0x2C, (uint)(start + lastRel));
            Hdr h2; List<KeyValuePair<long, long>> fr2;
            Bin.Require(Check(outb, out h2, out fr2) && h2.N == k, "output THP failed consistency check");
            return outb;
        }
    }
}
