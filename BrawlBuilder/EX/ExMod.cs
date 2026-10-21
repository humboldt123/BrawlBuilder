using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Text;
using System.Text.RegularExpressions;

namespace BrawlBuilder.EX
{
    /// <summary>
    /// A launcher-based Brawl mod (Project+ family: Project+, P+Ex/PMEX, Legacy, Minus, Void, ...), found by layout instead of by name:
    /// the folder that holds gc.txt + RSBE01.GCT is the mod folder ("/Minusery", "/Project+", "/ex_remix", "/P+Ex"); its name is the SD root
    /// the mod's codes use. The Ocarina code handler is taken from the mod's own launcher (.dol/.elf) by its code-list template.
    /// </summary>
    public class ExMod
    {
        public string Dir, Root, GctPath, CodesetPath;
        public uint CodeListStart, CodeListEnd;
        public List<(uint Addr, uint? IfEqual, uint WriteAddr, uint Value)> Pokes = new List<(uint, uint?, uint, uint)>();
        public byte[] Handler; public string HandlerSource;

        public const int HandlerLength = 0xAA8, HandlerTemplateOffset = 0x104;
        static readonly byte[] HandlerTemplate = { 0x3D, 0xE0, 0x80, 0x00, 0x61, 0xEF, 0x22, 0xA8 };     // lis r15,0x8000 ; ori r15,r15,0x22A8
        static readonly HashSet<string> KnownHandlers = new HashSet<string> { "21089b4b436dc280686a569d7af45edf96f4c276" };

        /// <summary>
        /// Accepts what a user is likely to pick: the mod's download folder, its SD folder, its "pf" folder, or a folder / file with an sd.raw
        /// image (extracted to <paramref name="work"/>). Returns null if this is not a launcher-based mod.
        /// </summary>
        public static ExMod Resolve(string path, string work, Action<string> log = null)
        {
            if (File.Exists(path) && (path.EndsWith(".raw", StringComparison.OrdinalIgnoreCase) || path.EndsWith(".img", StringComparison.OrdinalIgnoreCase)))
                return FromImage(path, work, log);
            if (!Directory.Exists(path)) return null;
            string p = Path.GetFullPath(path);
            for (int k = 0; k < 3 && p != null; k++, p = Path.GetDirectoryName(p))
            {
                try { return Find(p); } catch (ExException) { }
                var raw = Directory.EnumerateFiles(p, "*.raw").FirstOrDefault();
                if (raw != null) return FromImage(raw, work, log);
            }
            return null;
        }

        static ExMod FromImage(string raw, string work, Action<string> log)
        {
            var dir = Path.Combine(work, "sd"); if (Directory.Exists(dir)) Directory.Delete(dir, true);
            log?.Invoke("extracting " + Path.GetFileName(raw) + " ...");
            using (var fat = new FatImage(raw)) fat.ExtractAll(dir);
            var m = Find(dir);
            // the launcher (handler) may sit next to the image instead of on it
            return m;
        }

        public static ExMod Find(string package)
        {
            // the code list: RSBE01.GCT (any case) with a launcher config: gc.txt beside it, or gameconfig.txt up to two folders above
            // (MinimaLauncher layout: <sd>/codes/RSBE01.gct + <sd>/gameconfig.txt)
            var cands = new List<(string Gct, string Config)>();
            foreach (var f in Directory.EnumerateFiles(package, "*", SearchOption.AllDirectories).Where(f => Path.GetFileName(f).Equals("RSBE01.GCT", StringComparison.OrdinalIgnoreCase)))
            {
                string dir = Path.GetDirectoryName(f), cfg = FileIn(dir, "gc.txt"), up = dir;
                for (int k = 0; k < 3 && cfg == null && up != null; k++) { cfg = FileIn(up, "gameconfig.txt"); up = Path.GetDirectoryName(up); }
                if (cfg != null) cands.Add((f, cfg));
            }
            Bin.Require(cands.Count > 0, "no RSBE01.GCT with a launcher config (gc.txt / gameconfig.txt) found in " + package + " - is this a launcher-based mod?");
            var pick = cands.OrderBy(c => c.Gct.Length).First();
            var m = new ExMod { GctPath = pick.Gct };
            string gdir = Path.GetDirectoryName(m.GctPath);
            m.ParseGc(File.ReadAllText(pick.Config));
            // the SD folder the mod's codes use: the File Patch Code's root string, else the most frequent "/<folder>" string naming a real folder
            string root = RootString(GeckoList.Parse(File.ReadAllBytes(m.GctPath)));
            var near = new List<string>(); { string up = gdir; for (int k = 0; k < 3 && up != null; k++) { near.AddRange(Directory.GetDirectories(up)); near.Add(up); up = Path.GetDirectoryName(up); } }
            if (root == null)
            {
                var raw = File.ReadAllBytes(m.GctPath); var counts = new Dictionary<string, int>(StringComparer.OrdinalIgnoreCase);
                var names = new HashSet<string>(near.Select(d => Path.GetFileName(d.TrimEnd('/', (char)92))), StringComparer.OrdinalIgnoreCase);
                var skip = new HashSet<string>(StringComparer.OrdinalIgnoreCase) { "pf", "apps", "codes", "private", "module", "sound", "system" };
                foreach (Match mm in Regex.Matches(Encoding.Latin1.GetString(raw), @"/([A-Za-z0-9_+.-]{2,32})[/\x00]"))
                { var n = mm.Groups[1].Value; if (names.Contains(n) && !skip.Contains(n)) counts[n] = counts.TryGetValue(n, out int c) ? c + 1 : 1; }
                root = counts.Count > 0 ? counts.OrderByDescending(kv => kv.Value).First().Key : Path.GetFileName(gdir);
            }
            m.Dir = near.FirstOrDefault(d => Path.GetFileName(d.TrimEnd('/', (char)92)).Equals(root, StringComparison.OrdinalIgnoreCase));
            Bin.Require(m.Dir != null, "the SD folder /" + root + " was not found next to " + m.GctPath);
            m.Root = Path.GetFileName(m.Dir.TrimEnd('/', (char)92));
            foreach (var cs in new[] { "codeset.txt", "RSBE01_codeset.txt" }) { var f = FileIn(gdir, cs); if (f != null) m.CodesetPath = f; }
            // handler: prefer a known console launcher's copy
            var found = new List<(int Rank, string File, byte[] H)>();
            foreach (var f in Directory.EnumerateFiles(package, "*.*", SearchOption.AllDirectories)
                     .Where(f => f.EndsWith(".dol", StringComparison.OrdinalIgnoreCase) || f.EndsWith(".elf", StringComparison.OrdinalIgnoreCase)))
            {
                var d = File.ReadAllBytes(f); int i = Bin.IndexOf(d, HandlerTemplate);
                if (i < HandlerTemplateOffset || i - HandlerTemplateOffset + HandlerLength > d.Length) continue;
                var h = new byte[HandlerLength]; Array.Copy(d, i - HandlerTemplateOffset, h, 0, HandlerLength);
                if (Bin.Get32(h, 0xA8) != 0x9421FF58) continue;          // entry (0x800018A8) must be the handler's prologue
                found.Add(((KnownHandlers.Contains(Bin.Sha1(h)) ? 0 : 2) + (f.IndexOf("dolphin", StringComparison.OrdinalIgnoreCase) >= 0 ? 1 : 0), f, h));
            }
            Bin.Require(found.Count > 0, "no Gecko code handler found in the mod's launcher files (.dol/.elf) under " + package);
            var best = found.OrderBy(x => x.Rank).First(); m.Handler = best.H; m.HandlerSource = best.File;
            return m;
        }

        static string FileIn(string dir, string name) =>
            Directory.Exists(dir) ? Directory.EnumerateFiles(dir).FirstOrDefault(f => Path.GetFileName(f).Equals(name, StringComparison.OrdinalIgnoreCase)) : null;

        /// <summary>the string the File Patch Code writes to 0x80406920 ("/Minusery/")</summary>
        static string RootString(List<Line> l)
        {
            for (int i = 0; i < l.Count; i++)
            {
                if (((l[i].A >> 24) & 0xFE) != 6 || (0x80000000 | (l[i].A & 0x1FFFFFF)) != 0x80406920 || l[i].B == 0 || l[i].B >= 64) continue;
                var sb = new StringBuilder();
                for (int k = 0; k < l[i].B; k++) { var ln = l[i + 1 + k / 8]; uint w = (k % 8) < 4 ? ln.A : ln.B; char c = (char)((w >> (24 - 8 * (k % 4))) & 0xFF); if (c == 0) break; sb.Append(c); }
                var n = sb.ToString().Trim('/');
                if (n.Length > 0 && !n.Contains("/")) return n;
            }
            return null;
        }

        void ParseGc(string t)
        {
            uint Hex(string k) { var mm = Regex.Match(t, k + @"\s*=\s*([0-9A-Fa-f]+)"); Bin.Require(mm.Success, "gc.txt has no " + k); return Convert.ToUInt32(mm.Groups[1].Value, 16); }
            CodeListStart = Hex("codeliststart"); CodeListEnd = Hex("codelistend");
            var hook = Regex.Match(t, @"hooktype\s*=\s*([0-9]+)");
            foreach (Match mm in Regex.Matches(t, @"pokeifequal\(\s*([0-9A-Fa-f]+)\s*,\s*([0-9A-Fa-f]+)\s*,\s*([0-9A-Fa-f]+)\s*,\s*([0-9A-Fa-f]+)\s*\)"))
                Pokes.Add((Convert.ToUInt32(mm.Groups[1].Value, 16), Convert.ToUInt32(mm.Groups[2].Value, 16), Convert.ToUInt32(mm.Groups[3].Value, 16), Convert.ToUInt32(mm.Groups[4].Value, 16)));
            foreach (Match mm in Regex.Matches(t, @"(?<![a-z])poke\(\s*([0-9A-Fa-f]+)\s*,\s*([0-9A-Fa-f]+)\s*\)"))
            { uint a = Convert.ToUInt32(mm.Groups[1].Value, 16); Pokes.Add((a, null, a, Convert.ToUInt32(mm.Groups[2].Value, 16))); }
        }

        static readonly HashSet<string> DevDirs = new HashSet<string>(StringComparer.OrdinalIgnoreCase) { "source", "tools", "documentation", "gctrealmate" };
        static readonly string[] DevExt = { ".exe", ".dll", ".bat", ".map", ".md", ".bak", ".ini", ".log" };

        /// <summary>the mod folder exactly as it sits on the SD card, as (source file, disc path under files/) - development leftovers skipped</summary>
        public List<(string Src, string Disc)> SdFiles(out int skippedNonAscii)
        {
            var o = new List<(string, string)>(); skippedNonAscii = 0;
            foreach (var f in Directory.EnumerateFiles(Dir, "*", SearchOption.AllDirectories))
            {
                var rel = Path.GetRelativePath(Dir, f).Replace('\\', '/');
                var top = rel.Contains('/') ? rel.Substring(0, rel.IndexOf('/')) : "";
                if (DevDirs.Contains(top) || DevExt.Any(e => f.EndsWith(e, StringComparison.OrdinalIgnoreCase))) continue;
                var disc = Root + "/" + rel;
                if (disc.Any(c => c > 0x7E)) { skippedNonAscii++; continue; }          // FST names are ASCII; such files are tool leftovers
                o.Add((f, disc));
            }
            return o;
        }
    }
}
