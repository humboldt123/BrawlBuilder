using System;
using System.Security.Cryptography;

namespace BrawlBuilder.EX
{
    public class ExException : Exception { public ExException(string m) : base(m) { } }

    /// <summary>Big-endian helpers.</summary>
    public static class Bin
    {
        public static uint Get32(byte[] d, int o) => (uint)(d[o] << 24 | d[o + 1] << 16 | d[o + 2] << 8 | d[o + 3]);
        public static void Put32(byte[] d, int o, uint v) { d[o] = (byte)(v >> 24); d[o + 1] = (byte)(v >> 16); d[o + 2] = (byte)(v >> 8); d[o + 3] = (byte)v; }
        public static void Require(bool ok, string msg) { if (!ok) throw new ExException(msg); }
        public static string Sha1(byte[] d) { using (var s = SHA1.Create()) return BitConverter.ToString(s.ComputeHash(d)).Replace("-", "").ToLowerInvariant(); }
        public static int IndexOf(byte[] h, byte[] n, int from = 0)
        {
            for (int i = from; i <= h.Length - n.Length; i++)
            {
                int k = 0; while (k < n.Length && h[i + k] == n[k]) k++;
                if (k == n.Length) return i;
            }
            return -1;
        }
    }
}
