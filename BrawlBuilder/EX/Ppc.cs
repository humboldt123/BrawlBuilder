using System;
using System.Collections.Generic;

namespace BrawlBuilder.EX
{
    /// <summary>Minimal PowerPC (Broadway) encoder + two-pass assembler for the helper routines EX injects into main.dol.</summary>
    public static class Ppc
    {
        static uint D(uint op, int rt, int ra, int imm) => (op << 26) | ((uint)rt << 21) | ((uint)ra << 16) | ((uint)imm & 0xFFFF);
        static uint X(int rt, int ra, int rb, uint xo) => (31u << 26) | ((uint)rt << 21) | ((uint)ra << 16) | ((uint)rb << 11) | (xo << 1);
        public static uint Addi(int rt, int ra, int si) => D(14, rt, ra, si);
        public static uint Li(int rt, int si) => Addi(rt, 0, si);
        public static uint Addis(int rt, int ra, int si) => D(15, rt, ra, si);
        public static uint Lis(int rt, int si) => Addis(rt, 0, si);
        public static uint Ori(int ra, int rs, int ui) => D(24, rs, ra, ui);
        public static uint Lwz(int rt, int d, int ra) => D(32, rt, ra, d);
        public static uint Lbz(int rt, int d, int ra) => D(34, rt, ra, d);
        public static uint Stw(int rs, int d, int ra) => D(36, rs, ra, d);
        public static uint Stb(int rs, int d, int ra) => D(38, rs, ra, d);
        public static uint Stwu(int rs, int d, int ra) => D(37, rs, ra, d);
        public static uint Lmw(int rt, int d, int ra) => D(46, rt, ra, d);
        public static uint Stmw(int rs, int d, int ra) => D(47, rs, ra, d);
        public static uint Cmpwi(int ra, int si) => D(11, 0, ra, si);
        public static uint Cmplwi(int ra, int ui) => D(10, 0, ra, ui);
        public static uint Cmpw(int ra, int rb) => X(0, ra, rb, 0);
        public static uint Cmplw(int ra, int rb) => X(0, ra, rb, 32);
        public static uint Add(int rt, int ra, int rb) => X(rt, ra, rb, 266);
        public static uint Subf(int rt, int ra, int rb) => X(rt, ra, rb, 40);     // rt = rb - ra
        public static uint Mr(int ra, int rs) => X(rs, ra, rs, 444);
        public static uint Mullw(int rt, int ra, int rb) => X(rt, ra, rb, 235);
        public static uint Divwu(int rt, int ra, int rb) => X(rt, ra, rb, 459);
        public static uint Xor(int ra, int rs, int rb) => X(rs, ra, rb, 316);
        public static uint Lwzx(int rt, int ra, int rb) => X(rt, ra, rb, 23);
        public static uint Rlwinm(int ra, int rs, int sh, int mb, int me) => (21u << 26) | ((uint)rs << 21) | ((uint)ra << 16) | ((uint)sh << 11) | ((uint)mb << 6) | ((uint)me << 1);
        public static uint Mflr(int rt) => (31u << 26) | ((uint)rt << 21) | (8u << 16) | (339u << 1);
        public static uint Mtlr(int rs) => (31u << 26) | ((uint)rs << 21) | (8u << 16) | (467u << 1);
        public static uint Mtctr(int rs) => (31u << 26) | ((uint)rs << 21) | (9u << 16) | (467u << 1);
        public const uint Blr = 0x4E800020, Bctrl = 0x4E800421, Nop = 0x60000000;
        public static uint B(uint from, uint to, bool link = false) => 0x48000000u | ((to - from) & 0x03FFFFFC) | (link ? 1u : 0u);
        public static uint Bdnz(uint from, uint to) => (16u << 26) | (16u << 21) | ((to - from) & 0xFFFC);
        static readonly Dictionary<string, (int, int)> Cc = new Dictionary<string, (int, int)>
        { { "lt", (12, 0) }, { "gt", (12, 1) }, { "eq", (12, 2) }, { "ge", (4, 0) }, { "le", (4, 1) }, { "ne", (4, 2) } };
        public static uint Bc(string cond, uint from, uint to) { var c = Cc[cond]; return (16u << 26) | ((uint)c.Item1 << 21) | ((uint)c.Item2 << 16) | ((to - from) & 0xFFFC); }
    }

    /// <summary>Two-pass assembler: items are fixed words or functions of (pc, labels).</summary>
    public class Asm
    {
        public readonly uint Base;
        readonly List<Func<uint, Dictionary<string, uint>, uint>> items = new List<Func<uint, Dictionary<string, uint>, uint>>();
        public readonly Dictionary<string, uint> Labels = new Dictionary<string, uint>();
        public Asm(uint b) { Base = b; }
        public uint Pc => Base + 4u * (uint)items.Count;
        public void Label(string n) { Labels[n] = Pc; }
        public void W(uint w) { items.Add((pc, l) => w); }
        public void W(Func<uint, Dictionary<string, uint>, uint> f) { items.Add(f); }
        public void La(int reg, uint addr) { W(Ppc.Lis(reg, (int)(((addr + 0x8000) >> 16) & 0xFFFF))); W(Ppc.Addi(reg, reg, (int)(addr & 0xFFFF))); }
        public void Bl(uint target) { W((pc, l) => Ppc.B(pc, target, true)); }
        public void Bl(string label) { W((pc, l) => Ppc.B(pc, l[label], true)); }
        public void Br(string label) { W((pc, l) => Ppc.B(pc, l[label])); }
        public void Bc(string cond, string label) { W((pc, l) => Ppc.Bc(cond, pc, l[label])); }
        public void Bdnz(string label) { W((pc, l) => Ppc.Bdnz(pc, l[label])); }
        public byte[] Bytes()
        {
            var o = new byte[items.Count * 4];
            for (int i = 0; i < items.Count; i++) Bin.Put32(o, 4 * i, items[i](Base + 4u * (uint)i, Labels));
            return o;
        }
    }
}
