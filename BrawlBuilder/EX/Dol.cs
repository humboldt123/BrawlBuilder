using System;
using System.Collections.Generic;
using System.Linq;

namespace BrawlBuilder.EX
{
    /// <summary>Wii DOL image: 7 text + 11 data sections (offsets @0x00, addresses @0x48, sizes @0x90).</summary>
    public class Dol
    {
        List<byte> d; public uint[] Off = new uint[18], Addr = new uint[18], Size = new uint[18];
        public Dol(byte[] b)
        {
            d = new List<byte>(b);
            for (int i = 0; i < 18; i++) { Off[i] = Bin.Get32(b, 4 * i); Addr[i] = Bin.Get32(b, 0x48 + 4 * i); Size[i] = Bin.Get32(b, 0x90 + 4 * i); }
        }
        public int Section(uint va) { for (int i = 0; i < 18; i++) if (Size[i] != 0 && Addr[i] <= va && va < Addr[i] + Size[i]) return i; return -1; }
        int FileOff(uint va) { int i = Section(va); Bin.Require(i >= 0, "address " + va.ToString("X8") + " is not inside main.dol"); return (int)(Off[i] + va - Addr[i]); }
        public uint Read(uint va) { int o = FileOff(va); return (uint)(d[o] << 24 | d[o + 1] << 16 | d[o + 2] << 8 | d[o + 3]); }
        public void Write(uint va, uint w, uint? old = null)
        {
            if (old.HasValue) Bin.Require(Read(va) == old.Value, "main.dol @" + va.ToString("X8") + ": expected " + old.Value.ToString("X8") + " found " + Read(va).ToString("X8") + " (not Brawl USA Rev 2?)");
            int o = FileOff(va); d[o] = (byte)(w >> 24); d[o + 1] = (byte)(w >> 16); d[o + 2] = (byte)(w >> 8); d[o + 3] = (byte)w;
        }
        public void WriteBytes(uint va, byte[] b) { for (int k = 0; k < b.Length; k++) d[FileOff(va + (uint)k)] = b[k]; }
        public bool Covers(uint va, int n) { for (int k = 0; k < n; k++) if (Section(va + (uint)k) < 0) return false; return true; }
        public bool Free(uint va, int n) { for (int k = 0; k < n; k++) if (Section(va + (uint)k) >= 0) return false; return true; }
        /// <summary>append a new section (data may also use a spare text slot)</summary>
        public void Add(uint va, byte[] blob, bool text)
        {
            var slots = text ? Enumerable.Range(0, 7) : Enumerable.Range(7, 11).Concat(Enumerable.Range(0, 7));
            int i = slots.FirstOrDefault(k => Size[k] == 0 && !(k == 0 && Addr[0] != 0)); Bin.Require(Size[i] == 0, "no free main.dol section left");
            while (d.Count % 32 != 0) d.Add(0);
            Off[i] = (uint)d.Count; Addr[i] = va; Size[i] = (uint)((blob.Length + 31) & ~31);
            d.AddRange(blob); while (d.Count % 32 != 0) d.Add(0);
        }
        public byte[] ToArray()
        {
            var b = d.ToArray();
            for (int i = 0; i < 18; i++) { Bin.Put32(b, 4 * i, Off[i]); Bin.Put32(b, 0x48 + 4 * i, Addr[i]); Bin.Put32(b, 0x90 + 4 * i, Size[i]); }
            return b;
        }
    }
}
