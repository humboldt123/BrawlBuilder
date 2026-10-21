using System;
using System.Diagnostics;
using System.IO;
using System.Linq;

namespace BrawlBuilder.EX
{
    /// <summary>
    /// Command line front end: BrawlBuilder.exe --ex --iso &lt;Brawl USA Rev 2 .wbfs/.iso | extracted tree&gt; --mod &lt;mod download folder | sd.raw&gt;
    ///                                        --out &lt;out.wbfs|.iso&gt; [--id RSBExx] [--name "Title"] [--wit wit.exe] [--work dir] [--keep-work]
    /// --tree &lt;dir&gt; patches an already extracted tree in place (no extraction / no disc unless --out is given).
    /// </summary>
    public static class ExCli
    {
        public static int Run(string[] args)
        {
            string Arg(string k) { int i = Array.IndexOf(args, k); return i >= 0 && i + 1 < args.Length ? args[i + 1] : null; }
            bool Has(string k) => args.Contains(k);
            try
            {
                string iso = Arg("--iso"), tree = Arg("--tree"), mod = Arg("--mod"), outp = Arg("--out"), id = Arg("--id"), name = Arg("--name");
                string wit = Arg("--wit") ?? Path.Combine(AppContext.BaseDirectory, "Resources", "wit", "wit.exe");
                string work = Arg("--work") ?? Path.Combine(Path.GetDirectoryName(Path.GetFullPath(outp ?? tree ?? ".")), "BrawlBuilderEX.work");
                Bin.Require(mod != null && (iso != null || tree != null || Has("--analyze")), "usage: --ex --iso <brawl> --mod <mod folder or sd.raw> --out <file> [--id RSBExx] [--name text]");
                Directory.CreateDirectory(work);
                var m = ExMod.Resolve(mod, work, Console.WriteLine);
                Bin.Require(m != null, "no launcher-based mod (RSBE01.GCT + gc.txt/gameconfig.txt, or an sd.raw) found at " + mod);
                Console.WriteLine("mod folder: " + m.Dir + "  (SD root /" + m.Root + ", code list " + Path.GetFileName(m.GctPath) + " @0x" + m.CodeListStart.ToString("X8") + ", code handler from " + Path.GetFileName(m.HandlerSource) + ")");
                if (Has("--analyze")) return 0;
                if (tree == null)
                {
                    tree = Path.Combine(work, "ssbb.d"); if (Directory.Exists(tree)) Directory.Delete(tree, true);
                    Console.WriteLine("extracting Brawl ...");
                    Bin.Require(Wit(wit, "extract \"" + iso + "\" \"" + tree + "\" --psel=DATA -1ovv") == 0, "wit extract failed");
                }
                var b = new ExBuild { Log = Console.WriteLine, RemoveSubspace = Has("--remove-sse"), Bake = !Has("--no-bake"), Fit = Has("--fit"), Mem2Index = Has("--mem2-index"), FreeDiscSpace = !Has("--no-free"), KeepShadowed = Has("--keep-shadowed"), SubspaceList = Path.Combine(AppContext.BaseDirectory, "Resources", "SubspaceEmissaryFiles.txt") };
                b.Run(tree, m, id);
                foreach (var r in b.Report) Console.WriteLine("  " + r);
                if (outp != null)
                {
                    if (File.Exists(outp)) File.Delete(outp);
                    Console.WriteLine("building " + outp + " ...");
                    Bin.Require(Wit(wit, "copy \"" + tree + "\" \"" + outp + "\" -ovv" + (id != null ? " --id=" + id : "") + (name != null ? " --name \"" + name + "\"" : "")) == 0, "wit copy failed");
                    if (!Has("--keep-work") && Arg("--tree") == null) Directory.Delete(work, true);
                }
                Console.WriteLine("done");
                return 0;
            }
            catch (ExException e) { Console.Error.WriteLine("ERROR: " + e.Message); return 2; }
        }

        static int Wit(string exe, string args)
        {
            var p = Process.Start(new ProcessStartInfo(exe, args) { UseShellExecute = false });
            p.WaitForExit(); return p.ExitCode;
        }
    }
}
