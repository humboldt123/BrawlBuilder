using System;
using System.Windows.Forms;
using System.Reflection;
using System.IO;

namespace BrawlBuilder
{
	static class Program
	{
		/// <summary>
		/// The main entry point for the application.
		/// </summary>
		[STAThread]
		static int Main(string[] args)
		{
			if (Array.IndexOf(args, "--ex") >= 0)
				return EX.ExCli.Run(args);   // command line BrawlBuilderEX build (no window)

			Application.EnableVisualStyles();
			Application.SetCompatibleTextRenderingDefault(false);
			Application.Run(new BrawlBuilder());
			return 0;
		}
	}
}
