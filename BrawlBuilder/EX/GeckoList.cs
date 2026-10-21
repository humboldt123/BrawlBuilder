using System;
using System.Collections.Generic;
using System.Linq;

namespace BrawlBuilder.EX
{
    public struct Line { public uint A, B; public Line(uint a, uint b) { A = a; B = b; } }

    public class GeckoCode
    {
        public string Name;
        public List<Line> Lines = new List<Line>();
    }

    /// <summary>Gecko code list helpers: parsing, splitting into codes, the static pre-pass (what a launcher applied before the game ran),
    /// and the BrawlX-heap / vanilla-FST pointer fix-ups.</summary>
    public static class GeckoList
    {
        public static List<Line> Parse(byte[] gct)
        {
            Bin.Require(gct.Length >= 16 && Bin.Get32(gct, 0) == 0x00D0C0DE && Bin.Get32(gct, 4) == 0x00D0C0DE, "not a GCT (missing 00D0C0DE header)");
            var l = new List<Line>();
            for (int i = 8; i + 8 <= gct.Length; i += 8)
            {
                uint a = Bin.Get32(gct, i), b = Bin.Get32(gct, i + 4);
                if (a == 0xF0000000 && b == 0) break;
                l.Add(new Line(a, b));
            }
            return l;
        }

        public static byte[] Build(IEnumerable<GeckoCode> codes)
        {
            var lines = codes.SelectMany(c => c.Lines).ToList();
            var o = new byte[16 + 8 * lines.Count];
            Bin.Put32(o, 0, 0x00D0C0DE); Bin.Put32(o, 4, 0x00D0C0DE);
            for (int i = 0; i < lines.Count; i++) { Bin.Put32(o, 8 + 8 * i, lines[i].A); Bin.Put32(o, 12 + 8 * i, lines[i].B); }
            Bin.Put32(o, o.Length - 8, 0xF0000000);
            return o;
        }

        /// <summary>number of lines the op starting at <paramref name="i"/> occupies</summary>
        public static int OpLength(List<Line> l, int i)
        {
            uint t = (l[i].A >> 24) & 0xFE, b = l[i].B;
            if (t == 0x06 || t == 0x16) return 1 + (int)((b + 7) / 8);
            if (t == 0x08) return 2;
            if ((t == 0xC0 || t == 0xC2 || t == 0xD2 || t == 0xD0 || t == 0xD4 || t == 0xD6 || t == 0xF2 || t == 0xF4) && b < 3000) return 1 + (int)b;
            uint a = l[i].A;
            if (t == 0x66 && ((a >> 20) & 0xF) == 2 && (a & 0xFFFF) > 0 && (a & 0xFFFF) < 0x8000) return 1 + (int)(a & 0xFFFF);   // unconditional goto over data
            return 1;
        }

        /// <summary>split a code list into pseudo-codes after each full terminator (E0): no names needed, but boundaries for the pre-pass</summary>
        /// <summary>byte ranges the list may write in [lo, hi) with absolute (ba = 0x80000000) addresses - conservative, conditions ignored</summary>
        public static List<(uint Lo, uint Hi)> WritesIn(List<Line> l, uint lo, uint hi)
        {
            var o = new List<(uint, uint)>();
            for (int i = 0; i < l.Count; i += Math.Max(1, OpLength(l, i)))
            {
                uint t = (l[i].A >> 24) & 0xFE, a = 0x80000000 | (l[i].A & 0x1FFFFFF), n;
                if (t == 0x00 || t == 0x02) n = ((l[i].B >> 16) + 1) * 4;
                else if (t == 0x04 || t == 0xC6 || t == 0xC2) n = 4;
                else if (t == 0x06) n = l[i].B;
                else continue;
                if (a < hi && a + n > lo) o.Add((a, a + n));
            }
            // ASM in the payloads addresses low memory itself: "lis rX,0x8000" + a load/store/addi/ori with a displacement in range
            // (Legacy TE's Tags Controls keeps a flag at 0x80002800), or a plain pointer word - conservative, the whole list as words
            var w = l.SelectMany(x => new[] { x.A, x.B }).ToList();
            for (int i = 0; i < w.Count; i++)
            {
                if (w[i] >= lo && w[i] < hi) o.Add((w[i] & ~3u, (w[i] & ~3u) + 4));
                if ((w[i] & 0xFC1FFFFF) != 0x3C008000) continue;            // lis rX, 0x8000
                uint rx = (w[i] >> 21) & 31;
                for (int k = i + 1; k < Math.Min(w.Count, i + 24); k++)
                {
                    uint op = w[k] >> 26, rs = (w[k] >> 21) & 31, ra = (w[k] >> 16) & 31, d = w[k] & 0xFFFF;
                    bool uses = op >= 32 && op <= 47 ? ra == rx : op == 14 ? ra == rx : op == 24 ? rs == rx : false;
                    if (!uses || d >= 0x8000) continue;
                    uint a = 0x80000000 | d;
                    if (a >= lo && a < hi) o.Add((a, a + (op == 46 || op == 47 ? 0x80u : 0x20u)));
                }
            }
            return o;
        }

        public static List<GeckoCode> SplitAtTerminators(List<Line> l)
        {
            var o = new List<GeckoCode>(); var cur = new GeckoCode { Name = "code 0" };
            for (int i = 0; i < l.Count;)
            {
                int n = OpLength(l, i);
                for (int k = 0; k < n && i + k < l.Count; k++) cur.Lines.Add(l[i + k]);
                bool e0 = (l[i].A >> 24) == 0xE0; i += n;
                if (e0) { o.Add(cur); cur = new GeckoCode { Name = "code " + o.Count }; }
            }
            if (cur.Lines.Count > 0) o.Add(cur);
            return o;
        }

        /// <summary>try to name codes from a GCTRealMate codeset.txt ("name" line, then "* XXXXXXXX XXXXXXXX" lines); null if it does not match</summary>
        public static List<GeckoCode> FromCodeset(string text, List<Line> gct)
        {
            var codes = new List<GeckoCode>(); GeckoCode cur = null;
            foreach (var raw in text.Replace("\r", "").Split('\n'))
            {
                var s = raw.Trim(); if (s.Length == 0) continue;
                var p = s.TrimStart('*').Trim().Split(new[] { ' ', '\t' }, StringSplitOptions.RemoveEmptyEntries);
                if (cur != null && p.Length == 2 && p[0].Length == 8 && p[1].Length == 8 && IsHex(p[0]) && IsHex(p[1]))
                { cur.Lines.Add(new Line(Convert.ToUInt32(p[0], 16), Convert.ToUInt32(p[1], 16))); continue; }
                if (cur == null && s == "RSBE01") continue;
                if (s.StartsWith("*")) continue;
                cur = new GeckoCode { Name = s }; codes.Add(cur);
            }
            codes = codes.Where(c => c.Lines.Count > 0).ToList();
            var flat = codes.SelectMany(c => c.Lines).ToList();
            if (flat.Count != gct.Count) return null;
            for (int i = 0; i < flat.Count; i++) if (flat[i].A != gct[i].A || flat[i].B != gct[i].B) return null;
            return codes;
        }
        static bool IsHex(string s) => s.All(c => Uri.IsHexDigit(c));

        /// <summary>ASM blocks (C2/D2 hooks with their address, C0 without) as (header index, address or 0, payload lines)</summary>
        public static IEnumerable<(int Header, uint Addr, int N)> AsmBlocks(List<Line> l)
        {
            for (int i = 0; i < l.Count;)
            {
                uint t = (l[i].A >> 24) & 0xFE, b = l[i].B;
                if ((t == 0xC2 || t == 0xD2) && b > 0 && b < 3000) { yield return (i, 0x80000000 | (l[i].A & 0x1FFFFFF), (int)b); i += 1 + (int)b; }
                else if (t == 0xC0 && b > 0 && b < 3000) { yield return (i, 0, (int)b); i += 1 + (int)b; }
                else i += OpLength(l, i);
            }
        }

        /// <summary>
        /// The launcher's pre-pass: plain RAM writes (00/02/04/06) and C2 hooks that are not under a condition, a pointer/register op or a
        /// flow op of their code. A full terminator (E0) starts a new code (resets the state).
        /// </summary>
        public static void StaticOps(List<Line> l, List<(uint Addr, byte[] Data)> writes, List<(int Header, uint Addr, int N)> hooks)
        {
            int depth = 0; bool tainted = false;
            for (int i = 0; i < l.Count; i += OpLength(l, i))
            {
                uint a = l[i].A, b = l[i].B, t = (a >> 24) & 0xFE, full = 0x80000000 | (a & 0x1FFFFFF);
                if (t >= 0x20 && t <= 0x3E) { depth++; continue; }
                if (t == 0xE0) { depth = 0; tainted = false; continue; }
                if (t == 0xE2) { if (((a >> 20) & 0xF) != 0) tainted = true; depth = Math.Max(0, depth - (int)Math.Max(1, a & 0xFF)); continue; }   // E2T000VV: close VV ifs (T=1: else)
                if (t == 0xF0) break;
                bool plain = t == 0x00 || t == 0x02 || t == 0x04 || t == 0x06 || t == 0xC0 || t == 0xC2 || t == 0xD2 || t == 0xD0 || t == 0xD4 || t == 0xD6;
                bool neutral = (t >= 0x10 && t <= 0x16) || t == 0x08 || (t >= 0x80 && t <= 0xAE)       // po writes, serial, Gecko registers,
                               || t == 0x44 || t == 0x48 || t == 0x4A || t == 0x4C || t == 0x4E || t == 0x60 || t == 0x62; // po/store ops, repeat: ba-relative writes unaffected
                if (neutral) continue;
                if (!plain) { tainted = true; continue; }                  // base-address, flow, switch, checksum and unknown ops
                if (depth > 0 || tainted) continue;
                switch (t)
                {
                    case 0x00: for (uint k = 0; k <= (b >> 16); k++) writes.Add((full + k, new[] { (byte)b })); break;
                    case 0x02: for (uint k = 0; k <= (b >> 16); k++) writes.Add((full + 2 * k, new[] { (byte)(b >> 8), (byte)b })); break;
                    case 0x04: { var d = new byte[4]; Bin.Put32(d, 0, b); writes.Add((full, d)); break; }
                    case 0x06:
                        {
                            var d = new byte[b];
                            for (int k = 0; k < b; k++) { var ln = l[i + 1 + k / 8]; uint w = (k % 8) < 4 ? ln.A : ln.B; d[k] = (byte)(w >> (24 - 8 * (k % 4))); }
                            writes.Add((full, d)); break;
                        }
                    case 0xC2: if (hooks != null && b > 0 && b < 3000) hooks.Add((i, full, (int)b)); break;
                }
            }
        }

        // ---------------------------------------------------------------------------------------------------------------- heap / FST
        public const uint HeapOld = 0x817BA5A0, HeapSize = 0x20000, VanArenaHi = 0x817DA5A0, MemTop = 0x81800000;
        static readonly int[] DForm = { 14, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53, 54, 55 };
        static readonly int[] StoreLike = { 36, 37, 38, 39, 44, 45, 47, 52, 53, 54, 55 };

        static (int Index, bool Ori, uint Lo)? FindPair(IList<uint> words, int reg)
        {
            for (int r = 0; r < words.Count; r++)
            {
                uint u = words[r]; int op = (int)(u >> 26), ra = (int)((u >> 16) & 0x1F), rs = (int)((u >> 21) & 0x1F);
                if (op == 24 && rs == reg) return (r, true, u & 0xFFFF);
                if (DForm.Contains(op) && ra == reg) return (r, false, u & 0xFFFF);
                if (rs == reg && !StoreLike.Contains(op)) return null;
            }
            return null;
        }

        /// <summary>
        /// Move every hard-coded address into the vanilla BrawlX heap by <paramref name="delta"/> and point every hard-coded address into the
        /// vanilla FST name table at our own copy of the string. Returns notes (empty: nothing found).
        /// </summary>
        public static List<string> FixHeapAndFst(GeckoCode c, int delta, Func<uint, byte[]> fstName, Func<byte[], uint> stringAddr, Func<uint, uint> dolWord)
        {
            var notes = new List<string>(); var L = c.Lines;
            // flatten ASM payload words with their position
            var W = new List<(int Li, int Slot, uint Base, int Idx, int Hdr)>();
            foreach (var blk in AsmBlocks(L))
                for (int j = 0; j < blk.N; j++) for (int s = 0; s < 2; s++) W.Add((blk.Header + 1 + j, s, blk.Addr, 2 * j + s, blk.Header));
            Func<int, uint> word = q => W[q].Slot == 0 ? L[W[q].Li].A : L[W[q].Li].B;
            Action<int, uint> set = (q, v) => { var ln = L[W[q].Li]; if (W[q].Slot == 0) ln.A = v; else ln.B = v; L[W[q].Li] = ln; };
            for (int q = 0; q < W.Count; q++)
            {
                uint w = word(q);
                if (w >> 26 != 15 || ((w >> 16) & 0x1F) != 0) continue;                     // lis rX, hi
                uint hi = w & 0xFFFF; if (hi < 0x817B || hi > 0x817F) continue;
                int reg = (int)((w >> 21) & 0x1F);
                var rest = new List<int>();
                for (int k = q + 1; k < W.Count && rest.Count < 7 && W[k].Hdr == W[q].Hdr; k++) rest.Add(k);
                var f = FindPair(rest.Select(word).ToList(), reg); bool inDol = false;
                if (f == null && W[q].Base != 0 && rest.All(k => word(k) == 0 || word(k) == Ppc.Nop))
                { f = FindPair(Enumerable.Range(1, 7).Select(k => dolWord(W[q].Base + 4u * (uint)k)).ToList(), reg); inDol = true; }
                if (f == null) { notes.Add("lis " + hi.ToString("x") + " without a pair (left)"); continue; }
                uint lo = f.Value.Lo;
                uint addr = (hi << 16) + (f.Value.Ori ? lo : (uint)(short)lo);
                uint neu;
                if (addr >= HeapOld && addr < HeapOld + HeapSize) neu = (uint)(addr + delta);
                else if (addr >= VanArenaHi && addr < MemTop)
                {
                    var s = fstName(addr); if (s == null) { notes.Add("FST pointer " + addr.ToString("x") + " not on a name (left)"); continue; }
                    neu = stringAddr(s);
                }
                else continue;
                uint nh = f.Value.Ori ? neu >> 16 : ((neu + 0x8000) >> 16) & 0xFFFF, nl = neu & 0xFFFF;
                if (neu == addr) continue;
                if (inDol)
                {
                    Bin.Require(nl == lo, "a lis completed in main.dol needs a different low half (" + addr.ToString("X8") + ")");
                    set(q, (w & 0xFFFF0000) | nh); notes.Add(addr.ToString("x") + " -> " + neu.ToString("x") + " (pair in main.dol)"); continue;
                }
                int r = rest[f.Value.Index];
                set(q, (w & 0xFFFF0000) | nh); set(r, (word(r) & 0xFFFF0000) | nl);
                notes.Add(addr.ToString("x") + " -> " + neu.ToString("x"));
            }
            if (delta == 0) return notes;
            // 32-byte-aligned data words inside ASM payloads that hold a heap address (e.g. Syringe's 'word 0x817ba5a0')
            for (int q = 0; q < W.Count; q++)
            {
                uint w = word(q);
                if (w >= HeapOld && w <= VanArenaHi && (w & 0x1F) == 0) { set(q, (uint)(w + delta)); notes.Add("data word " + w.ToString("x") + " moved"); }
            }
            // plain writes into the heap, and the prefab heap header blob (pointers inside it move too)
            for (int i = 0; i < L.Count; i += OpLength(L, i))
            {
                uint a = L[i].A, b = L[i].B, t = (a >> 24) & 0xFE, ad = 0x80000000 | (a & 0x1FFFFFF);
                if (ad < HeapOld || ad >= HeapOld + HeapSize || (a & 0x10000000) != 0) continue;
                if (t == 0 || t == 2 || t == 4) { L[i] = new Line((a & 0xFE000000) | ((uint)(ad + delta) & 0x01FFFFFF), b); notes.Add("write " + ad.ToString("x") + " moved"); }
                else if (t == 6)
                {
                    for (int k = 0; k + 4 <= b; k += 4)
                    {
                        int li = i + 1 + k / 8; bool hiw = (k % 8) == 0; uint v = hiw ? L[li].A : L[li].B;
                        if (v >= HeapOld && v <= HeapOld + HeapSize) L[li] = hiw ? new Line((uint)(v + delta), L[li].B) : new Line(L[li].A, (uint)(v + delta));
                    }
                    L[i] = new Line((a & 0xFE000000) | ((uint)(ad + delta) & 0x01FFFFFF), b); notes.Add("heap blob " + ad.ToString("x") + " moved");
                }
            }
            return notes;
        }
    }
}
