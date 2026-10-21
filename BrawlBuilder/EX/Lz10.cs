using System;

namespace BrawlBuilder.EX
{
    /// <summary>Nintendo LZ77 type 0x10. Compress is a line-by-line port of scripts/lz10c.c (greedy + 1-step lazy, hash chains),
    /// so its output is byte-identical to the reference compressor.</summary>
    public static class Lz10
    {
        const int WIN = 4096, MAXL = 18, HB = 16;

        /// <summary>LZ77 type 0x11 (extended lengths, used by e.g. PMEX's common2.pac)</summary>
        static byte[] Decompress11(byte[] d)
        {
            int size = d[1] | d[2] << 8 | d[3] << 16, i = 4;
            if (size == 0) { size = d[4] | d[5] << 8 | d[6] << 16 | d[7] << 24; i = 8; }
            var o = new byte[size]; int op = 0;
            while (op < size && i < d.Length)
            {
                int flags = d[i++];
                for (int bit = 0; bit < 8 && op < size; bit++)
                {
                    if ((flags & (0x80 >> bit)) == 0) { o[op++] = d[i++]; continue; }
                    int b0 = d[i], ind = b0 >> 4, ln, disp;
                    if (ind == 0) { int b1 = d[i + 1], b2 = d[i + 2]; i += 3; ln = (((b0 & 0xF) << 4) | (b1 >> 4)) + 0x11; disp = (((b1 & 0xF) << 8) | b2) + 1; }
                    else if (ind == 1) { int b1 = d[i + 1], b2 = d[i + 2], b3 = d[i + 3]; i += 4; ln = (((b0 & 0xF) << 12) | (b1 << 4) | (b2 >> 4)) + 0x111; disp = (((b2 & 0xF) << 8) | b3) + 1; }
                    else { int b1 = d[i + 1]; i += 2; ln = ind + 1; disp = (((b0 & 0xF) << 8) | b1) + 1; }
                    Bin.Require(disp <= op, "corrupt LZ11 stream");
                    for (int k = 0; k < ln && op < size; k++) { o[op] = o[op - disp]; op++; }
                }
            }
            Bin.Require(op == size, "truncated LZ11 stream");
            return o;
        }

        public static bool IsCompressed(byte[] d) => d.Length >= 4 && (d[0] == 0x10 || d[0] == 0x11);

        public static byte[] Decompress(byte[] d)
        {
            if (d.Length >= 4 && d[0] == 0x11) return Decompress11(d);
            Bin.Require(d.Length >= 4 && d[0] == 0x10, "not LZ77 type 0x10/0x11");
            int size = d[1] | d[2] << 8 | d[3] << 16;
            var o = new byte[size];
            int op = 0, i = 4;
            while (op < size && i < d.Length)
            {
                int flags = d[i++];
                for (int bit = 0; bit < 8; bit++)
                {
                    if (op >= size) break;
                    if ((flags & (0x80 >> bit)) != 0)
                    {
                        int b0 = d[i], b1 = d[i + 1]; i += 2;
                        int ln = (b0 >> 4) + 3, disp = (((b0 & 0xF) << 8) | b1) + 1;
                        Bin.Require(disp <= op, "corrupt LZ10 stream");
                        for (int k = 0; k < ln && op < size; k++) { o[op] = o[op - disp]; op++; }
                    }
                    else o[op++] = d[i++];
                }
            }
            Bin.Require(op == size, "truncated LZ10 stream");
            return o;
        }

        static int Find(byte[] s, int n, int p, int[] head, int[] prev, int maxLen, out int disp)
        {
            disp = 0;
            if (p + 3 > n) return 0;
            int h = ((s[p] << 8) ^ (s[p + 1] << 4) ^ s[p + 2]) & ((1 << HB) - 1);
            int best = 0, c = head[h], depth = 0, maxl = Math.Min(n - p, maxLen);
            while (c >= 0 && p - c <= WIN && depth++ < 512)
            {
                int l = 0; while (l < maxl && s[c + l] == s[p + l]) l++;
                if (l > best) { best = l; disp = p - c; if (l == maxl) break; }
                c = prev[c];
            }
            return best >= 3 ? best : 0;
        }

        /// <summary>type 0x10, or 0x11 (<paramref name="type11"/>: matches up to 0x10110 bytes) to keep a file in the format it came in</summary>
        public static byte[] Compress(byte[] s, bool type11 = false)
        {
            int n = s.Length, maxLen = type11 ? 0x10110 : MAXL;
            var o = new byte[n + n / 8 + 64]; int op = 0;
            o[op++] = (byte)(type11 ? 0x11 : 0x10); o[op++] = (byte)n; o[op++] = (byte)(n >> 8); o[op++] = (byte)(n >> 16);
            var head = new int[1 << HB]; for (int i = 0; i < head.Length; i++) head[i] = -1;
            var prev = new int[Math.Max(n, 1)];
            Action<int> ins = q =>
            {
                if (q + 3 <= n) { int h = ((s[q] << 8) ^ (s[q + 1] << 4) ^ s[q + 2]) & ((1 << HB) - 1); prev[q] = head[h]; head[h] = q; }
            };
            int p = 0;
            while (p < n)
            {
                int fp = op++; int flags = 0;
                for (int b = 0; b < 8 && p < n; b++)
                {
                    int d, d2;
                    int l = Find(s, n, p, head, prev, maxLen, out d);
                    if (l != 0) { ins(p); int l2 = Find(s, n, p + 1, head, prev, maxLen, out d2); if (l2 > l + 1) l = 0; }
                    else ins(p);
                    if (l != 0)
                    {
                        flags |= 0x80 >> b;
                        int dd = d - 1;
                        if (!type11) { o[op++] = (byte)(((l - 3) << 4) | (dd >> 8)); o[op++] = (byte)(dd & 255); }
                        else if (l <= 0x10) { o[op++] = (byte)(((l - 1) << 4) | (dd >> 8)); o[op++] = (byte)(dd & 255); }
                        else if (l <= 0x110) { int m = l - 0x11; o[op++] = (byte)(m >> 4); o[op++] = (byte)(((m & 0xF) << 4) | (dd >> 8)); o[op++] = (byte)(dd & 255); }
                        else { int m = l - 0x111; o[op++] = (byte)(0x10 | (m >> 12)); o[op++] = (byte)((m >> 4) & 255); o[op++] = (byte)(((m & 0xF) << 4) | (dd >> 8)); o[op++] = (byte)(dd & 255); }
                        for (int k = 1; k < l; k++) ins(p + k);
                        p += l;
                    }
                    else o[op++] = s[p++];
                }
                o[fp] = (byte)flags;
            }
            while ((op & 3) != 0) o[op++] = 0;
            var r = new byte[op]; Array.Copy(o, r, op); return r;
        }
    }
}
